//! `dict<K, V>` and `set<T>`: one insertion-ordered hash table (DESIGN §4.1).
//!
//! # Layout
//!
//! A dict value is a pointer to a [`TyDict`] header. Its first field is the
//! number of live entries, so `len(d)` is one inline load exactly like a
//! list's; everything else is private to this module, and the compiler only
//! ever reaches it through the `ty_dict_*` entry points.
//!
//! The table is CPython's "compact dict": a dense **entry array** in insertion
//! order, plus an open-addressing **index** that maps a hash to an entry.
//!
//! ```text
//! entries -> [ { u64 hash, K key, V value } ... ]   append-only, `used` long
//! index   -> [ u64 slot ... ]                       power of two, linear probing
//!            slot = (high 32 bits of the hash) << 32 | (entry index + 1)
//!            0 = empty, low half u32::MAX = deleted (a tombstone)
//! ```
//!
//! * Iteration walks the entry array, so it is insertion order on every
//!   target (the order the language promises, DESIGN §4.1), and deleting a key
//!   then re-inserting it moves it to the end, as in Python.
//! * The index slot carries 32 bits of the hash, so a probe rejects almost
//!   every non-matching slot without touching the entry, and the entry keeps
//!   the full hash, so growing the table never re-hashes a key (re-hashing a
//!   string key would touch its bytes, a cache miss per key).
//! * A deleted entry gets hash 0 (a live hash is never 0) and stays in place
//!   until the next rebuild, which compacts the array.
//! * `{ K key, V value }` is laid out exactly like `tuple<K, V>`, so `items()`
//!   copies one block per entry, and `keys()` / `values()` one field.
//!
//! # Keys
//!
//! The key type is described by a **descriptor** (see [`KeyShape`]): a `u64`
//! holding one 4-bit field kind per tuple member, first member in the lowest
//! nibble. The runtime derives every offset from it with the same C layout
//! rules both code generators use, so it agrees with them on every target
//! without being told a single offset.
//!
//! A key crosses the C ABI as one 64-bit **key word**: an `int` is itself, a
//! `bool` is 0 / 1, a `str` is its address, and a tuple is the address of a
//! buffer holding it laid out as a tuple (an `alloca` natively, the static
//! [`ty_key_buf`] on wasm).
//!
//! # The lookup cache
//!
//! `counts[k] = counts.get(k, 0) + 1` and `if k in d: d[k] = d[k] + 1` look the
//! same key up two or three times in a row. The table remembers its last
//! probe — key word, hash, slot, entry or miss — and a lookup of the same key
//! word answers from it. It is only used for `int`, `bool` and `str` keys,
//! whose key word identifies the key (a `str` is immutable, so the same
//! address is the same string), and every mutation refreshes or clears it, so
//! a cached miss still names the empty slot an insertion goes to.
//!
//! # wasm
//!
//! The table is identical on wasm32, where a pointer is four bytes: only the
//! offsets of pointer-shaped key members move, and those come from the same
//! descriptor. With no collector there (DESIGN §6.2.1), the old arrays a
//! rebuild leaves behind are simply never reused.

use std::io::Write;

use crate::{TyList, str_bytes, ty_alloc, ty_alloc_atomic, ty_list_new, ty_panic};

// ---------------------------------------------------------------------------
// Key descriptors
// ---------------------------------------------------------------------------

/// Descriptor kind of an `int` member: 8 bytes, 8-aligned.
pub const KEY_INT: u64 = 1;
/// Descriptor kind of a `float` member (sorting only; a `float` is not a
/// valid dict key): 8 bytes, 8-aligned.
pub const KEY_FLOAT: u64 = 2;
/// Descriptor kind of a `bool` member: one byte.
pub const KEY_BOOL: u64 = 3;
/// Descriptor kind of a `str` member: one pointer.
pub const KEY_STR: u64 = 4;

/// The most members a descriptor can describe: 16 nibbles in a `u64`.
pub const MAX_KEY_FIELDS: usize = 16;

/// Size of a pointer on the target the runtime is compiled for.
const PTR_SIZE: usize = size_of::<*const u8>();

/// The memory layout a key descriptor stands for: the kind and offset of
/// every member, and the size and alignment of the whole.
///
/// ```
/// # use typhoon_runtime::dict::{KeyShape, KEY_BOOL, KEY_INT, KEY_STR};
/// // tuple<bool, int, str>: bool at 0, int at 8, the pointer at 16.
/// let shape = KeyShape::of(KEY_BOOL | KEY_INT << 4 | KEY_STR << 8);
/// assert_eq!(shape.offsets(), &[0, 8, 16][..shape.len()]);
/// assert!(shape.has_pointers);
/// ```
#[derive(Debug, Clone, Copy)]
pub struct KeyShape {
    kinds: [u8; MAX_KEY_FIELDS],
    offsets: [u8; MAX_KEY_FIELDS],
    count: usize,
    /// Size in bytes, rounded up to `align`: the size of one list element.
    pub size: usize,
    /// Alignment of the widest member.
    pub align: usize,
    /// Whether any member is a pointer the collector must trace.
    pub has_pointers: bool,
}

/// Size (and alignment) of one descriptor kind.
fn kind_size(kind: u8) -> usize {
    match u64::from(kind) {
        KEY_INT | KEY_FLOAT => 8,
        KEY_BOOL => 1,
        _ => PTR_SIZE,
    }
}

impl KeyShape {
    /// Decodes a descriptor.
    pub fn of(desc: u64) -> KeyShape {
        let mut shape = KeyShape {
            kinds: [0; MAX_KEY_FIELDS],
            offsets: [0; MAX_KEY_FIELDS],
            count: 0,
            size: 0,
            align: 1,
            has_pointers: false,
        };
        let mut rest = desc;
        let mut offset = 0usize;
        while rest != 0 && shape.count < MAX_KEY_FIELDS {
            let kind = (rest & 0xF) as u8;
            rest >>= 4;
            // Every kind is aligned to its own size.
            let size = kind_size(kind);
            offset = offset.next_multiple_of(size);
            shape.kinds[shape.count] = kind;
            shape.offsets[shape.count] = offset as u8;
            shape.count += 1;
            offset += size;
            shape.align = shape.align.max(size);
            shape.has_pointers |= u64::from(kind) == KEY_STR;
        }
        shape.size = offset.next_multiple_of(shape.align);
        shape
    }

    /// The number of members.
    pub fn len(&self) -> usize {
        self.count
    }

    /// Whether the descriptor was empty.
    pub fn is_empty(&self) -> bool {
        self.count == 0
    }

    /// The byte offset of every member.
    pub fn offsets(&self) -> &[u8] {
        &self.offsets[..self.count]
    }

    /// The kind (`KEY_*`) of member `i`.
    pub fn kind(&self, i: usize) -> u64 {
        u64::from(self.kinds[i])
    }

    /// The members as `(kind, offset)` pairs.
    fn fields(&self) -> impl Iterator<Item = (u64, usize)> + '_ {
        self.kinds[..self.count]
            .iter()
            .zip(&self.offsets[..self.count])
            .map(|(k, o)| (u64::from(*k), usize::from(*o)))
    }
}

// ---------------------------------------------------------------------------
// Raw memory access
// ---------------------------------------------------------------------------

unsafe fn read_u64(p: *const u8) -> u64 {
    unsafe { std::ptr::read_unaligned(p as *const u64) }
}

unsafe fn write_u64(p: *mut u8, value: u64) {
    unsafe { std::ptr::write_unaligned(p as *mut u64, value) }
}

unsafe fn read_ptr(p: *const u8) -> *const u8 {
    unsafe { std::ptr::read_unaligned(p as *const *const u8) }
}

unsafe fn write_ptr(p: *mut u8, value: *const u8) {
    unsafe { std::ptr::write_unaligned(p as *mut *const u8, value) }
}

/// The address a key word or a `str` key word stands for.
fn word_ptr(word: u64) -> *const u8 {
    word as usize as *const u8
}

// ---------------------------------------------------------------------------
// Hashing
// ---------------------------------------------------------------------------

const P0: u64 = 0xa076_1d64_78bd_642f;
const P1: u64 = 0xe703_7ed1_a0b4_28db;
const P2: u64 = 0x8ebc_6af0_9c88_c6e3;
const P3: u64 = 0x5899_65cc_7537_4cc3;

/// The 128-bit product of `a` and `b`, folded to 64 bits (wyhash's `mum`).
#[inline(always)]
fn mum(a: u64, b: u64) -> u64 {
    let r = u128::from(a).wrapping_mul(u128::from(b));
    (r as u64) ^ ((r >> 64) as u64)
}

#[inline(always)]
fn r8(p: &[u8], at: usize) -> u64 {
    debug_assert!(at + 8 <= p.len());
    unsafe { std::ptr::read_unaligned(p.as_ptr().add(at) as *const u64) }.to_le()
}

#[inline(always)]
fn r4(p: &[u8], at: usize) -> u64 {
    debug_assert!(at + 4 <= p.len());
    u64::from(unsafe { std::ptr::read_unaligned(p.as_ptr().add(at) as *const u32) }.to_le())
}

/// Hashes a byte string, wyhash-style: two multiplies for anything up to 16
/// bytes, one more per further 16 bytes.
///
/// ```
/// # use typhoon_runtime::dict::hash_bytes;
/// assert_eq!(hash_bytes(b"GGTATT"), hash_bytes(b"GGTATT"));
/// assert_ne!(hash_bytes(b"GGTATT"), hash_bytes(b"GGTATA"));
/// ```
pub fn hash_bytes(p: &[u8]) -> u64 {
    let len = p.len();
    let mut seed = P0;
    let (a, b) = if len <= 16 {
        if len >= 4 {
            let quarter = (len >> 3) << 2;
            (
                (r4(p, 0) << 32) | r4(p, quarter),
                (r4(p, len - 4) << 32) | r4(p, len - 4 - quarter),
            )
        } else if len > 0 {
            (
                (u64::from(p[0]) << 16) | (u64::from(p[len >> 1]) << 8) | u64::from(p[len - 1]),
                0,
            )
        } else {
            (0, 0)
        }
    } else {
        let mut at = 0;
        while len - at > 16 {
            seed = mum(r8(p, at) ^ P1, r8(p, at + 8) ^ seed);
            at += 16;
        }
        (r8(p, len - 16), r8(p, len - 8))
    };
    mum(P1 ^ len as u64, mum(a ^ P1, b ^ seed))
}

/// Hashes an `int` (or a `bool` widened to one).
pub fn hash_int(value: u64) -> u64 {
    mum(value ^ P0, P1)
}

/// Hashes a tuple key laid out at `p`, member by member.
///
/// # Safety
///
/// `p` must point at a value laid out as `shape` describes, whose `str`
/// members point at well-formed strings.
unsafe fn hash_fields(shape: &KeyShape, p: *const u8) -> u64 {
    let mut h = P2;
    for (kind, offset) in shape.fields() {
        let field = unsafe { p.add(offset) };
        let fh = match kind {
            KEY_BOOL => hash_int(u64::from(unsafe { *field })),
            KEY_STR => hash_bytes(unsafe { str_bytes(read_ptr(field)) }),
            _ => hash_int(unsafe { read_u64(field) }),
        };
        h = mum(h ^ fh, P3);
    }
    h
}

/// Member-wise equality of two values laid out as `shape` describes.
///
/// # Safety
///
/// As for [`hash_fields`], for both values.
unsafe fn fields_equal(shape: &KeyShape, a: *const u8, b: *const u8) -> bool {
    shape.fields().all(|(kind, offset)| unsafe {
        let (x, y) = (a.add(offset), b.add(offset));
        match kind {
            KEY_BOOL => *x == *y,
            KEY_STR => str_bytes(read_ptr(x)) == str_bytes(read_ptr(y)),
            _ => read_u64(x) == read_u64(y),
        }
    })
}

/// Writes the repr of one member, for a `key not found` panic.
unsafe fn write_field(w: &mut Vec<u8>, kind: u64, p: *const u8) {
    unsafe {
        let _ = match kind {
            KEY_BOOL => w.write_all(if *p != 0 { b"True" } else { b"False" }),
            KEY_STR => crate::write_str_repr(w, str_bytes(read_ptr(p))),
            KEY_FLOAT => crate::write_float(w, f64::from_bits(read_u64(p))),
            _ => crate::write_int(w, read_u64(p) as i64),
        };
    }
}

// ---------------------------------------------------------------------------
// The table
// ---------------------------------------------------------------------------

/// How a table compares and hashes its keys.
const FAST_GENERIC: u32 = 0;
const FAST_INT: u32 = 1;
const FAST_BOOL: u32 = 2;
const FAST_STR: u32 = 3;

/// The low half of a deleted index slot.
const TOMBSTONE: u32 = u32::MAX;

/// Size of the hash at the start of every entry.
const ENTRY_HASH: usize = 8;

/// The header of a `dict<K, V>` or `set<T>` value; see the module docs.
#[repr(C)]
pub struct TyDict {
    /// The number of live entries: `len(d)`. The compiler reads it inline.
    pub len: i64,
    /// Entries written to `entries`, live or deleted.
    used: i64,
    /// How many entries `entries` has room for.
    entry_cap: i64,
    /// Index slots minus one; the slot count is a power of two.
    mask: u64,
    /// The key descriptor.
    desc: u64,
    /// The key word of the last lookup.
    cache_word: u64,
    /// Its hash.
    cache_hash: u64,
    /// The index slot it ended at: its slot on a hit, the first empty slot
    /// of its probe sequence on a miss.
    cache_pos: u64,
    /// Its entry, or -1 for a miss.
    cache_entry: i64,
    /// One of the `FAST_*` key classes.
    fast: u32,
    /// Whether the `cache_*` fields describe the current table.
    cache_valid: u32,
    /// Size of a key, padded like a list element.
    key_size: u32,
    /// Offset of the value inside an entry.
    val_off: u32,
    /// Size of a value; 0 for a set.
    val_size: u32,
    /// Size of `{ key, value }`, i.e. of `tuple<K, V>`.
    pair_size: u32,
    /// Size of a whole entry, hash included, a multiple of 8.
    entry_size: u32,
    /// Bit 0: the entry array holds no pointers. Bit 1: values hold none.
    flags: u32,
    /// The entry array, null until the first insertion.
    entries: *mut u8,
    /// The index, null until the first insertion.
    index: *mut u64,
}

/// Byte offset of a table's `len` field, which the compiler reads inline.
pub const DICT_LEN_OFFSET: usize = std::mem::offset_of!(TyDict, len);

const _: () = assert!(DICT_LEN_OFFSET == 0);

const FLAG_ENTRIES_ATOMIC: u32 = 1;
const FLAG_VALUES_ATOMIC: u32 = 2;

/// The outcome of one probe sequence.
enum Probe {
    /// The key is in entry `entry`, referenced from index slot `pos`.
    Hit { pos: usize, entry: usize },
    /// The key is absent; `pos` is the empty slot that ended the search.
    Miss { pos: usize },
}

/// Entries a table with `slots` index slots may hold: a load factor of 5/8,
/// where linear probing still averages about four slots per miss.
fn capacity_for(slots: usize) -> usize {
    slots - slots / 4 - slots / 8
}

impl TyDict {
    #[inline(always)]
    unsafe fn entry(&self, idx: usize) -> *mut u8 {
        unsafe { self.entries.add(idx * self.entry_size as usize) }
    }

    #[inline(always)]
    unsafe fn value_of(&self, idx: usize) -> *mut u8 {
        unsafe { self.entry(idx).add(self.val_off as usize) }
    }

    /// Whether the lookup cache answers for `word`.
    #[inline(always)]
    fn cached(&self, word: u64) -> bool {
        self.cache_valid != 0 && self.cache_word == word
    }

    /// Records the outcome of a probe for `word`.
    #[inline(always)]
    fn remember(&mut self, word: u64, h: u64, pos: usize, entry: i64) {
        if self.fast != FAST_GENERIC {
            self.cache_valid = 1;
            self.cache_word = word;
            self.cache_hash = h;
            self.cache_pos = pos as u64;
            self.cache_entry = entry;
        }
    }

    /// The hash of the key `word` stands for; never 0, which marks a deleted
    /// entry.
    #[inline(always)]
    unsafe fn hash_word(&self, word: u64) -> u64 {
        let h = match self.fast {
            FAST_INT | FAST_BOOL => hash_int(word),
            FAST_STR => hash_bytes(unsafe { str_bytes(word_ptr(word)) }),
            _ => unsafe { hash_fields(&KeyShape::of(self.desc), word_ptr(word)) },
        };
        h.max(1)
    }

    /// Whether the key stored at `stored` is the one `word` stands for.
    #[inline(always)]
    unsafe fn key_matches(&self, stored: *const u8, word: u64) -> bool {
        unsafe {
            match self.fast {
                FAST_INT => read_u64(stored) == word,
                FAST_BOOL => u64::from(*stored) == word,
                FAST_STR => {
                    let (a, b) = (read_ptr(stored), word_ptr(word));
                    a == b || str_bytes(a) == str_bytes(b)
                }
                _ => fields_equal(&KeyShape::of(self.desc), stored, word_ptr(word)),
            }
        }
    }

    /// Copies the key `word` stands for into an entry.
    unsafe fn store_key(&self, dst: *mut u8, word: u64) {
        unsafe {
            match self.fast {
                FAST_INT => write_u64(dst, word),
                FAST_BOOL => *dst = word as u8,
                FAST_STR => write_ptr(dst, word_ptr(word)),
                _ => std::ptr::copy_nonoverlapping(word_ptr(word), dst, self.key_size as usize),
            }
        }
    }

    /// Walks the probe sequence of `h`. The index must exist.
    #[inline(always)]
    unsafe fn probe(&self, word: u64, h: u64) -> Probe {
        let mask = self.mask as usize;
        let tag = (h >> 32) as u32;
        let mut pos = h as usize & mask;
        loop {
            let slot = unsafe { *self.index.add(pos) };
            if slot == 0 {
                return Probe::Miss { pos };
            }
            let idx = slot as u32;
            if (slot >> 32) as u32 == tag && idx != TOMBSTONE {
                let entry = idx as usize - 1;
                unsafe {
                    let e = self.entry(entry);
                    if read_u64(e) == h && self.key_matches(e.add(ENTRY_HASH), word) {
                        return Probe::Hit { pos, entry };
                    }
                }
            }
            pos = (pos + 1) & mask;
        }
    }

    /// The entry holding `word`'s key, if any, through the lookup cache.
    unsafe fn lookup(&mut self, word: u64) -> Option<usize> {
        if self.cached(word) {
            return usize::try_from(self.cache_entry).ok();
        }
        if self.len == 0 {
            return None;
        }
        let h = unsafe { self.hash_word(word) };
        match unsafe { self.probe(word, h) } {
            Probe::Hit { pos, entry } => {
                self.remember(word, h, pos, entry as i64);
                Some(entry)
            }
            Probe::Miss { pos } => {
                self.remember(word, h, pos, -1);
                None
            }
        }
    }

    /// The value slot of `word`'s key, inserting the key first if needed.
    unsafe fn insert(&mut self, word: u64) -> *mut u8 {
        unsafe {
            if self.cached(word) {
                if self.cache_entry >= 0 {
                    return self.value_of(self.cache_entry as usize);
                }
                return self.insert_at(word, self.cache_hash, self.cache_pos as usize);
            }
            if self.index.is_null() {
                self.rebuild(8);
            }
            let h = self.hash_word(word);
            match self.probe(word, h) {
                Probe::Hit { pos, entry } => {
                    self.remember(word, h, pos, entry as i64);
                    self.value_of(entry)
                }
                Probe::Miss { pos } => self.insert_at(word, h, pos),
            }
        }
    }

    /// Appends a new entry for `word`'s key, whose probe ended at the empty
    /// slot `pos`, and returns its (uninitialised) value slot.
    unsafe fn insert_at(&mut self, word: u64, h: u64, pos: usize) -> *mut u8 {
        unsafe {
            let mut pos = pos;
            if self.used >= self.entry_cap {
                self.grow();
                pos = self.empty_slot(h);
            }
            let idx = self.used as usize;
            let e = self.entry(idx);
            write_u64(e, h);
            self.store_key(e.add(ENTRY_HASH), word);
            *self.index.add(pos) = (h & 0xFFFF_FFFF_0000_0000) | (idx as u64 + 1);
            self.used += 1;
            self.len += 1;
            self.remember(word, h, pos, idx as i64);
            self.value_of(idx)
        }
    }

    /// The first empty slot in the probe sequence of `h`.
    unsafe fn empty_slot(&self, h: u64) -> usize {
        let mask = self.mask as usize;
        let mut pos = h as usize & mask;
        while unsafe { *self.index.add(pos) } != 0 {
            pos = (pos + 1) & mask;
        }
        pos
    }

    /// Makes room for one more entry: doubles the index, or — when at least
    /// half of the entries are deleted — compacts at the same size.
    unsafe fn grow(&mut self) {
        let slots = if self.index.is_null() {
            0
        } else {
            self.mask as usize + 1
        };
        let new_slots = if slots == 0 {
            8
        } else if (self.len as usize) * 2 <= self.used as usize {
            slots
        } else {
            slots * 2
        };
        unsafe { self.rebuild(new_slots) };
    }

    /// Re-creates the entry array (live entries only, in order) and the index
    /// with `slots` slots.
    unsafe fn rebuild(&mut self, slots: usize) {
        let cap = capacity_for(slots);
        let entry_size = self.entry_size as usize;
        let bytes = cap * entry_size;
        let entries = if self.flags & FLAG_ENTRIES_ATOMIC != 0 {
            ty_alloc_atomic(bytes)
        } else {
            ty_alloc(bytes)
        };
        let mut live = 0usize;
        unsafe {
            if self.used > 0 {
                if self.used == self.len {
                    std::ptr::copy_nonoverlapping(
                        self.entries,
                        entries,
                        self.used as usize * entry_size,
                    );
                    live = self.used as usize;
                } else {
                    for i in 0..self.used as usize {
                        let e = self.entry(i);
                        if read_u64(e) != 0 {
                            std::ptr::copy_nonoverlapping(
                                e,
                                entries.add(live * entry_size),
                                entry_size,
                            );
                            live += 1;
                        }
                    }
                }
            }
            // An atomic block is not guaranteed to be zeroed.
            let index = ty_alloc_atomic(slots * 8) as *mut u64;
            std::ptr::write_bytes(index, 0, slots);
            let mask = slots - 1;
            for i in 0..live {
                let h = read_u64(entries.add(i * entry_size));
                let mut pos = h as usize & mask;
                while *index.add(pos) != 0 {
                    pos = (pos + 1) & mask;
                }
                *index.add(pos) = (h & 0xFFFF_FFFF_0000_0000) | (i as u64 + 1);
            }
            self.entries = entries;
            self.index = index;
        }
        self.mask = (slots - 1) as u64;
        self.entry_cap = cap as i64;
        self.used = live as i64;
        self.cache_valid = 0;
    }

    /// Deletes `word`'s key, returning the entry it occupied. The entry's value
    /// stays readable until the next insertion.
    unsafe fn remove(&mut self, word: u64) -> Option<usize> {
        if self.len == 0 {
            return None;
        }
        let (pos, entry) = if self.cached(word) {
            let entry = usize::try_from(self.cache_entry).ok()?;
            (self.cache_pos as usize, entry)
        } else {
            let h = unsafe { self.hash_word(word) };
            match unsafe { self.probe(word, h) } {
                Probe::Hit { pos, entry } => (pos, entry),
                Probe::Miss { .. } => return None,
            }
        };
        unsafe {
            *self.index.add(pos) = u64::from(TOMBSTONE);
            let e = self.entry(entry);
            write_u64(e, 0);
            // Drop the key's references early; the value is still needed by
            // the caller of `pop`.
            std::ptr::write_bytes(e.add(ENTRY_HASH), 0, self.key_size as usize);
        }
        self.len -= 1;
        self.cache_valid = 0;
        Some(entry)
    }

    /// Copies one field of every live entry into a fresh list.
    unsafe fn collect(&self, offset: usize, size: usize, atomic: bool) -> *mut TyList {
        let list = ty_list_new(size as i64, u8::from(atomic), self.len);
        unsafe {
            let data = (*list).data;
            let mut n = 0usize;
            for i in 0..self.used.max(0) as usize {
                let e = self.entry(i);
                if read_u64(e) != 0 {
                    std::ptr::copy_nonoverlapping(e.add(offset), data.add(n * size), size);
                    n += 1;
                }
            }
            (*list).len = n as i64;
        }
        list
    }

    /// Panics with `key not found: <repr of the key>`.
    unsafe fn missing(&self, word: u64) -> ! {
        let mut msg = b"key not found: ".to_vec();
        unsafe {
            match self.fast {
                FAST_INT => write_field(&mut msg, KEY_INT, (&raw const word) as *const u8),
                FAST_BOOL => {
                    let byte = word as u8;
                    write_field(&mut msg, KEY_BOOL, &raw const byte);
                }
                FAST_STR => {
                    let _ = crate::write_str_repr(&mut msg, str_bytes(word_ptr(word)));
                }
                _ => {
                    let shape = KeyShape::of(self.desc);
                    msg.push(b'(');
                    for (i, (kind, offset)) in shape.fields().enumerate() {
                        if i > 0 {
                            msg.extend_from_slice(b", ");
                        }
                        write_field(&mut msg, kind, word_ptr(word).add(offset));
                    }
                    if shape.len() == 1 {
                        msg.push(b',');
                    }
                    msg.push(b')');
                }
            }
            ty_panic(msg.as_ptr(), msg.len())
        }
    }
}

// ---------------------------------------------------------------------------
// The C ABI
// ---------------------------------------------------------------------------

/// A fresh, empty table for keys described by `desc` and values of
/// `val_size` bytes aligned to `val_align` (`val_size == 0` for a set).
///
/// `val_atomic` is 1 when a value holds no pointer, so that a table whose keys
/// and values are both pointer-free keeps its entries in an unscanned block
/// (DESIGN §5.1). Nothing but the header is allocated until the first key
/// goes in.
#[unsafe(no_mangle)]
pub extern "C" fn ty_dict_new(
    desc: i64,
    val_size: i64,
    val_align: i64,
    val_atomic: u8,
) -> *mut TyDict {
    let desc = desc as u64;
    let shape = KeyShape::of(desc);
    let val_size = val_size.max(0) as usize;
    let val_align = (val_align.max(1) as usize).next_power_of_two();
    let val_in_pair = shape.size.next_multiple_of(val_align);
    let pair_size = (val_in_pair + val_size).next_multiple_of(shape.align.max(val_align));
    let entry_size = (ENTRY_HASH + pair_size).next_multiple_of(8);
    let fast = match desc {
        KEY_INT => FAST_INT,
        KEY_BOOL => FAST_BOOL,
        KEY_STR => FAST_STR,
        _ => FAST_GENERIC,
    };
    let values_atomic = val_atomic != 0 || val_size == 0;
    let mut flags = 0;
    if values_atomic {
        flags |= FLAG_VALUES_ATOMIC;
    }
    if values_atomic && !shape.has_pointers {
        flags |= FLAG_ENTRIES_ATOMIC;
    }
    let d = ty_alloc(size_of::<TyDict>()) as *mut TyDict;
    unsafe {
        std::ptr::write(
            d,
            TyDict {
                len: 0,
                used: 0,
                entry_cap: 0,
                mask: 0,
                desc,
                cache_word: 0,
                cache_hash: 0,
                cache_pos: 0,
                cache_entry: -1,
                fast,
                cache_valid: 0,
                key_size: shape.size as u32,
                val_off: (ENTRY_HASH + val_in_pair) as u32,
                val_size: val_size as u32,
                pair_size: pair_size as u32,
                entry_size: entry_size as u32,
                flags,
                entries: std::ptr::null_mut(),
                index: std::ptr::null_mut(),
            },
        );
    }
    d
}

/// The value slot of `key`, or null when the key is absent: `k in d` and
/// `d.get(k, default)`.
///
/// # Safety
///
/// `d` must come from [`ty_dict_new`] and `key` must be a key word of its key
/// type (see the module docs).
#[unsafe(no_mangle)]
pub unsafe extern "C" fn ty_dict_find(d: *mut TyDict, key: i64) -> *mut u8 {
    unsafe {
        let d = &mut *d;
        match d.lookup(key as u64) {
            Some(entry) => d.value_of(entry),
            None => std::ptr::null_mut(),
        }
    }
}

/// The value slot of `key`: `d[k]`. Panics with `key not found: <key>` when
/// the key is absent (DESIGN §4.3).
///
/// # Safety
///
/// As for [`ty_dict_find`].
#[unsafe(no_mangle)]
pub unsafe extern "C" fn ty_dict_get(d: *mut TyDict, key: i64) -> *mut u8 {
    unsafe {
        let d = &mut *d;
        match d.lookup(key as u64) {
            Some(entry) => d.value_of(entry),
            None => d.missing(key as u64),
        }
    }
}

/// The value slot of `key`, inserting the key first when it is absent:
/// `d[k] = v` and `s.add(x)`.
///
/// A new entry's value slot is uninitialised; the caller stores the value
/// before anything else touches the table. An existing key keeps its place in
/// the iteration order, as in Python.
///
/// # Safety
///
/// As for [`ty_dict_find`].
#[unsafe(no_mangle)]
pub unsafe extern "C" fn ty_dict_insert(d: *mut TyDict, key: i64) -> *mut u8 {
    unsafe { (*d).insert(key as u64) }
}

/// Deletes `key`, returning its value slot — still readable until the next
/// insertion — or null when the key was absent: `s.discard(x)` and
/// `d.pop(k, default)`.
///
/// # Safety
///
/// As for [`ty_dict_find`].
#[unsafe(no_mangle)]
pub unsafe extern "C" fn ty_dict_remove(d: *mut TyDict, key: i64) -> *mut u8 {
    unsafe {
        let d = &mut *d;
        match d.remove(key as u64) {
            Some(entry) => d.value_of(entry),
            None => std::ptr::null_mut(),
        }
    }
}

/// Deletes `key` and returns its value slot like [`ty_dict_remove`], but
/// panics with `key not found: <key>` when it is absent: `d.pop(k)` and
/// `s.remove(x)`.
///
/// # Safety
///
/// As for [`ty_dict_find`].
#[unsafe(no_mangle)]
pub unsafe extern "C" fn ty_dict_pop(d: *mut TyDict, key: i64) -> *mut u8 {
    unsafe {
        let d = &mut *d;
        match d.remove(key as u64) {
            Some(entry) => d.value_of(entry),
            None => d.missing(key as u64),
        }
    }
}

/// `d.keys()`, and the snapshot `for k in d` / `for x in s` iterate: a fresh
/// `list<K>` in insertion order.
///
/// # Safety
///
/// `d` must come from [`ty_dict_new`].
#[unsafe(no_mangle)]
pub unsafe extern "C" fn ty_dict_keys(d: *const TyDict) -> *mut TyList {
    unsafe {
        let d = &*d;
        let atomic = !KeyShape::of(d.desc).has_pointers;
        d.collect(ENTRY_HASH, d.key_size as usize, atomic)
    }
}

/// `d.values()`: a fresh `list<V>` in insertion order.
///
/// # Safety
///
/// `d` must come from [`ty_dict_new`].
#[unsafe(no_mangle)]
pub unsafe extern "C" fn ty_dict_values(d: *const TyDict) -> *mut TyList {
    unsafe {
        let d = &*d;
        let atomic = d.flags & FLAG_VALUES_ATOMIC != 0;
        d.collect(d.val_off as usize, d.val_size as usize, atomic)
    }
}

/// `d.items()`: a fresh `list<tuple<K, V>>` in insertion order. An entry is
/// laid out as `{ hash, tuple<K, V> }`, so each item is one copy.
///
/// # Safety
///
/// `d` must come from [`ty_dict_new`].
#[unsafe(no_mangle)]
pub unsafe extern "C" fn ty_dict_items(d: *const TyDict) -> *mut TyList {
    unsafe {
        let d = &*d;
        let atomic = d.flags & FLAG_ENTRIES_ATOMIC != 0;
        d.collect(ENTRY_HASH, d.pair_size as usize, atomic)
    }
}

/// The buffer the wasm backend lays a tuple key out in before passing its
/// address as the key word.
///
/// The native backend uses a stack slot instead; wasm has no addressable
/// stack, and a heap block per lookup would never be freed there (DESIGN
/// §6.2.1). A Typhoon program is single-threaded and the buffer is filled
/// immediately before the one call that reads it, so one static buffer is
/// enough. It holds the largest key a descriptor can describe.
#[unsafe(no_mangle)]
pub extern "C" fn ty_key_buf() -> *mut u8 {
    (&raw mut KEY_BUF) as *mut u8
}

/// Storage behind [`ty_key_buf`]: 16 members of at most 8 bytes each.
#[repr(C, align(8))]
struct KeyBuf([u8; MAX_KEY_FIELDS * 8]);

static mut KEY_BUF: KeyBuf = KeyBuf([0; MAX_KEY_FIELDS * 8]);

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{str_new, ty_str_from_int};

    fn str_key(text: &str) -> i64 {
        str_new(text.as_bytes()) as usize as i64
    }

    unsafe fn read_i64(p: *const u8) -> i64 {
        unsafe { std::ptr::read_unaligned(p as *const i64) }
    }

    unsafe fn write_i64(p: *mut u8, v: i64) {
        unsafe { std::ptr::write_unaligned(p as *mut i64, v) }
    }

    #[test]
    fn shapes_follow_the_c_layout() {
        let s = KeyShape::of(KEY_STR);
        assert_eq!((s.size, s.align, s.len()), (PTR_SIZE, PTR_SIZE, 1));
        let s = KeyShape::of(KEY_INT | KEY_BOOL << 4);
        assert_eq!(s.offsets(), &[0, 8]);
        assert_eq!(s.size, 16);
        assert!(!s.has_pointers);
        let s = KeyShape::of(KEY_BOOL | KEY_BOOL << 4 | KEY_BOOL << 8);
        assert_eq!(s.size, 3);
        assert!(KeyShape::of(0).is_empty());
    }

    #[test]
    fn int_keys_insert_find_and_keep_their_order() {
        unsafe {
            let d = ty_dict_new(KEY_INT as i64, 8, 8, 1);
            for k in [5i64, -1, 1 << 40, 0, 7] {
                write_i64(ty_dict_insert(d, k), k * 10);
            }
            assert_eq!((*d).len, 5);
            assert_eq!(read_i64(ty_dict_get(d, 1 << 40)), (1 << 40) * 10);
            assert!(ty_dict_find(d, 3).is_null());
            // Overwriting keeps the position.
            write_i64(ty_dict_insert(d, -1), 99);
            let keys = ty_dict_keys(d);
            let got: Vec<i64> = (0..(*keys).len as usize)
                .map(|i| read_i64((*keys).data.add(i * 8)))
                .collect();
            assert_eq!(got, [5, -1, 1 << 40, 0, 7]);
            let values = ty_dict_values(d);
            assert_eq!(read_i64((*values).data.add(8)), 99);
        }
    }

    #[test]
    fn many_keys_survive_growth_and_deletion() {
        unsafe {
            let d = ty_dict_new(KEY_INT as i64, 8, 8, 1);
            for k in 0..10_000i64 {
                write_i64(ty_dict_insert(d, k * 7919), k);
            }
            for k in (0..10_000i64).step_by(2) {
                assert_eq!(read_i64(ty_dict_pop(d, k * 7919)), k);
            }
            assert_eq!((*d).len, 5_000);
            for k in 0..10_000i64 {
                let found = ty_dict_find(d, k * 7919);
                assert_eq!(found.is_null(), k % 2 == 0, "key {k}");
            }
            // Re-inserting deleted keys appends them after the survivors.
            for k in (0..10_000i64).step_by(2) {
                write_i64(ty_dict_insert(d, k * 7919), -k);
            }
            let keys = ty_dict_keys(d);
            assert_eq!((*keys).len, 10_000);
            assert_eq!(read_i64((*keys).data), 7919);
            assert_eq!(read_i64((*keys).data.add(5_000 * 8)), 0);
        }
    }

    #[test]
    fn str_keys_compare_by_contents() {
        unsafe {
            let d = ty_dict_new(KEY_STR as i64, 8, 8, 1);
            write_i64(ty_dict_insert(d, str_key("typhoon")), 1);
            let other = str_key("typhoon");
            let slot = ty_dict_insert(d, other);
            write_i64(slot, read_i64(slot) + 1);
            assert_eq!((*d).len, 1);
            assert_eq!(read_i64(ty_dict_get(d, str_key("typhoon"))), 2);
            assert!(ty_dict_find(d, str_key("typhoo")).is_null());
            assert!(ty_dict_find(d, str_key("")).is_null());
            for i in 0..1000 {
                ty_dict_insert(d, ty_str_from_int(i) as usize as i64);
            }
            assert_eq!((*d).len, 1001);
            assert!(!ty_dict_find(d, str_key("999")).is_null());
        }
    }

    #[test]
    fn the_cache_answers_a_miss_then_inserts_at_the_remembered_slot() {
        unsafe {
            let d = ty_dict_new(KEY_STR as i64, 8, 8, 1);
            for i in 0..100 {
                let k = ty_str_from_int(i) as usize as i64;
                assert!(ty_dict_find(d, k).is_null());
                write_i64(ty_dict_insert(d, k), i);
                assert_eq!(read_i64(ty_dict_get(d, k)), i);
            }
            for i in 0..100 {
                let k = ty_str_from_int(i) as usize as i64;
                assert_eq!(read_i64(ty_dict_get(d, k)), i);
            }
        }
    }

    #[test]
    fn tuple_keys_go_through_the_descriptor() {
        unsafe {
            let desc = (KEY_STR | KEY_INT << 4) as i64;
            let d = ty_dict_new(desc, 1, 1, 1);
            let mut buf = [0u8; 16];
            for (name, n) in [("a", 1i64), ("b", 2), ("a", 2)] {
                write_ptr(buf.as_mut_ptr(), str_new(name.as_bytes()));
                write_i64(buf.as_mut_ptr().add(8), n);
                *ty_dict_insert(d, buf.as_ptr() as usize as i64) = 1;
            }
            assert_eq!((*d).len, 3);
            write_ptr(buf.as_mut_ptr(), str_new(b"a"));
            write_i64(buf.as_mut_ptr().add(8), 2);
            assert!(!ty_dict_find(d, buf.as_ptr() as usize as i64).is_null());
            write_i64(buf.as_mut_ptr().add(8), 3);
            assert!(ty_dict_find(d, buf.as_ptr() as usize as i64).is_null());
            // items(): { str, int } then the bool value at 16, 24 bytes each.
            let items = ty_dict_items(d);
            assert_eq!((*items).len, 3);
        }
    }

    #[test]
    fn sets_have_no_value_bytes() {
        unsafe {
            let d = ty_dict_new(KEY_BOOL as i64, 0, 1, 1);
            ty_dict_insert(d, 1);
            ty_dict_insert(d, 0);
            ty_dict_insert(d, 1);
            assert_eq!((*d).len, 2);
            assert!(!ty_dict_remove(d, 0).is_null());
            assert!(ty_dict_remove(d, 0).is_null());
            assert_eq!((*d).len, 1);
            assert_eq!((*d).entry_size, 16);
        }
    }

    #[test]
    fn the_hash_spreads_short_keys() {
        let mut seen = std::collections::HashSet::new();
        for a in b"ACGT" {
            for b in b"ACGT" {
                for c in b"ACGT" {
                    seen.insert(hash_bytes(&[*a, *b, *c]) & 0xFF);
                }
            }
        }
        // 64 keys into 256 buckets: a weak hash collapses them.
        assert!(seen.len() > 40, "{}", seen.len());
        assert_ne!(hash_bytes(b""), hash_bytes(b"\0"));
        assert_ne!(hash_bytes(&[0; 17]), hash_bytes(&[0; 18]));
        assert_ne!(hash_int(1), hash_int(2));
    }
}
