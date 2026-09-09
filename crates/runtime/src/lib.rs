//! `typhoon-runtime` — the C-ABI runtime linked into every Typhoon binary.
//!
//! See `docs/DESIGN.md` §5 (memory management) and §6.1 (crate layout).
//!
//! The crate is built as a `staticlib` (`libtyphoon_runtime.a`). Generated LLVM
//! IR declares the `ty_*` symbols below and `clang` links the archive together
//! with Boehm GC (`-lgc`) into the final executable:
//!
//! ```sh
//! clang -O2 out.ll libtyphoon_runtime.a -lgc -o out
//! ```
//!
//! # Calling contract
//!
//! * [`ty_rt_init`] must be called once, first, from `main`.
//! * `print(a, b)` is lowered to `print_a` / [`ty_print_sep`] / `print_b` /
//!   [`ty_print_end`] (DESIGN §4.6: single space separator, trailing newline);
//!   a `str` inside a container is printed with [`ty_print_str_repr`] instead.
//! * a `str` value is a pointer to `{ byte_len: i64, char_len: i64, bytes }`
//!   and a `list<T>` value a pointer to a [`TyList`]; both layouts are part of
//!   the ABI and are documented where they are defined.
//! * stdout is buffered process-wide; it is flushed by [`ty_rt_exit`],
//!   [`ty_panic`] and by an `atexit` hook installed by [`ty_rt_init`], so a
//!   plain `return 0` from `main` still produces complete output.

use std::ffi::{c_int, c_void};
use std::io::{self, BufWriter, Stdout, Write};
use std::sync::{Mutex, MutexGuard, Once, OnceLock};

// Boehm-Demers-Weiser GC. Declared only: the archive does not link it, the
// final `clang` invocation does (`-lgc`).
unsafe extern "C" {
    fn GC_init();
    // Unused in `cfg(test)`, where allocation goes through Rust's allocator;
    // see `raw_alloc`.
    #[cfg_attr(test, allow(dead_code))]
    fn GC_malloc(n: usize) -> *mut c_void;
    #[cfg_attr(test, allow(dead_code))]
    fn GC_malloc_atomic(n: usize) -> *mut c_void;
    fn atexit(cb: extern "C" fn()) -> c_int;
}

/// Exit code used for every runtime panic (matches Rust's own convention).
pub const PANIC_EXIT_CODE: i32 = 101;

// ---------------------------------------------------------------------------
// Buffered stdout
// ---------------------------------------------------------------------------

static OUT: OnceLock<Mutex<BufWriter<Stdout>>> = OnceLock::new();

/// Locks the process-global buffered stdout, recovering from poisoning.
fn out() -> MutexGuard<'static, BufWriter<Stdout>> {
    OUT.get_or_init(|| Mutex::new(BufWriter::with_capacity(64 * 1024, io::stdout())))
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
}

/// Flushes the buffered stdout, ignoring errors (nothing sensible to do).
fn flush_out() {
    let _ = out().flush();
}

extern "C" fn flush_at_exit() {
    flush_out();
}

// ---------------------------------------------------------------------------
// Lifecycle
// ---------------------------------------------------------------------------

/// Initialises the runtime: starts Boehm GC and installs the stdout flush hook.
///
/// Must be the first runtime call made by a Typhoon program. Calling it more
/// than once, or from several threads at once, is harmless: the work happens
/// exactly once (Boehm's `GC_init` is not itself reentrant).
#[unsafe(no_mangle)]
pub extern "C" fn ty_rt_init() {
    static INIT: Once = Once::new();
    INIT.call_once(|| unsafe {
        GC_init();
        atexit(flush_at_exit);
    });
}

/// Flushes stdout and terminates the process with `code`.
#[unsafe(no_mangle)]
pub extern "C" fn ty_rt_exit(code: i32) -> ! {
    flush_out();
    std::process::exit(code)
}

// ---------------------------------------------------------------------------
// Allocation
// ---------------------------------------------------------------------------

/// The raw block allocator behind [`ty_alloc`] and [`ty_alloc_atomic`].
///
/// In a real Typhoon binary this is Boehm GC. Under `cargo test` it is Rust's
/// allocator instead, because `cargo test` runs every test on its own thread
/// and those threads are not registered with the GC — a Typhoon program is
/// single-threaded, so the runtime never calls `GC_register_my_thread`, and
/// allocating from an unregistered thread makes Boehm collect live objects and
/// abort in `thread_suspend`. The GC path itself is covered end to end by the
/// golden suite (`tests/run/gc_stress.ty` in particular), which runs real
/// compiled programs.
#[cfg(not(test))]
fn raw_alloc(size: usize, atomic: bool) -> *mut u8 {
    unsafe {
        if atomic {
            GC_malloc_atomic(size) as *mut u8
        } else {
            GC_malloc(size) as *mut u8
        }
    }
}

#[cfg(test)]
fn raw_alloc(size: usize, _atomic: bool) -> *mut u8 {
    // Leaked on purpose: a test process is short lived, and a Typhoon value
    // is never freed explicitly.
    let layout = std::alloc::Layout::from_size_align(size.max(1), 8).expect("a valid layout");
    unsafe { std::alloc::alloc(layout) }
}

/// Allocates `size` GC-traced bytes. The block *is* scanned for pointers.
///
/// Aborts the process with `panic: out of memory` when the GC cannot satisfy
/// the request. Never returns null.
#[unsafe(no_mangle)]
pub extern "C" fn ty_alloc(size: usize) -> *mut u8 {
    let p = raw_alloc(size, false);
    if p.is_null() {
        out_of_memory();
    }
    p
}

/// Allocates `size` GC-managed bytes that are known to contain no pointers
/// (string bytes, `list<int>` payloads, …). The block is *not* scanned.
///
/// Aborts the process with `panic: out of memory` when the GC cannot satisfy
/// the request. Never returns null.
#[unsafe(no_mangle)]
pub extern "C" fn ty_alloc_atomic(size: usize) -> *mut u8 {
    let p = raw_alloc(size, true);
    if p.is_null() {
        out_of_memory();
    }
    p
}

fn out_of_memory() -> ! {
    let msg = b"out of memory";
    unsafe { ty_panic(msg.as_ptr(), msg.len()) }
}

// ---------------------------------------------------------------------------
// Panics
// ---------------------------------------------------------------------------

/// Aborts the program with `panic: <msg>` on stderr and exit code 101.
///
/// # Safety
///
/// `msg_ptr` must point to `msg_len` readable bytes (UTF-8 is expected but not
/// required; the bytes are written verbatim).
#[unsafe(no_mangle)]
pub unsafe extern "C" fn ty_panic(msg_ptr: *const u8, msg_len: usize) -> ! {
    flush_out();
    let msg = unsafe { bytes(msg_ptr, msg_len) };
    let mut err = io::stderr().lock();
    let _ = write_panic(&mut err, msg);
    let _ = err.flush();
    std::process::exit(PANIC_EXIT_CODE)
}

/// Renders a panic message: `panic: <msg>\n`.
pub fn write_panic(w: &mut impl Write, msg: &[u8]) -> io::Result<()> {
    w.write_all(b"panic: ")?;
    w.write_all(msg)?;
    w.write_all(b"\n")
}

// ---------------------------------------------------------------------------
// print
// ---------------------------------------------------------------------------

/// Writes an `int` in decimal (DESIGN §4.6).
pub fn write_int(w: &mut impl Write, value: i64) -> io::Result<()> {
    let mut buf = itoa(value);
    w.write_all(buf.as_bytes_mut())
}

/// Writes a `float` using Python's `repr(float)` spelling (DESIGN §4.6).
pub fn write_float(w: &mut impl Write, value: f64) -> io::Result<()> {
    w.write_all(format_float(value).as_bytes())
}

/// Writes a `bool` as `True` / `False` (DESIGN §4.6).
pub fn write_bool(w: &mut impl Write, value: u8) -> io::Result<()> {
    w.write_all(if value != 0 {
        b"True" as &[u8]
    } else {
        b"False"
    })
}

/// Writes a `str` verbatim, without quotes (DESIGN §4.6).
pub fn write_str(w: &mut impl Write, value: &[u8]) -> io::Result<()> {
    w.write_all(value)
}

/// Writes `None` (DESIGN §4.6).
pub fn write_none(w: &mut impl Write) -> io::Result<()> {
    w.write_all(b"None")
}

/// Writes the separator between two `print` arguments: a single space.
pub fn write_sep(w: &mut impl Write) -> io::Result<()> {
    w.write_all(b" ")
}

/// Writes the terminator of a `print` call: a newline.
pub fn write_end(w: &mut impl Write) -> io::Result<()> {
    w.write_all(b"\n")
}

/// Prints an `int`.
#[unsafe(no_mangle)]
pub extern "C" fn ty_print_int(value: i64) {
    let _ = write_int(&mut *out(), value);
}

/// Prints a `float` in Python `repr` form.
#[unsafe(no_mangle)]
pub extern "C" fn ty_print_float(value: f64) {
    let _ = write_float(&mut *out(), value);
}

/// Prints a `bool` as `True` / `False`.
#[unsafe(no_mangle)]
pub extern "C" fn ty_print_bool(value: u8) {
    let _ = write_bool(&mut *out(), value);
}

/// Prints a `str` given as pointer + byte length.
///
/// # Safety
///
/// `ptr` must point to `len` readable bytes.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn ty_print_str(ptr: *const u8, len: usize) {
    let s = unsafe { bytes(ptr, len) };
    let _ = write_str(&mut *out(), s);
}

/// Prints `None`.
#[unsafe(no_mangle)]
pub extern "C" fn ty_print_none() {
    let _ = write_none(&mut *out());
}

/// Prints the single-space separator between `print` arguments.
#[unsafe(no_mangle)]
pub extern "C" fn ty_print_sep() {
    let _ = write_sep(&mut *out());
}

/// Prints the trailing newline of a `print` call.
#[unsafe(no_mangle)]
pub extern "C" fn ty_print_end() {
    let _ = write_end(&mut *out());
}

/// Reads `len` bytes at `ptr` as a slice, tolerating a null pointer when
/// `len == 0`.
///
/// # Safety
///
/// `ptr` must point to `len` readable bytes (or `len` must be zero).
unsafe fn bytes<'a>(ptr: *const u8, len: usize) -> &'a [u8] {
    if len == 0 {
        return &[];
    }
    debug_assert!(!ptr.is_null());
    unsafe { std::slice::from_raw_parts(ptr, len) }
}

// ---------------------------------------------------------------------------
// Integer formatting
// ---------------------------------------------------------------------------

/// A tiny stack buffer holding the decimal spelling of an `i64`.
struct IntBuf {
    buf: [u8; 20],
    len: usize,
}

impl IntBuf {
    fn as_bytes_mut(&mut self) -> &[u8] {
        &self.buf[..self.len]
    }
}

/// Formats `value` in decimal without allocating.
fn itoa(value: i64) -> IntBuf {
    let mut tmp = [0u8; 20];
    let mut n = 0usize;
    let negative = value < 0;
    // Use the unsigned magnitude so that i64::MIN is handled correctly.
    let mut mag = value.unsigned_abs();
    loop {
        tmp[n] = b'0' + (mag % 10) as u8;
        n += 1;
        mag /= 10;
        if mag == 0 {
            break;
        }
    }
    let mut buf = [0u8; 20];
    let mut len = 0usize;
    if negative {
        buf[0] = b'-';
        len = 1;
    }
    while n > 0 {
        n -= 1;
        buf[len] = tmp[n];
        len += 1;
    }
    IntBuf { buf, len }
}

// ---------------------------------------------------------------------------
// Float formatting (Python `repr(float)`)
// ---------------------------------------------------------------------------

/// Splits a Rust `LowerExp` rendering (`d[.ddd]e<exp>`) into mantissa and
/// decimal exponent.
fn split_exp(repr: &str) -> (&str, i32) {
    let (mantissa, exp) = repr.split_once('e').expect("LowerExp always emits `e`");
    let exp = exp
        .parse()
        .expect("LowerExp always emits an integer exponent");
    (mantissa, exp)
}

/// Formats `value` exactly the way CPython's `repr(float)` does.
///
/// The rules (DESIGN §4.6) are:
///
/// * shortest representation that round-trips through `f64`;
/// * fixed notation while `-4 < decpt <= 16`, where `decpt` is the position of
///   the decimal point (`value = 0.<digits> * 10^decpt`), otherwise exponent
///   notation with a signed, at-least-two-digit exponent (`1e+16`, `1e-05`);
/// * the result always contains a `.` or an `e`, so `5.0` never prints as `5`;
/// * `inf`, `-inf`, `nan`, and a signed `-0.0`.
///
/// ```
/// # use typhoon_runtime::format_float;
/// assert_eq!(format_float(5.0), "5.0");
/// assert_eq!(format_float(1e16), "1e+16");
/// assert_eq!(format_float(1e-5), "1e-05");
/// ```
pub fn format_float(value: f64) -> String {
    if value.is_nan() {
        return "nan".to_string();
    }
    if value.is_infinite() {
        return if value < 0.0 { "-inf" } else { "inf" }.to_string();
    }

    let negative = value.is_sign_negative();
    let sign = if negative { "-" } else { "" };
    if value == 0.0 {
        return format!("{sign}0.0");
    }

    // Rust's `LowerExp` for f64 emits the shortest round-trip digit string as
    // `d[.ddd]e<exp>`. It agrees with CPython's `dtoa` everywhere except on an
    // exact tie, where two equally-short, equally-distant decimals both
    // round-trip: Rust rounds half away from zero, CPython rounds half to even
    // (`repr(-2227059643032627.25)` is `-2227059643032627.2`, not `...3`).
    //
    // A tie can only be resolved differently when Rust's last digit came out
    // odd, so in that case we re-render the same number of significant digits
    // with `{:.*e}`, which *does* round half to even, and use that instead.
    let repr = format!("{:e}", value.abs());
    let (mantissa, exp) = split_exp(&repr);
    let significant = mantissa.len() - usize::from(mantissa.contains('.'));
    let owned;
    let (mantissa, exp) = if mantissa.as_bytes()[mantissa.len() - 1] % 2 == 1 {
        owned = format!("{:.*e}", significant - 1, value.abs());
        split_exp(&owned)
    } else {
        (mantissa, exp)
    };

    let digits: String = mantissa.chars().filter(|c| *c != '.').collect();
    let digits = digits.trim_end_matches('0');
    let digits = if digits.is_empty() { "0" } else { digits };

    // `decpt` counts the digits that belong in front of the decimal point.
    let decpt = exp + 1;

    if decpt <= -4 || decpt > 16 {
        // Exponent notation. CPython does not pad the mantissa with `.0` here.
        let e = decpt - 1;
        let (esign, emag) = if e < 0 { ("-", -e) } else { ("+", e) };
        let mut s = String::with_capacity(digits.len() + 8);
        s.push_str(sign);
        s.push_str(&digits[..1]);
        if digits.len() > 1 {
            s.push('.');
            s.push_str(&digits[1..]);
        }
        s.push('e');
        s.push_str(esign);
        if emag < 10 {
            s.push('0');
        }
        s.push_str(&emag.to_string());
        return s;
    }

    let mut s = String::with_capacity(digits.len() + 20);
    s.push_str(sign);
    if decpt <= 0 {
        s.push_str("0.");
        for _ in 0..-decpt {
            s.push('0');
        }
        s.push_str(digits);
    } else {
        let decpt = decpt as usize;
        if decpt >= digits.len() {
            s.push_str(digits);
            for _ in 0..(decpt - digits.len()) {
                s.push('0');
            }
            s.push_str(".0");
        } else {
            s.push_str(&digits[..decpt]);
            s.push('.');
            s.push_str(&digits[decpt..]);
        }
    }
    s
}

// ---------------------------------------------------------------------------
// Strings
// ---------------------------------------------------------------------------

/// A `str` value: a pointer to `{ byte_len: i64, char_len: i64, bytes: [u8;
/// byte_len] }` on the GC heap.
///
/// The header is what the compiler passes around; string literals are emitted
/// as private LLVM constants with exactly this layout, so a literal costs no
/// allocation at all. The payload holds no pointers, so it is allocated with
/// [`ty_alloc_atomic`] and never scanned.
///
/// Both lengths are stored as `i64` to match the `int` type and to keep the
/// bytes 8-byte aligned. `char_len` is the number of Unicode code points, i.e.
/// exactly what `len(s)` returns (DESIGN §4.1: `str` is an immutable UTF-8
/// value with Python semantics); caching it keeps `len` O(1) while every
/// internal operation still works on bytes.
const STR_HEADER: usize = 16;

/// Byte offset of the `char_len` field inside a string header.
const STR_CHAR_LEN_OFFSET: usize = 8;

/// The number of Unicode code points in well-formed UTF-8 `bytes`.
///
/// Every code point starts at a byte that is *not* a continuation byte
/// (`0b10xx_xxxx`), so one branch-free pass over the payload is enough.
///
/// ```
/// # use typhoon_runtime::char_count;
/// assert_eq!(char_count("台风".as_bytes()), 2);
/// ```
pub fn char_count(bytes: &[u8]) -> i64 {
    bytes.iter().filter(|b| (*b & 0xC0) != 0x80).count() as i64
}

/// The length in bytes of the UTF-8 sequence introduced by the lead byte
/// `lead` (1..=4).
///
/// Continuation bytes and the invalid lead bytes `0xF8..=0xFF` report `1`, so
/// that a malformed string still advances one byte at a time instead of
/// looping forever or running off the end.
pub fn utf8_seq_len(lead: u8) -> usize {
    match lead {
        0x00..=0xBF => 1,
        0xC0..=0xDF => 2,
        0xE0..=0xEF => 3,
        0xF0..=0xF7 => 4,
        _ => 1,
    }
}

/// Allocates an uninitialised string of `byte_len` bytes whose header already
/// records `char_len` code points, and returns the header.
///
/// # Safety
///
/// The caller must initialise all `byte_len` payload bytes before the value is
/// handed to any other function, and `char_len` must be the true code point
/// count of those bytes.
unsafe fn str_alloc(byte_len: usize, char_len: i64) -> *mut u8 {
    let p = ty_alloc_atomic(STR_HEADER + byte_len);
    unsafe {
        std::ptr::write_unaligned(p as *mut i64, byte_len as i64);
        std::ptr::write_unaligned(p.add(STR_CHAR_LEN_OFFSET) as *mut i64, char_len);
    }
    p
}

/// Copies `bytes` into a fresh GC-allocated string, counting its code points.
pub fn str_new(bytes: &[u8]) -> *mut u8 {
    str_new_with_char_len(bytes, char_count(bytes))
}

/// Copies `bytes` into a fresh GC-allocated string whose code point count is
/// already known — an ASCII rendering, a concatenation, a case mapping.
///
/// `char_len` must be the true code point count of `bytes`; anything else
/// makes `len(s)` lie.
pub fn str_new_with_char_len(bytes: &[u8], char_len: i64) -> *mut u8 {
    unsafe {
        let p = str_alloc(bytes.len(), char_len);
        std::ptr::copy_nonoverlapping(bytes.as_ptr(), p.add(STR_HEADER), bytes.len());
        p
    }
}

/// Copies pure-ASCII `bytes` — the rendering of an `int`, a `float` or a
/// `bool` — into a fresh string, where one byte is always one code point.
fn ascii_str_new(bytes: &[u8]) -> *mut u8 {
    debug_assert!(bytes.is_ascii());
    str_new_with_char_len(bytes, bytes.len() as i64)
}

/// The payload of a string value.
///
/// # Safety
///
/// `s` must point at a well-formed string header.
pub unsafe fn str_bytes<'a>(s: *const u8) -> &'a [u8] {
    unsafe {
        let len = std::ptr::read_unaligned(s as *const i64) as usize;
        bytes(s.add(STR_HEADER), len)
    }
}

/// The number of Unicode code points in a string value: `len(s)`, in O(1).
///
/// # Safety
///
/// `s` must point at a well-formed string header.
pub unsafe fn str_char_len(s: *const u8) -> i64 {
    unsafe { std::ptr::read_unaligned(s.add(STR_CHAR_LEN_OFFSET) as *const i64) }
}

/// A statically allocated one-byte string, laid out exactly like a heap one:
/// `byte_len`, `char_len`, the payload byte, and padding back up to the
/// 8-byte alignment every string header has.
#[repr(C, align(8))]
#[derive(Clone, Copy)]
struct AsciiStr {
    byte_len: i64,
    char_len: i64,
    byte: u8,
    _padding: [u8; 7],
}

/// The static string holding the single byte `byte`.
const fn ascii_str(byte: u8) -> AsciiStr {
    AsciiStr {
        byte_len: 1,
        char_len: 1,
        byte,
        _padding: [0; 7],
    }
}

// The layout of an entry has to match a heap string exactly, because
// `ty_str_char_at` hands entries out as ordinary `str` values. This also reads
// the fields, which are otherwise only ever observed through a raw pointer.
const _: () = {
    let entry = ascii_str(b'A');
    assert!(entry.byte_len == 1 && entry.char_len == 1 && entry.byte == b'A');
    assert!(size_of::<AsciiStr>() == STR_HEADER + 8);
    assert!(align_of::<AsciiStr>() == 8);
};

/// The 128 one-byte ASCII strings, so that iterating over ASCII text
/// ([`ty_str_char_at`]) allocates nothing at all.
static ASCII_STRS: [AsciiStr; 128] = {
    let mut table = [ascii_str(0); 128];
    let mut i = 0usize;
    while i < 128 {
        table[i] = ascii_str(i as u8);
        i += 1;
    }
    table
};

/// The static one-byte `str` for the ASCII byte `byte` (which must be < 0x80).
fn ascii_str_value(byte: u8) -> *const u8 {
    debug_assert!(byte < 0x80);
    &raw const ASCII_STRS[byte as usize] as *const u8
}

/// Concatenates two strings into a new one (`str` is immutable).
///
/// # Safety
///
/// Both arguments must point at well-formed string headers.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn ty_str_concat(a: *const u8, b: *const u8) -> *mut u8 {
    unsafe {
        let char_len = str_char_len(a) + str_char_len(b);
        let (a, b) = (str_bytes(a), str_bytes(b));
        let out = str_alloc(a.len() + b.len(), char_len);
        std::ptr::copy_nonoverlapping(a.as_ptr(), out.add(STR_HEADER), a.len());
        std::ptr::copy_nonoverlapping(b.as_ptr(), out.add(STR_HEADER + a.len()), b.len());
        out
    }
}

/// Byte-wise string equality, `1` for equal.
///
/// # Safety
///
/// Both arguments must point at well-formed string headers.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn ty_str_eq(a: *const u8, b: *const u8) -> u8 {
    unsafe { u8::from(str_bytes(a) == str_bytes(b)) }
}

/// Renders an `int` the way `print` does.
#[unsafe(no_mangle)]
pub extern "C" fn ty_str_from_int(value: i64) -> *mut u8 {
    let mut buf = itoa(value);
    ascii_str_new(buf.as_bytes_mut())
}

/// Renders a `float` the way `print` does (shortest round-trip, always a `.`).
#[unsafe(no_mangle)]
pub extern "C" fn ty_str_from_float(value: f64) -> *mut u8 {
    ascii_str_new(format_float(value).as_bytes())
}

/// Renders a `bool` as `True` / `False`.
#[unsafe(no_mangle)]
pub extern "C" fn ty_str_from_bool(value: u8) -> *mut u8 {
    ascii_str_new(if value != 0 { b"True" } else { b"False" })
}

/// Renders a `float` with exactly `precision` digits after the point, the
/// `{x:.Nf}` format spec of DESIGN section 3.7.
///
/// Non-finite values ignore the precision and render as `inf`, `-inf`, `nan`,
/// which is what Python's `format` does.
#[unsafe(no_mangle)]
pub extern "C" fn ty_str_from_float_fixed(value: f64, precision: i64) -> *mut u8 {
    ascii_str_new(format_fixed(value, precision).as_bytes())
}

/// The text of `{value:.<precision>f}`.
pub fn format_fixed(value: f64, precision: i64) -> String {
    if !value.is_finite() {
        return format_float(value);
    }
    let precision = precision.clamp(0, 32) as usize;
    format!("{value:.precision$}")
}

// ---------------------------------------------------------------------------
// String queries
// ---------------------------------------------------------------------------

/// Byte-wise lexicographic comparison: `-1`, `0` or `1`.
///
/// UTF-8 orders code points and their encodings the same way, so this is also
/// the code point order Python compares strings by.
pub fn str_cmp(a: &[u8], b: &[u8]) -> i64 {
    match a.cmp(b) {
        std::cmp::Ordering::Less => -1,
        std::cmp::Ordering::Equal => 0,
        std::cmp::Ordering::Greater => 1,
    }
}

/// The byte offset of the first occurrence of `needle` in `haystack`.
///
/// An empty needle occurs at offset `0`, as in Python.
pub fn str_find_bytes(haystack: &[u8], needle: &[u8]) -> Option<usize> {
    if needle.is_empty() {
        return Some(0);
    }
    haystack.windows(needle.len()).position(|w| w == needle)
}

/// `haystack.find(needle)`: the index of the first occurrence **in code
/// points**, or `-1` when there is none.
pub fn str_find_chars(haystack: &[u8], needle: &[u8]) -> i64 {
    match str_find_bytes(haystack, needle) {
        Some(offset) => char_count(&haystack[..offset]),
        None => -1,
    }
}

/// Whether `b` is one of the ASCII characters Python's `str.strip()` removes:
/// space, `\t`, `\n`, `\r`, `\x0b`, `\x0c`.
///
/// This is deliberately *not* `u8::is_ascii_whitespace`, which leaves `\x0b`
/// in place.
pub fn is_ascii_space(b: u8) -> bool {
    matches!(b, b' ' | b'\t' | b'\n' | b'\r' | 0x0b | 0x0c)
}

/// `s.strip()`: `bytes` without the ASCII whitespace at either end.
pub fn str_strip(bytes: &[u8]) -> &[u8] {
    let mut start = 0usize;
    let mut end = bytes.len();
    while start < end && is_ascii_space(bytes[start]) {
        start += 1;
    }
    while end > start && is_ascii_space(bytes[end - 1]) {
        end -= 1;
    }
    &bytes[start..end]
}

/// Byte-wise lexicographic comparison of two strings: `-1`, `0` or `1`.
///
/// # Safety
///
/// Both arguments must point at well-formed string headers.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn ty_str_cmp(a: *const u8, b: *const u8) -> i64 {
    unsafe { str_cmp(str_bytes(a), str_bytes(b)) }
}

/// `1` when `needle` is a substring of `s`; an empty needle is contained in
/// every string.
///
/// # Safety
///
/// Both arguments must point at well-formed string headers.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn ty_str_contains(s: *const u8, needle: *const u8) -> u8 {
    unsafe { u8::from(str_find_bytes(str_bytes(s), str_bytes(needle)).is_some()) }
}

/// `1` when `s` starts with `prefix`.
///
/// # Safety
///
/// Both arguments must point at well-formed string headers.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn ty_str_startswith(s: *const u8, prefix: *const u8) -> u8 {
    unsafe { u8::from(str_bytes(s).starts_with(str_bytes(prefix))) }
}

/// `1` when `s` ends with `suffix`.
///
/// # Safety
///
/// Both arguments must point at well-formed string headers.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn ty_str_endswith(s: *const u8, suffix: *const u8) -> u8 {
    unsafe { u8::from(str_bytes(s).ends_with(str_bytes(suffix))) }
}

/// `s.find(needle)`: the code point index of the first occurrence, or `-1`.
///
/// # Safety
///
/// Both arguments must point at well-formed string headers.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn ty_str_find(s: *const u8, needle: *const u8) -> i64 {
    unsafe { str_find_chars(str_bytes(s), str_bytes(needle)) }
}

// ---------------------------------------------------------------------------
// String transforms (`str` is immutable: every one returns a new string)
// ---------------------------------------------------------------------------

/// Copies `s` through a byte-wise map that preserves both lengths.
///
/// # Safety
///
/// `s` must point at a well-formed string header, and `map` must map every
/// byte to one that keeps the payload's UTF-8 structure intact — which, for
/// the ASCII case mappings, means leaving every byte >= 0x80 alone.
unsafe fn str_map(s: *const u8, map: fn(&u8) -> u8) -> *mut u8 {
    unsafe {
        let src = str_bytes(s);
        let out = str_alloc(src.len(), str_char_len(s));
        let dst = std::slice::from_raw_parts_mut(out.add(STR_HEADER), src.len());
        for (d, b) in dst.iter_mut().zip(src) {
            *d = map(b);
        }
        out
    }
}

/// `s.upper()`, ASCII-only: `a`..`z` are upper-cased, every other byte —
/// including all of UTF-8's multi-byte sequences — passes through unchanged.
///
/// # Safety
///
/// `s` must point at a well-formed string header.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn ty_str_upper(s: *const u8) -> *mut u8 {
    unsafe { str_map(s, u8::to_ascii_uppercase) }
}

/// `s.lower()`, ASCII-only: `A`..`Z` are lower-cased, every other byte passes
/// through unchanged.
///
/// # Safety
///
/// `s` must point at a well-formed string header.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn ty_str_lower(s: *const u8) -> *mut u8 {
    unsafe { str_map(s, u8::to_ascii_lowercase) }
}

/// `s.strip()`: a new string without the ASCII whitespace at either end.
///
/// # Safety
///
/// `s` must point at a well-formed string header.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn ty_str_strip(s: *const u8) -> *mut u8 {
    unsafe {
        let src = str_bytes(s);
        let kept = str_strip(src);
        // Only ASCII bytes are stripped, and each of them is one code point.
        let char_len = str_char_len(s) - (src.len() - kept.len()) as i64;
        str_new_with_char_len(kept, char_len)
    }
}

/// `s.replace(old, new)`: every non-overlapping occurrence, left to right.
///
/// An empty `old` matches before every code point and once at the end, the way
/// Python does it: `"ab".replace("", "-") == "-a-b-"`.
pub fn str_replace(s: &[u8], old: &[u8], new: &[u8]) -> Vec<u8> {
    if old.is_empty() {
        let slots = char_count(s) as usize + 1;
        let mut out = Vec::with_capacity(s.len() + new.len() * slots);
        out.extend_from_slice(new);
        let mut at = 0usize;
        while at < s.len() {
            let n = utf8_seq_len(s[at]).min(s.len() - at);
            out.extend_from_slice(&s[at..at + n]);
            out.extend_from_slice(new);
            at += n;
        }
        return out;
    }
    let mut out = Vec::with_capacity(s.len());
    let mut rest = s;
    while let Some(at) = str_find_bytes(rest, old) {
        out.extend_from_slice(&rest[..at]);
        out.extend_from_slice(new);
        rest = &rest[at + old.len()..];
    }
    out.extend_from_slice(rest);
    out
}

/// `s.replace(old, new)` (DESIGN §4.6), Python semantics.
///
/// # Safety
///
/// All three arguments must point at well-formed string headers.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn ty_str_replace(s: *const u8, old: *const u8, new: *const u8) -> *mut u8 {
    unsafe { str_new(&str_replace(str_bytes(s), str_bytes(old), str_bytes(new))) }
}

/// `s.split(sep)`, or `None` when `sep` is empty (which Python rejects).
///
/// The result always holds at least one part: `"".split(",") == [""]`, and
/// every run between two separators becomes a part of its own, so
/// `"a,,b".split(",") == ["a", "", "b"]`.
pub fn str_split_checked<'a>(s: &'a [u8], sep: &[u8]) -> Option<Vec<&'a [u8]>> {
    if sep.is_empty() {
        return None;
    }
    let mut parts = Vec::new();
    let mut rest = s;
    while let Some(at) = str_find_bytes(rest, sep) {
        parts.push(&rest[..at]);
        rest = &rest[at + sep.len()..];
    }
    parts.push(rest);
    Some(parts)
}

/// `sep.join(parts)`: the parts in order, with `sep` between neighbours.
pub fn str_join(sep: &[u8], parts: &[&[u8]]) -> Vec<u8> {
    let total: usize =
        parts.iter().map(|p| p.len()).sum::<usize>() + sep.len() * parts.len().saturating_sub(1);
    let mut out = Vec::with_capacity(total);
    for (i, part) in parts.iter().enumerate() {
        if i > 0 {
            out.extend_from_slice(sep);
        }
        out.extend_from_slice(part);
    }
    out
}

/// `s.split(sep)` as a `list<str>` (DESIGN §4.6).
///
/// Panics with `empty separator` when `sep` is empty.
///
/// # Safety
///
/// Both arguments must point at well-formed string headers.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn ty_str_split(s: *const u8, sep: *const u8) -> *mut TyList {
    unsafe {
        let Some(parts) = str_split_checked(str_bytes(s), str_bytes(sep)) else {
            let msg = b"empty separator";
            ty_panic(msg.as_ptr(), msg.len())
        };
        // A `list<str>` stores pointers, so its buffer has to be scanned.
        let list = ty_list_new(size_of::<*mut u8>() as i64, 0, parts.len() as i64);
        let data = (*list).data as *mut *mut u8;
        for (i, part) in parts.iter().enumerate() {
            std::ptr::write(data.add(i), str_new(part));
        }
        (*list).len = parts.len() as i64;
        list
    }
}

/// `sep.join(parts)` where `parts` is a `list<str>` (DESIGN §4.6).
///
/// # Safety
///
/// `sep` must point at a well-formed string header and `parts` at a well-formed
/// [`TyList`] whose elements are pointers to well-formed string headers.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn ty_str_join(sep: *const u8, parts: *const TyList) -> *mut u8 {
    unsafe {
        let count = (*parts).len.max(0) as usize;
        let data = (*parts).data as *const *const u8;
        let mut slices: Vec<&[u8]> = Vec::with_capacity(count);
        let mut char_len = 0i64;
        for i in 0..count {
            let part = *data.add(i);
            slices.push(str_bytes(part));
            char_len += str_char_len(part);
        }
        char_len += str_char_len(sep) * (count as i64 - 1).max(0);
        str_new_with_char_len(&str_join(str_bytes(sep), &slices), char_len)
    }
}

/// The one-code-point `str` starting at byte offset `byte_off` of `s`.
///
/// The sequence length comes from the lead byte and is clamped so that it
/// never runs past the end of the payload; the caller advances its own byte
/// offset by the `byte_len` of the returned string, which is how `for c in s`
/// walks a string. An ASCII code point is answered with a pointer into a
/// static table, so iterating over ASCII text allocates nothing.
///
/// # Safety
///
/// `s` must point at a well-formed string header. `byte_off` should be the
/// offset of a lead byte within it; an offset past the end yields an empty
/// string rather than reading out of bounds.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn ty_str_char_at(s: *const u8, byte_off: i64) -> *const u8 {
    unsafe {
        let src = str_bytes(s);
        let at = byte_off.max(0) as usize;
        if at >= src.len() {
            debug_assert!(false, "ty_str_char_at past the end of the string");
            return str_new_with_char_len(b"", 0);
        }
        let lead = src[at];
        if lead < 0x80 {
            return ascii_str_value(lead);
        }
        let len = utf8_seq_len(lead).min(src.len() - at);
        str_new_with_char_len(&src[at..at + len], 1)
    }
}

// ---------------------------------------------------------------------------
// Lists
// ---------------------------------------------------------------------------

/// The header of a `list<T>` value (DESIGN §4.1: a growable, contiguous,
/// mutable reference type).
///
/// The compiler passes a pointer to this 24-byte header around and reads
/// `len` / `data` inline for indexing; the header itself is allocated with
/// [`ty_alloc`], because `data` is a pointer the collector must be able to
/// see. The element buffer holds `cap * elem_size` bytes and is allocated with
/// [`ty_alloc_atomic`] whenever the element type contains no pointers.
///
/// `elem_size` and the atomicity are not stored: the compiler knows the
/// element type statically and passes both to every runtime entry point that
/// needs them.
#[repr(C)]
pub struct TyList {
    /// The number of elements currently stored in `data`.
    pub len: i64,
    /// The number of elements `data` has room for.
    pub cap: i64,
    /// The element buffer, or null while `cap` is zero.
    pub data: *mut u8,
}

/// Allocates a `cap * elem_size` byte element buffer, scanned unless `atomic`.
fn list_alloc_data(cap: i64, elem_size: i64, atomic: u8) -> *mut u8 {
    let size = (cap.max(0) as usize).saturating_mul(elem_size.max(0) as usize);
    if atomic != 0 {
        ty_alloc_atomic(size)
    } else {
        ty_alloc(size)
    }
}

/// A fresh empty `list<T>` with room for `cap` elements.
///
/// `cap == 0` allocates no element buffer at all and leaves `data` null, which
/// is what an empty list literal lowers to.
#[unsafe(no_mangle)]
pub extern "C" fn ty_list_new(elem_size: i64, atomic: u8, cap: i64) -> *mut TyList {
    let cap = cap.max(0);
    let data = if cap == 0 {
        std::ptr::null_mut()
    } else {
        list_alloc_data(cap, elem_size, atomic)
    };
    let list = ty_alloc(size_of::<TyList>()) as *mut TyList;
    unsafe { std::ptr::write(list, TyList { len: 0, cap, data }) };
    list
}

/// Makes sure `list` can hold `needed` elements, growing geometrically.
///
/// The new capacity is `max(4, cap * 2, needed)`, so pushing one element at a
/// time stays amortised O(1). Does nothing when the capacity already suffices.
///
/// # Safety
///
/// `list` must point at a well-formed [`TyList`] whose elements are
/// `elem_size` bytes wide.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn ty_list_grow(list: *mut TyList, elem_size: i64, atomic: u8, needed: i64) {
    unsafe {
        let cap = (*list).cap;
        if cap >= needed {
            return;
        }
        let new_cap = needed.max(4).max(cap.saturating_mul(2));
        let data = list_alloc_data(new_cap, elem_size, atomic);
        let len = (*list).len.max(0) as usize;
        let old = (*list).data;
        if !old.is_null() && len > 0 {
            std::ptr::copy_nonoverlapping(old, data, len * elem_size.max(0) as usize);
        }
        (*list).data = data;
        (*list).cap = new_cap;
    }
}

/// Opens a slot for `list.insert(index, value)` and returns a pointer to it.
///
/// Index handling is Python's: a negative index counts from the end and then
/// clamps to `0`, an index past the end appends. The tail moves right by one
/// element and `len` grows; the slot itself is left uninitialised, and the
/// caller must store an element there before anything else looks at the list.
///
/// # Safety
///
/// `list` must point at a well-formed [`TyList`] whose elements are
/// `elem_size` bytes wide.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn ty_list_insert_slot(
    list: *mut TyList,
    index: i64,
    elem_size: i64,
    atomic: u8,
) -> *mut u8 {
    unsafe {
        let len = (*list).len.max(0);
        let mut index = index;
        if index < 0 {
            index = (index + len).max(0);
        }
        if index > len {
            index = len;
        }
        ty_list_grow(list, elem_size, atomic, len + 1);
        let elem_size = elem_size.max(0) as usize;
        let data = (*list).data;
        let at = index as usize * elem_size;
        let tail = (len - index) as usize * elem_size;
        if tail > 0 {
            std::ptr::copy(data.add(at), data.add(at + elem_size), tail);
        }
        (*list).len = len + 1;
        data.add(at)
    }
}

/// `a + b`: a fresh list holding `a`'s elements followed by `b`'s.
///
/// # Safety
///
/// Both arguments must point at well-formed [`TyList`]s whose elements are
/// `elem_size` bytes wide.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn ty_list_concat(
    a: *const TyList,
    b: *const TyList,
    elem_size: i64,
    atomic: u8,
) -> *mut TyList {
    unsafe {
        let (a_len, b_len) = ((*a).len.max(0), (*b).len.max(0));
        let list = ty_list_new(elem_size, atomic, a_len + b_len);
        let size = elem_size.max(0) as usize;
        let data = (*list).data;
        if a_len > 0 {
            std::ptr::copy_nonoverlapping((*a).data, data, a_len as usize * size);
        }
        if b_len > 0 {
            std::ptr::copy_nonoverlapping(
                (*b).data,
                data.add(a_len as usize * size),
                b_len as usize * size,
            );
        }
        (*list).len = a_len + b_len;
        list
    }
}

// ---------------------------------------------------------------------------
// Container printing
// ---------------------------------------------------------------------------

/// Writes the `repr` form of a `str`, the one DESIGN §4.6 prescribes for a
/// `str` nested in a container (`["a", "b"]`).
///
/// Unlike Python, which prefers `'`, the quote is always `"`. Inside it, `\`
/// and `"` are backslash-escaped, `\n` / `\t` / `\r` take their short form,
/// every other byte below `0x20` plus `0x7f` becomes `\xNN` with lower-case
/// hex, and everything else — including every byte of a multi-byte UTF-8
/// sequence — is written verbatim.
pub fn write_str_repr(w: &mut impl Write, bytes: &[u8]) -> io::Result<()> {
    const HEX: &[u8; 16] = b"0123456789abcdef";
    w.write_all(b"\"")?;
    // Bytes that need no escape are written in runs rather than one at a time.
    let mut verbatim = 0usize;
    for (i, &b) in bytes.iter().enumerate() {
        let short: [u8; 2] = match b {
            b'\\' => *b"\\\\",
            b'"' => *b"\\\"",
            b'\n' => *b"\\n",
            b'\t' => *b"\\t",
            b'\r' => *b"\\r",
            0x00..=0x1f | 0x7f => {
                w.write_all(&bytes[verbatim..i])?;
                verbatim = i + 1;
                w.write_all(&[b'\\', b'x', HEX[(b >> 4) as usize], HEX[(b & 0xf) as usize]])?;
                continue;
            }
            _ => continue,
        };
        w.write_all(&bytes[verbatim..i])?;
        verbatim = i + 1;
        w.write_all(&short)?;
    }
    w.write_all(&bytes[verbatim..])?;
    w.write_all(b"\"")
}

/// Prints a `str` in its container form: quoted and escaped (DESIGN §4.6).
///
/// # Safety
///
/// `s` must point at a well-formed string header.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn ty_print_str_repr(s: *const u8) {
    let value = unsafe { str_bytes(s) };
    let _ = write_str_repr(&mut *out(), value);
}

// ---------------------------------------------------------------------------
// Integer power
// ---------------------------------------------------------------------------

/// `base ** exp` by squaring, wrapping like every other `int` operation
/// (DESIGN section 4.3); `None` for a negative exponent.
pub fn int_pow_checked(base: i64, exp: i64) -> Option<i64> {
    if exp < 0 {
        return None;
    }
    let mut base = base;
    let mut exp = exp;
    let mut acc: i64 = 1;
    while exp > 0 {
        if exp & 1 == 1 {
            acc = acc.wrapping_mul(base);
        }
        exp >>= 1;
        if exp > 0 {
            base = base.wrapping_mul(base);
        }
    }
    Some(acc)
}

/// `base ** exp` for two `int`s; panics on a negative exponent.
#[unsafe(no_mangle)]
pub extern "C" fn ty_int_pow(base: i64, exp: i64) -> i64 {
    match int_pow_checked(base, exp) {
        Some(value) => value,
        None => {
            let msg = b"negative exponent in integer power";
            unsafe { ty_panic(msg.as_ptr(), msg.len()) }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn rendered(f: impl FnOnce(&mut Vec<u8>) -> io::Result<()>) -> String {
        let mut buf = Vec::new();
        f(&mut buf).unwrap();
        String::from_utf8(buf).unwrap()
    }

    // -- format_float --------------------------------------------------

    #[test]
    fn float_repr_matches_python() {
        let cases: &[(f64, &str)] = &[
            (5.0, "5.0"),
            (3.5, "3.5"),
            (0.1, "0.1"),
            (1.0 / 3.0, "0.3333333333333333"),
            (1e15, "1000000000000000.0"),
            (1e16, "1e+16"),
            (1e-4, "0.0001"),
            (1e-5, "1e-05"),
            (2.5e-10, "2.5e-10"),
            (1.5e300, "1.5e+300"),
            (123456789.123, "123456789.123"),
            (0.0, "0.0"),
            (-0.0, "-0.0"),
            (100.0, "100.0"),
            (1e22, "1e+22"),
        ];
        for (value, expected) in cases {
            assert_eq!(format_float(*value), *expected, "repr({value:?})");
        }
    }

    #[test]
    fn float_repr_sqrt_two() {
        assert_eq!(format_float(2.0f64.powf(0.5)), "1.4142135623730951");
    }

    #[test]
    fn float_repr_non_finite() {
        assert_eq!(format_float(f64::INFINITY), "inf");
        assert_eq!(format_float(f64::NEG_INFINITY), "-inf");
        assert_eq!(format_float(f64::NAN), "nan");
        assert_eq!(format_float(-f64::NAN), "nan");
    }

    #[test]
    fn float_repr_negatives() {
        assert_eq!(format_float(-5.0), "-5.0");
        assert_eq!(format_float(-0.1), "-0.1");
        assert_eq!(format_float(-1e16), "-1e+16");
        assert_eq!(format_float(-1e-5), "-1e-05");
    }

    #[test]
    fn float_repr_boundaries() {
        // The fixed/exponent switch lives at decpt == 16.
        assert_eq!(format_float(9999999999999998.0), "9999999999999998.0");
        assert_eq!(format_float(1e17), "1e+17");
        assert_eq!(format_float(0.001), "0.001");
        assert_eq!(format_float(0.0001), "0.0001");
        assert_eq!(format_float(0.00001), "1e-05");
        assert_eq!(format_float(1.5e-5), "1.5e-05");
    }

    #[test]
    fn float_repr_breaks_exact_ties_to_even() {
        // The exact value is `...27.25`: both `...27.2` and `...27.3` are
        // 17 digits and round-trip. CPython picks the even last digit; Rust's
        // own shortest formatter would pick `...3`.
        assert_eq!(format_float(-2227059643032627.2), "-2227059643032627.2");
        assert_eq!(format_float(2160375749930557.2), "2160375749930557.2");
        assert_eq!(format_float(-246856750993115.62), "-246856750993115.62");
        assert_eq!(format_float(1181537280768007.2), "1181537280768007.2");
    }

    #[test]
    fn float_repr_extremes() {
        assert_eq!(format_float(f64::MIN_POSITIVE), "2.2250738585072014e-308");
        assert_eq!(format_float(5e-324), "5e-324");
        assert_eq!(format_float(f64::MAX), "1.7976931348623157e+308");
    }

    #[test]
    fn float_repr_always_has_dot_or_exponent() {
        let samples = [
            0.0, -0.0, 1.0, -1.0, 42.0, 1e15, 1e16, 1e-4, 1e-5, 2.5e-10, 1e300, 6.02e23,
        ];
        for v in samples {
            let s = format_float(v);
            assert!(s.contains('.') || s.contains('e'), "{v} formatted as {s}");
        }
    }

    #[test]
    fn float_repr_round_trips() {
        let samples = [
            0.1,
            1.0 / 3.0,
            2.0f64.sqrt(),
            std::f64::consts::PI,
            1e-5,
            1e16,
            1.5e300,
            5e-324,
            f64::MAX,
            f64::MIN_POSITIVE,
            -123456789.123,
        ];
        for v in samples {
            let s = format_float(v);
            let back: f64 = s.parse().unwrap();
            assert_eq!(back.to_bits(), v.to_bits(), "{s} did not round-trip");
        }
    }

    // -- write_* -------------------------------------------------------

    #[test]
    fn writes_ints() {
        assert_eq!(rendered(|w| write_int(w, 0)), "0");
        assert_eq!(rendered(|w| write_int(w, 7)), "7");
        assert_eq!(rendered(|w| write_int(w, -42)), "-42");
        assert_eq!(rendered(|w| write_int(w, i64::MAX)), "9223372036854775807");
        assert_eq!(rendered(|w| write_int(w, i64::MIN)), "-9223372036854775808");
    }

    #[test]
    fn writes_floats() {
        assert_eq!(rendered(|w| write_float(w, 5.0)), "5.0");
        assert_eq!(rendered(|w| write_float(w, -0.0)), "-0.0");
        assert_eq!(rendered(|w| write_float(w, 1e16)), "1e+16");
    }

    #[test]
    fn writes_bools() {
        assert_eq!(rendered(|w| write_bool(w, 1)), "True");
        assert_eq!(rendered(|w| write_bool(w, 0)), "False");
        // Any non-zero byte is truthy.
        assert_eq!(rendered(|w| write_bool(w, 255)), "True");
    }

    #[test]
    fn writes_strings_verbatim() {
        assert_eq!(
            rendered(|w| write_str(w, b"Hello, Typhoon!")),
            "Hello, Typhoon!"
        );
        assert_eq!(rendered(|w| write_str(w, b"")), "");
        assert_eq!(rendered(|w| write_str(w, "台风".as_bytes())), "台风");
        // No quoting, no escaping.
        assert_eq!(rendered(|w| write_str(w, b"a\"b\\c")), "a\"b\\c");
    }

    #[test]
    fn writes_none_sep_end() {
        assert_eq!(rendered(write_none), "None");
        assert_eq!(rendered(write_sep), " ");
        assert_eq!(rendered(write_end), "\n");
    }

    #[test]
    fn writes_panic_message() {
        assert_eq!(
            rendered(|w| write_panic(w, b"index out of range")),
            "panic: index out of range\n"
        );
    }

    #[test]
    fn print_lowering_of_two_arguments() {
        // `print(-42, 5.0)` lowers to int / sep / float / end.
        let s = rendered(|w| {
            write_int(w, -42)?;
            write_sep(w)?;
            write_float(w, 5.0)?;
            write_end(w)
        });
        assert_eq!(s, "-42 5.0\n");
    }

    #[test]
    fn print_lowering_of_mixed_arguments() {
        let s = rendered(|w| {
            write_str(w, b"x")?;
            write_sep(w)?;
            write_bool(w, 1)?;
            write_sep(w)?;
            write_none(w)?;
            write_end(w)
        });
        assert_eq!(s, "x True None\n");
    }

    // -- allocation ----------------------------------------------------

    #[test]
    fn allocation_returns_distinct_writable_blocks() {
        ty_rt_init();
        let mut blocks = Vec::new();
        for i in 1..=64usize {
            let size = i * 1024;
            let p = ty_alloc(size);
            assert!(!p.is_null());
            assert_eq!(p as usize % 8, 0, "blocks are at least 8-byte aligned");
            unsafe { std::ptr::write_bytes(p, 0xAB, size) };
            assert_eq!(unsafe { *p }, 0xAB);

            let q = ty_alloc_atomic(size);
            assert!(!q.is_null());
            unsafe { std::ptr::write_bytes(q, 0xCD, size) };
            assert_eq!(unsafe { *q.add(size - 1) }, 0xCD);

            blocks.push((p, size));
        }
        // Nothing overlaps, and the first write is still there.
        for (p, size) in &blocks {
            assert_eq!(unsafe { **p }, 0xAB);
            assert_eq!(unsafe { *p.add(size - 1) }, 0xAB);
        }
    }

    // -- str -----------------------------------------------------------

    /// Reads back a string value built by the runtime.
    fn read(s: *const u8) -> String {
        String::from_utf8(unsafe { str_bytes(s) }.to_vec()).unwrap()
    }

    /// The payload length recorded in a string header.
    fn byte_len(s: *const u8) -> i64 {
        unsafe { std::ptr::read_unaligned(s as *const i64) }
    }

    /// What [`str_cmp`] has to agree with: Rust's own ordering of two `str`s.
    fn str_cmp_reference(a: &str, b: &str) -> i64 {
        match a.cmp(b) {
            std::cmp::Ordering::Less => -1,
            std::cmp::Ordering::Equal => 0,
            std::cmp::Ordering::Greater => 1,
        }
    }

    /// The elements of a `list<int>`.
    unsafe fn list_i64s(list: *const TyList) -> Vec<i64> {
        unsafe {
            let data = (*list).data as *const i64;
            (0..(*list).len as usize)
                .map(|i| std::ptr::read(data.add(i)))
                .collect()
        }
    }

    /// The elements of a `list<str>`, decoded.
    unsafe fn list_strings(list: *const TyList) -> Vec<String> {
        unsafe {
            let data = (*list).data as *const *const u8;
            (0..(*list).len as usize)
                .map(|i| read(std::ptr::read(data.add(i))))
                .collect()
        }
    }

    /// A `list<int>` holding `values`.
    fn i64_list(values: &[i64]) -> *mut TyList {
        let list = ty_list_new(8, 1, values.len() as i64);
        unsafe {
            let data = (*list).data as *mut i64;
            for (i, v) in values.iter().enumerate() {
                std::ptr::write(data.add(i), *v);
            }
            (*list).len = values.len() as i64;
        }
        list
    }

    /// A `list<str>` holding `parts` — the shape [`ty_str_join`] consumes.
    fn str_list(parts: &[&str]) -> *mut TyList {
        let list = ty_list_new(8, 0, parts.len() as i64);
        unsafe {
            let data = (*list).data as *mut *mut u8;
            for (i, part) in parts.iter().enumerate() {
                std::ptr::write(data.add(i), str_new(part.as_bytes()));
            }
            (*list).len = parts.len() as i64;
        }
        list
    }

    #[test]
    fn strings_round_trip_through_the_heap() {
        ty_rt_init();
        for text in ["", "a", "Hello, Typhoon!", "台风", "\0\u{1}"] {
            let s = str_new(text.as_bytes());
            assert_eq!(read(s), text);
            // The header holds the byte length first, the code point count
            // second.
            assert_eq!(byte_len(s), text.len() as i64);
            assert_eq!(unsafe { str_char_len(s) }, text.chars().count() as i64);
        }
    }

    #[test]
    fn concatenation_allocates_a_new_string() {
        ty_rt_init();
        let a = str_new(b"Hello, ");
        let b = str_new("Typhoon 台风".as_bytes());
        let c = unsafe { ty_str_concat(a, b) };
        assert_eq!(read(c), "Hello, Typhoon 台风");
        // The operands are untouched: `str` is immutable.
        assert_eq!(read(a), "Hello, ");
        assert_eq!(read(b), "Typhoon 台风");

        let empty = str_new(b"");
        assert_eq!(read(unsafe { ty_str_concat(empty, empty) }), "");
        assert_eq!(read(unsafe { ty_str_concat(empty, a) }), "Hello, ");
        assert_eq!(read(unsafe { ty_str_concat(a, empty) }), "Hello, ");
    }

    #[test]
    fn string_equality_is_by_bytes() {
        ty_rt_init();
        let a = str_new(b"abc");
        let b = str_new(b"abc");
        let c = str_new(b"abd");
        let d = str_new(b"ab");
        assert_eq!(unsafe { ty_str_eq(a, b) }, 1);
        assert_eq!(unsafe { ty_str_eq(a, a) }, 1);
        assert_eq!(unsafe { ty_str_eq(a, c) }, 0);
        assert_eq!(unsafe { ty_str_eq(a, d) }, 0);
        let empty = str_new(b"");
        assert_eq!(unsafe { ty_str_eq(empty, empty) }, 1);
        assert_eq!(unsafe { ty_str_eq(empty, a) }, 0);
    }

    #[test]
    fn values_render_the_way_print_does() {
        ty_rt_init();
        assert_eq!(read(ty_str_from_int(-42)), "-42");
        assert_eq!(read(ty_str_from_int(i64::MIN)), "-9223372036854775808");
        assert_eq!(read(ty_str_from_float(5.0)), "5.0");
        assert_eq!(read(ty_str_from_float(1e16)), "1e+16");
        assert_eq!(read(ty_str_from_float(f64::NAN)), "nan");
        assert_eq!(read(ty_str_from_bool(1)), "True");
        assert_eq!(read(ty_str_from_bool(0)), "False");
    }

    #[test]
    fn fixed_precision_matches_python_format() {
        assert_eq!(format_fixed(3.5, 2), "3.50");
        assert_eq!(format_fixed(-0.0, 1), "-0.0");
        assert_eq!(format_fixed(2.675, 2), "2.67"); // the double is below 2.675
        assert_eq!(format_fixed(0.125, 2), "0.12"); // exact tie, rounds to even
        assert_eq!(format_fixed(0.375, 2), "0.38"); // exact tie, rounds to even
        assert_eq!(format_fixed(1.0, 0), "1");
        assert_eq!(format_fixed(1.5, 0), "2");
        assert_eq!(format_fixed(2.5, 0), "2");
        assert_eq!(format_fixed(1234.5678, 3), "1234.568");
        assert_eq!(format_fixed(f64::INFINITY, 2), "inf");
        assert_eq!(format_fixed(f64::NAN, 2), "nan");
        // Out-of-range precisions are clamped rather than panicking.
        assert_eq!(format_fixed(1.0, -3), "1");
        assert_eq!(format_fixed(1.0, 1000).len(), 34);
    }

    #[test]
    fn fixed_precision_allocates_a_string() {
        ty_rt_init();
        assert_eq!(read(ty_str_from_float_fixed(1.23456, 2)), "1.23");
        assert_eq!(read(ty_str_from_float_fixed(1.23456, 0)), "1");
    }

    // -- int_pow -------------------------------------------------------

    #[test]
    fn integer_power_by_squaring() {
        assert_eq!(int_pow_checked(2, 10), Some(1024));
        assert_eq!(int_pow_checked(3, 5), Some(243));
        assert_eq!(int_pow_checked(-2, 3), Some(-8));
        assert_eq!(int_pow_checked(-2, 4), Some(16));
        assert_eq!(int_pow_checked(7, 0), Some(1));
        assert_eq!(int_pow_checked(0, 0), Some(1));
        assert_eq!(int_pow_checked(0, 5), Some(0));
        assert_eq!(int_pow_checked(1, i64::MAX), Some(1));
        for base in [-5i64, -1, 0, 1, 2, 3, 10] {
            for exp in 0..12u32 {
                assert_eq!(
                    int_pow_checked(base, i64::from(exp)),
                    Some(base.wrapping_pow(exp)),
                    "{base} ** {exp}"
                );
            }
        }
    }

    #[test]
    fn integer_power_wraps_instead_of_trapping() {
        assert_eq!(int_pow_checked(2, 63), Some(i64::MIN));
        assert_eq!(int_pow_checked(2, 64), Some(0));
        assert_eq!(int_pow_checked(10, 19), Some(10i64.wrapping_pow(19)));
        assert_eq!(
            int_pow_checked(i64::MAX, 2),
            Some(i64::MAX.wrapping_mul(i64::MAX))
        );
    }

    #[test]
    fn a_negative_exponent_has_no_value() {
        // `ty_int_pow` turns this into `panic: negative exponent in integer
        // power`; the panic itself exits the process, so only the inner
        // function can be tested here.
        assert_eq!(int_pow_checked(2, -1), None);
        assert_eq!(int_pow_checked(0, i64::MIN), None);
        assert_eq!(ty_int_pow(2, 10), 1024);
    }

    #[test]
    fn zero_length_string_is_safe() {
        let s = unsafe { bytes(std::ptr::null(), 0) };
        assert!(s.is_empty());
    }

    // -- code points ---------------------------------------------------

    #[test]
    fn char_count_counts_code_points() {
        assert_eq!(char_count(b""), 0);
        assert_eq!(char_count(b"abc"), 3);
        assert_eq!(char_count("台风".as_bytes()), 2);
        assert_eq!(char_count("héllo".as_bytes()), 5);
        assert_eq!(char_count("🌀".as_bytes()), 1);
        assert_eq!(char_count("a🌀台b".as_bytes()), 4);
        assert_eq!(char_count(b"\x00\x7f"), 2);
        // The count is the code point count, never the byte count.
        assert_eq!("台风".len(), 6);
        assert_eq!("🌀".len(), 4);
    }

    #[test]
    fn utf8_sequence_lengths_come_from_the_lead_byte() {
        assert_eq!(utf8_seq_len(b'a'), 1);
        assert_eq!(utf8_seq_len(0x00), 1);
        assert_eq!(utf8_seq_len(0x7f), 1);
        assert_eq!(utf8_seq_len(0xC3), 2); // é
        assert_eq!(utf8_seq_len(0xE5), 3); // 台
        assert_eq!(utf8_seq_len(0xF0), 4); // 🌀
        // Continuation bytes and invalid lead bytes advance by one.
        assert_eq!(utf8_seq_len(0x80), 1);
        assert_eq!(utf8_seq_len(0xBF), 1);
        assert_eq!(utf8_seq_len(0xF8), 1);
        assert_eq!(utf8_seq_len(0xFF), 1);
    }

    #[test]
    fn walking_by_sequence_length_visits_every_code_point() {
        for text in ["", "a", "台风", "a🌀b", "héllo, world"] {
            let bytes = text.as_bytes();
            let (mut at, mut seen) = (0usize, 0i64);
            while at < bytes.len() {
                at += utf8_seq_len(bytes[at]);
                seen += 1;
            }
            assert_eq!(at, bytes.len(), "{text:?} ended mid-sequence");
            assert_eq!(seen, char_count(bytes), "{text:?}");
        }
    }

    // -- str header ----------------------------------------------------

    #[test]
    fn the_header_holds_both_lengths() {
        ty_rt_init();
        for text in ["", "a", "Hello, Typhoon!", "台风", "a🌀b", "\0\u{1}"] {
            let s = str_new(text.as_bytes());
            assert_eq!(byte_len(s), text.len() as i64, "byte_len of {text:?}");
            assert_eq!(
                unsafe { str_char_len(s) },
                text.chars().count() as i64,
                "char_len of {text:?}"
            );
            assert_eq!(read(s), text);
            assert_eq!(s as usize % 8, 0, "the payload stays 8-byte aligned");
        }
    }

    #[test]
    fn a_known_char_len_is_stored_verbatim() {
        ty_rt_init();
        let s = str_new_with_char_len(b"abc", 3);
        assert_eq!(read(s), "abc");
        assert_eq!(byte_len(s), 3);
        assert_eq!(unsafe { str_char_len(s) }, 3);
        let empty = str_new_with_char_len(b"", 0);
        assert_eq!(read(empty), "");
        assert_eq!(unsafe { str_char_len(empty) }, 0);
    }

    #[test]
    fn concatenation_sums_the_character_counts() {
        ty_rt_init();
        let a = str_new("台风".as_bytes());
        let b = str_new(b"!");
        let c = unsafe { ty_str_concat(a, b) };
        assert_eq!(read(c), "台风!");
        assert_eq!(byte_len(c), 7);
        assert_eq!(unsafe { str_char_len(c) }, 3);
        let empty = str_new(b"");
        assert_eq!(unsafe { str_char_len(ty_str_concat(empty, empty)) }, 0);
        assert_eq!(unsafe { str_char_len(ty_str_concat(empty, a)) }, 2);
        assert_eq!(unsafe { str_char_len(ty_str_concat(a, empty)) }, 2);
    }

    #[test]
    fn rendered_values_know_their_length() {
        ty_rt_init();
        let s = ty_str_from_int(-42);
        assert_eq!(byte_len(s), 3);
        assert_eq!(unsafe { str_char_len(s) }, 3);
        assert_eq!(unsafe { str_char_len(ty_str_from_bool(0)) }, 5);
        assert_eq!(unsafe { str_char_len(ty_str_from_float(5.0)) }, 3);
        assert_eq!(unsafe { str_char_len(ty_str_from_float_fixed(1.5, 3)) }, 5);
    }

    // -- str comparison ------------------------------------------------

    #[test]
    fn comparison_is_lexicographic_by_bytes() {
        assert_eq!(str_cmp(b"a", b"b"), -1);
        assert_eq!(str_cmp(b"b", b"a"), 1);
        assert_eq!(str_cmp(b"a", b"a"), 0);
        assert_eq!(str_cmp(b"", b""), 0);
        assert_eq!(str_cmp(b"", b"a"), -1);
        assert_eq!(str_cmp(b"a", b""), 1);
        assert_eq!(str_cmp(b"ab", b"b"), -1);
        assert_eq!(str_cmp(b"abc", b"ab"), 1);
        // UTF-8 byte order is code point order.
        assert_eq!(str_cmp("é".as_bytes(), "台".as_bytes()), -1);
        assert_eq!(str_cmp(b"z", "é".as_bytes()), -1);
        assert_eq!(str_cmp("台风".as_bytes(), "台".as_bytes()), 1);
    }

    #[test]
    fn ty_str_cmp_agrees_with_the_helper() {
        ty_rt_init();
        let words = ["", "a", "ab", "b", "abc", "台", "台风", "é", "🌀"];
        for a in words {
            for b in words {
                let expected = str_cmp(a.as_bytes(), b.as_bytes());
                let got = unsafe { ty_str_cmp(str_new(a.as_bytes()), str_new(b.as_bytes())) };
                assert_eq!(got, expected, "{a:?} <=> {b:?}");
                // And it agrees with Rust's own ordering of the `str`s.
                assert_eq!(got, str_cmp_reference(a, b), "{a:?} <=> {b:?}");
            }
        }
    }

    // -- str queries ---------------------------------------------------

    #[test]
    fn find_returns_a_byte_offset() {
        assert_eq!(str_find_bytes(b"hello", b"ll"), Some(2));
        assert_eq!(str_find_bytes(b"hello", b"h"), Some(0));
        assert_eq!(str_find_bytes(b"hello", b"o"), Some(4));
        assert_eq!(str_find_bytes(b"hello", b"z"), None);
        assert_eq!(str_find_bytes(b"hello", b""), Some(0));
        assert_eq!(str_find_bytes(b"", b""), Some(0));
        assert_eq!(str_find_bytes(b"", b"a"), None);
        assert_eq!(str_find_bytes(b"ab", b"abc"), None);
        assert_eq!(str_find_bytes(b"aaa", b"aa"), Some(0));
        assert_eq!(str_find_bytes("台风".as_bytes(), "风".as_bytes()), Some(3));
    }

    #[test]
    fn find_reports_code_point_indices() {
        assert_eq!(str_find_chars(b"hello", b"l"), 2);
        assert_eq!(str_find_chars("台风x".as_bytes(), b"x"), 2);
        assert_eq!(str_find_chars("a🌀b".as_bytes(), b"b"), 2);
        assert_eq!(str_find_chars("héllo".as_bytes(), b"llo"), 2);
        assert_eq!(str_find_chars(b"abc", b"z"), -1);
        assert_eq!(str_find_chars(b"abc", b""), 0);
        assert_eq!(str_find_chars(b"", b""), 0);
        assert_eq!(str_find_chars(b"", b"a"), -1);
    }

    #[test]
    fn substring_queries_over_string_values() {
        ty_rt_init();
        let s = str_new("台风 warning".as_bytes());
        let empty = str_new(b"");
        unsafe {
            assert_eq!(ty_str_contains(s, str_new("风".as_bytes())), 1);
            assert_eq!(ty_str_contains(s, str_new(b"warn")), 1);
            assert_eq!(ty_str_contains(s, empty), 1);
            assert_eq!(ty_str_contains(empty, empty), 1);
            assert_eq!(ty_str_contains(empty, s), 0);
            assert_eq!(ty_str_contains(s, str_new(b"typhoon")), 0);
            assert_eq!(ty_str_find(s, str_new(b"warning")), 3);
            assert_eq!(ty_str_find(s, str_new("风".as_bytes())), 1);
            assert_eq!(ty_str_find(s, str_new(b"x")), -1);
            assert_eq!(ty_str_find(s, empty), 0);
        }
    }

    #[test]
    fn prefixes_and_suffixes() {
        ty_rt_init();
        let s = str_new("台风 warning".as_bytes());
        let empty = str_new(b"");
        unsafe {
            assert_eq!(ty_str_startswith(s, str_new("台".as_bytes())), 1);
            assert_eq!(ty_str_startswith(s, s), 1);
            assert_eq!(ty_str_startswith(s, empty), 1);
            assert_eq!(ty_str_startswith(s, str_new(b"warning")), 0);
            assert_eq!(ty_str_startswith(empty, str_new(b"a")), 0);
            assert_eq!(ty_str_endswith(s, str_new(b"warning")), 1);
            assert_eq!(ty_str_endswith(s, s), 1);
            assert_eq!(ty_str_endswith(s, empty), 1);
            assert_eq!(ty_str_endswith(s, str_new(b"Warning")), 0);
            assert_eq!(ty_str_endswith(empty, empty), 1);
        }
    }

    // -- str transforms ------------------------------------------------

    #[test]
    fn case_mapping_is_ascii_only() {
        ty_rt_init();
        let s = str_new("héllo, 台风 42!".as_bytes());
        let upper = unsafe { ty_str_upper(s) };
        assert_eq!(read(upper), "HéLLO, 台风 42!");
        let lower = unsafe { ty_str_lower(upper) };
        assert_eq!(read(lower), "héllo, 台风 42!");
        // Both lengths survive an ASCII case mapping...
        for mapped in [upper, lower] {
            assert_eq!(byte_len(mapped), byte_len(s));
            assert_eq!(unsafe { str_char_len(mapped) }, unsafe { str_char_len(s) });
        }
        // ... and the operand is untouched: `str` is immutable.
        assert_eq!(read(s), "héllo, 台风 42!");
    }

    #[test]
    fn case_mapping_of_an_empty_string() {
        ty_rt_init();
        let empty = str_new(b"");
        assert_eq!(read(unsafe { ty_str_upper(empty) }), "");
        assert_eq!(read(unsafe { ty_str_lower(empty) }), "");
        assert_eq!(unsafe { str_char_len(ty_str_upper(empty)) }, 0);
    }

    #[test]
    fn case_mapping_leaves_every_non_ascii_byte_alone() {
        ty_rt_init();
        let source: Vec<u8> = (0u8..=255).collect();
        let s = str_new_with_char_len(&source, source.len() as i64);
        let upper = unsafe { str_bytes(ty_str_upper(s)) };
        let lower = unsafe { str_bytes(ty_str_lower(s)) };
        for (i, b) in source.iter().enumerate() {
            assert_eq!(upper[i], b.to_ascii_uppercase());
            assert_eq!(lower[i], b.to_ascii_lowercase());
            if !b.is_ascii() {
                assert_eq!(upper[i], *b);
                assert_eq!(lower[i], *b);
            }
        }
    }

    #[test]
    fn strip_removes_ascii_whitespace_at_both_ends() {
        assert_eq!(str_strip(b"  a  "), b"a".as_slice());
        assert_eq!(str_strip(b"\t\n\r\x0b\x0ca\x0c\x0b\r\n\t"), b"a".as_slice());
        assert_eq!(str_strip(b"   "), b"".as_slice());
        assert_eq!(str_strip(b""), b"".as_slice());
        assert_eq!(str_strip(b"a b"), b"a b".as_slice());
        assert_eq!(str_strip(b" a b "), b"a b".as_slice());
        assert_eq!(str_strip(b"abc"), b"abc".as_slice());
        // A NUL is not whitespace, and neither is a non-breaking space.
        assert_eq!(str_strip(b"\0a\0"), b"\0a\0".as_slice());
        assert_eq!(
            str_strip("\u{a0}x\u{a0}".as_bytes()),
            "\u{a0}x\u{a0}".as_bytes()
        );
    }

    #[test]
    fn every_ascii_space_is_recognised() {
        for b in 0u8..=255 {
            let expected = matches!(b, b' ' | b'\t' | b'\n' | b'\r' | 0x0b | 0x0c);
            assert_eq!(is_ascii_space(b), expected, "byte {b:#04x}");
        }
    }

    #[test]
    fn ty_str_strip_keeps_the_char_count_consistent() {
        ty_rt_init();
        let s = str_new("  台风 \n".as_bytes());
        let stripped = unsafe { ty_str_strip(s) };
        assert_eq!(read(stripped), "台风");
        assert_eq!(byte_len(stripped), 6);
        assert_eq!(unsafe { str_char_len(stripped) }, 2);
        // The operand is untouched.
        assert_eq!(read(s), "  台风 \n");

        let blank = str_new(b" \t\n\x0b\x0c");
        let empty = unsafe { ty_str_strip(blank) };
        assert_eq!(read(empty), "");
        assert_eq!(unsafe { str_char_len(empty) }, 0);

        let untouched = str_new("a🌀b".as_bytes());
        assert_eq!(unsafe { str_char_len(ty_str_strip(untouched)) }, 3);
    }

    #[test]
    fn replace_follows_python() {
        let cases: &[(&str, &str, &str, &str)] = &[
            ("hello", "l", "L", "heLLo"),
            ("hello", "ll", "LL", "heLLo"),
            ("aaa", "aa", "b", "ba"),
            ("aaaa", "aa", "b", "bb"),
            ("banana", "a", "", "bnn"),
            ("aaa", "a", "aa", "aaaaaa"),
            ("abc", "z", "y", "abc"),
            ("abc", "abcd", "x", "abc"),
            ("", "a", "b", ""),
            ("ab", "", "-", "-a-b-"),
            ("", "", "-", "-"),
            ("a", "", "", "a"),
            ("台风", "台", "大", "大风"),
            ("台风", "", "-", "-台-风-"),
            ("a🌀b", "🌀", "-", "a-b"),
        ];
        for (s, old, new, expected) in cases {
            let got = str_replace(s.as_bytes(), old.as_bytes(), new.as_bytes());
            assert_eq!(
                String::from_utf8(got).unwrap(),
                *expected,
                "{s:?}.replace({old:?}, {new:?})"
            );
        }
    }

    #[test]
    fn ty_str_replace_counts_the_new_code_points() {
        ty_rt_init();
        let out =
            unsafe { ty_str_replace(str_new(b"a-b"), str_new(b"-"), str_new("台".as_bytes())) };
        assert_eq!(read(out), "a台b");
        assert_eq!(byte_len(out), 5);
        assert_eq!(unsafe { str_char_len(out) }, 3);

        let deleted = unsafe {
            ty_str_replace(
                str_new("台风".as_bytes()),
                str_new("风".as_bytes()),
                str_new(b""),
            )
        };
        assert_eq!(read(deleted), "台");
        assert_eq!(unsafe { str_char_len(deleted) }, 1);
    }

    // -- split / join --------------------------------------------------

    #[test]
    fn split_follows_python() {
        let cases: &[(&str, &str, &[&str])] = &[
            ("a,b", ",", &["a", "b"]),
            ("", ",", &[""]),
            ("a,,b", ",", &["a", "", "b"]),
            (",a", ",", &["", "a"]),
            ("a,", ",", &["a", ""]),
            (",", ",", &["", ""]),
            ("abc", ",", &["abc"]),
            ("a::b::c", "::", &["a", "b", "c"]),
            ("aaa", "aa", &["", "a"]),
            ("台,风", ",", &["台", "风"]),
            ("a台b", "台", &["a", "b"]),
            ("ab", "abc", &["ab"]),
        ];
        for (s, sep, expected) in cases {
            let parts = str_split_checked(s.as_bytes(), sep.as_bytes()).expect("a non-empty sep");
            let parts: Vec<&str> = parts
                .iter()
                .map(|p| std::str::from_utf8(p).unwrap())
                .collect();
            assert_eq!(parts, *expected, "{s:?}.split({sep:?})");
        }
    }

    #[test]
    fn an_empty_separator_has_no_split() {
        // `ty_str_split` turns this into `panic: empty separator`; the panic
        // exits the process, so only the inner function can be tested here.
        assert!(str_split_checked(b"abc", b"").is_none());
        assert!(str_split_checked(b"", b"").is_none());
        assert!(str_split_checked(b"abc", b"b").is_some());
    }

    #[test]
    fn ty_str_split_builds_a_list_of_strings() {
        ty_rt_init();
        let list = unsafe { ty_str_split(str_new(b"a,,b"), str_new(b",")) };
        unsafe {
            assert_eq!((*list).len, 3);
            assert!((*list).cap >= 3);
            assert_eq!(list_strings(list), ["a", "", "b"]);
        }
        // Every element is a proper string value with its own code point count.
        let list = unsafe { ty_str_split(str_new("台,风".as_bytes()), str_new(b",")) };
        unsafe {
            assert_eq!(list_strings(list), ["台", "风"]);
            let data = (*list).data as *const *const u8;
            for i in 0..2 {
                let part = std::ptr::read(data.add(i));
                assert_eq!(byte_len(part), 3);
                assert_eq!(str_char_len(part), 1);
            }
        }
        // A string without the separator splits into itself.
        let list = unsafe { ty_str_split(str_new(b""), str_new(b",")) };
        unsafe {
            assert_eq!((*list).len, 1);
            assert_eq!(list_strings(list), [""]);
        }
    }

    #[test]
    fn join_places_the_separator_between_parts() {
        fn join(sep: &str, parts: &[&str]) -> String {
            let parts: Vec<&[u8]> = parts.iter().map(|p| p.as_bytes()).collect();
            String::from_utf8(str_join(sep.as_bytes(), &parts)).unwrap()
        }
        assert_eq!(join(", ", &["a", "b", "c"]), "a, b, c");
        assert_eq!(join(", ", &[]), "");
        assert_eq!(join(", ", &["only"]), "only");
        assert_eq!(join("", &["a", "b"]), "ab");
        assert_eq!(join(",", &["", ""]), ",");
        assert_eq!(join("台", &["a", "b"]), "a台b");
        assert_eq!(join(", ", &["台风", "🌀"]), "台风, 🌀");
    }

    #[test]
    fn ty_str_join_walks_a_list_of_strings() {
        ty_rt_init();
        unsafe {
            let out = ty_str_join(str_new(b", "), str_list(&["a", "b", "c"]));
            assert_eq!(read(out), "a, b, c");
            assert_eq!(str_char_len(out), 7);

            let none = ty_str_join(str_new(b","), str_list(&[]));
            assert_eq!(read(none), "");
            assert_eq!(str_char_len(none), 0);

            let one = ty_str_join(str_new(b","), str_list(&["台风"]));
            assert_eq!(read(one), "台风");
            assert_eq!(str_char_len(one), 2);

            let wide = ty_str_join(str_new("台".as_bytes()), str_list(&["a", "b"]));
            assert_eq!(read(wide), "a台b");
            assert_eq!(byte_len(wide), 5);
            assert_eq!(str_char_len(wide), 3);
        }
    }

    #[test]
    fn split_and_join_round_trip() {
        ty_rt_init();
        for text in ["a,台,,b", "", ",", "no separator here"] {
            unsafe {
                let sep = str_new(b",");
                let parts = ty_str_split(str_new(text.as_bytes()), sep);
                let back = ty_str_join(sep, parts);
                assert_eq!(read(back), text);
                assert_eq!(str_char_len(back), text.chars().count() as i64);
            }
        }
    }

    // -- char_at -------------------------------------------------------

    #[test]
    fn char_at_returns_one_code_point() {
        ty_rt_init();
        let s = str_new("a台🌀".as_bytes());
        unsafe {
            let a = ty_str_char_at(s, 0);
            assert_eq!(read(a), "a");
            assert_eq!(byte_len(a), 1);
            assert_eq!(str_char_len(a), 1);

            let wide = ty_str_char_at(s, 1);
            assert_eq!(read(wide), "台");
            assert_eq!(byte_len(wide), 3);
            assert_eq!(str_char_len(wide), 1);

            let widest = ty_str_char_at(s, 4);
            assert_eq!(read(widest), "🌀");
            assert_eq!(byte_len(widest), 4);
            assert_eq!(str_char_len(widest), 1);
        }
    }

    #[test]
    fn char_at_walks_a_whole_string() {
        ty_rt_init();
        for text in ["", "abc", "héllo 台风 🌀!", "\0\u{1}\u{7f}"] {
            let s = str_new(text.as_bytes());
            let mut at = 0i64;
            let mut seen = String::new();
            while at < text.len() as i64 {
                let c = unsafe { ty_str_char_at(s, at) };
                seen.push_str(&read(c));
                assert_eq!(unsafe { str_char_len(c) }, 1);
                at += byte_len(c);
            }
            assert_eq!(seen, text);
            assert_eq!(seen.chars().count() as i64, unsafe { str_char_len(s) });
        }
    }

    #[test]
    fn a_truncated_sequence_never_runs_past_the_end() {
        ty_rt_init();
        // A lead byte announcing four bytes, with only two of them present.
        let s = str_new_with_char_len(b"\xf0\x9f", 1);
        let c = unsafe { ty_str_char_at(s, 0) };
        assert_eq!(byte_len(c), 2);
        assert_eq!(unsafe { str_bytes(c) }, b"\xf0\x9f".as_slice());
        // A stray continuation byte advances by one.
        let s = str_new_with_char_len(b"\x9f\x9f", 2);
        assert_eq!(byte_len(unsafe { ty_str_char_at(s, 0) }), 1);
    }

    #[test]
    fn ascii_characters_come_from_the_static_table() {
        ty_rt_init();
        let text = str_new(b"aa");
        let first = unsafe { ty_str_char_at(text, 0) };
        let second = unsafe { ty_str_char_at(text, 1) };
        // The very same static entry: iterating ASCII allocates nothing.
        assert_eq!(first, second);
        assert_eq!(first, unsafe { ty_str_char_at(str_new(b"xa"), 1) });
        assert_eq!(read(first), "a");

        // Every ASCII byte has an entry laid out like a heap string.
        for b in 0u8..0x80 {
            let s = str_new_with_char_len(&[b], 1);
            let c = unsafe { ty_str_char_at(s, 0) };
            assert_eq!(byte_len(c), 1, "entry {b:#04x}");
            assert_eq!(unsafe { str_char_len(c) }, 1, "entry {b:#04x}");
            assert_eq!(unsafe { str_bytes(c) }, [b].as_slice());
            assert_eq!(c as usize % 8, 0, "entries stay 8-byte aligned");
        }

        // A non-ASCII code point is copied onto the heap instead.
        let wide = str_new("台".as_bytes());
        let a = unsafe { ty_str_char_at(wide, 0) };
        let b = unsafe { ty_str_char_at(wide, 0) };
        assert_ne!(a, b);
        assert_eq!(read(a), read(b));
    }

    // -- lists ---------------------------------------------------------

    #[test]
    fn the_list_header_is_twenty_four_bytes() {
        assert_eq!(size_of::<TyList>(), 24);
        assert_eq!(align_of::<TyList>(), 8);
        assert_eq!(std::mem::offset_of!(TyList, len), 0);
        assert_eq!(std::mem::offset_of!(TyList, cap), 8);
        assert_eq!(std::mem::offset_of!(TyList, data), 16);
    }

    #[test]
    fn a_new_list_is_empty() {
        ty_rt_init();
        unsafe {
            let list = ty_list_new(8, 1, 0);
            assert_eq!((*list).len, 0);
            assert_eq!((*list).cap, 0);
            assert!((*list).data.is_null(), "capacity 0 allocates no buffer");

            let list = ty_list_new(8, 1, 4);
            assert_eq!((*list).len, 0);
            assert_eq!((*list).cap, 4);
            assert!(!(*list).data.is_null());
            assert_eq!((*list).data as usize % 8, 0);

            // A negative capacity is treated as zero.
            let list = ty_list_new(8, 1, -3);
            assert_eq!((*list).cap, 0);
            assert!((*list).data.is_null());
        }
    }

    #[test]
    fn growth_from_zero_capacity_starts_at_four() {
        ty_rt_init();
        unsafe {
            let list = ty_list_new(8, 1, 0);
            ty_list_grow(list, 8, 1, 1);
            assert_eq!((*list).cap, 4);
            assert!(!(*list).data.is_null());

            // Already big enough: nothing moves.
            let data = (*list).data;
            ty_list_grow(list, 8, 1, 4);
            assert_eq!((*list).cap, 4);
            assert_eq!((*list).data, data);
            ty_list_grow(list, 8, 1, 0);
            assert_eq!((*list).cap, 4);

            // Doubling wins while it is enough...
            ty_list_grow(list, 8, 1, 5);
            assert_eq!((*list).cap, 8);
            // ... and `needed` wins when it is not.
            ty_list_grow(list, 8, 1, 100);
            assert_eq!((*list).cap, 100);
        }
    }

    #[test]
    fn growth_preserves_the_elements() {
        ty_rt_init();
        let list = i64_list(&[1, 2, 3]);
        unsafe {
            ty_list_grow(list, 8, 1, 9);
            assert_eq!((*list).cap, 9);
            assert_eq!((*list).len, 3);
            assert_eq!(list_i64s(list), [1, 2, 3]);
        }
    }

    #[test]
    fn appending_one_element_at_a_time_grows_geometrically() {
        ty_rt_init();
        let list = ty_list_new(8, 1, 0);
        let mut capacities = Vec::new();
        for i in 0..64i64 {
            unsafe {
                let slot = ty_list_insert_slot(list, i64::MAX, 8, 1) as *mut i64;
                std::ptr::write(slot, i * i);
                capacities.push((*list).cap);
            }
        }
        let expected: Vec<i64> = (0..64).map(|i| i * i).collect();
        assert_eq!(unsafe { list_i64s(list) }, expected);
        assert_eq!(unsafe { (*list).len }, 64);
        assert!(unsafe { (*list).cap } >= 64);
        // 4, 8, 16, 32, 64: five reallocations for 64 appends.
        capacities.dedup();
        assert_eq!(capacities, [4, 8, 16, 32, 64]);
    }

    #[test]
    fn insert_uses_python_index_rules() {
        ty_rt_init();
        let list = i64_list(&[10, 20, 30]);
        unsafe {
            // In the middle.
            std::ptr::write(ty_list_insert_slot(list, 1, 8, 1) as *mut i64, 15);
            assert_eq!(list_i64s(list), [10, 15, 20, 30]);
            // A negative index counts from the end.
            std::ptr::write(ty_list_insert_slot(list, -1, 8, 1) as *mut i64, 25);
            assert_eq!(list_i64s(list), [10, 15, 20, 25, 30]);
            // Far too negative clamps to the front, far too large appends.
            std::ptr::write(ty_list_insert_slot(list, -100, 8, 1) as *mut i64, 5);
            std::ptr::write(ty_list_insert_slot(list, 100, 8, 1) as *mut i64, 35);
            assert_eq!(list_i64s(list), [5, 10, 15, 20, 25, 30, 35]);
            assert_eq!((*list).len, 7);
        }
    }

    #[test]
    fn insert_into_an_empty_list() {
        ty_rt_init();
        unsafe {
            for index in [0i64, -1, 7, i64::MIN, i64::MAX] {
                let list = ty_list_new(8, 1, 0);
                let slot = ty_list_insert_slot(list, index, 8, 1) as *mut i64;
                std::ptr::write(slot, 42);
                assert_eq!(list_i64s(list), [42], "insert at {index}");
                assert_eq!((*list).cap, 4);
            }
        }
    }

    #[test]
    fn insert_moves_single_byte_elements() {
        ty_rt_init();
        // `bool` elements are one byte wide.
        let list = ty_list_new(1, 1, 0);
        unsafe {
            for v in [1u8, 0, 1] {
                std::ptr::write(ty_list_insert_slot(list, i64::MAX, 1, 1), v);
            }
            std::ptr::write(ty_list_insert_slot(list, 1, 1, 1), 0);
            let data = std::slice::from_raw_parts((*list).data, (*list).len as usize);
            assert_eq!(data, [1u8, 0, 0, 1].as_slice());
        }
    }

    #[test]
    fn concatenation_copies_both_lists() {
        ty_rt_init();
        let a = i64_list(&[1, 2, 3]);
        let b = i64_list(&[4, 5]);
        unsafe {
            let c = ty_list_concat(a, b, 8, 1);
            assert_eq!(list_i64s(c), [1, 2, 3, 4, 5]);
            assert_eq!((*c).len, 5);
            assert!((*c).cap >= 5);
            // The result is independent of both operands.
            std::ptr::write((*a).data as *mut i64, 99);
            assert_eq!(list_i64s(c), [1, 2, 3, 4, 5]);

            let empty = ty_list_new(8, 1, 0);
            assert!(list_i64s(ty_list_concat(empty, empty, 8, 1)).is_empty());
            assert_eq!((*ty_list_concat(empty, empty, 8, 1)).cap, 0);
            assert_eq!(list_i64s(ty_list_concat(empty, b, 8, 1)), [4, 5]);
            assert_eq!(list_i64s(ty_list_concat(a, empty, 8, 1)), [99, 2, 3]);
        }
    }

    #[test]
    fn lists_of_strings_hold_pointers() {
        ty_rt_init();
        let a = str_list(&["a", "台"]);
        let b = str_list(&["b"]);
        unsafe {
            let c = ty_list_concat(a, b, 8, 0);
            assert_eq!(list_strings(c), ["a", "台", "b"]);
            // Inserting a string keeps the buffer a list of pointers.
            let slot = ty_list_insert_slot(c, 0, 8, 0) as *mut *mut u8;
            std::ptr::write(slot, str_new("🌀".as_bytes()));
            assert_eq!(list_strings(c), ["🌀", "a", "台", "b"]);
        }
    }

    // -- container printing --------------------------------------------

    #[test]
    fn str_repr_quotes_and_escapes() {
        let cases: &[(&str, &str)] = &[
            ("", "\"\""),
            ("abc", "\"abc\""),
            ("a\"b", "\"a\\\"b\""),
            ("a\\b", "\"a\\\\b\""),
            ("a\nb", "\"a\\nb\""),
            ("a\tb", "\"a\\tb\""),
            ("a\rb", "\"a\\rb\""),
            ("台风", "\"台风\""),
            ("🌀", "\"🌀\""),
            // Unlike Python's `repr`, the quote is always `"`.
            ("it's", "\"it's\""),
        ];
        for (input, expected) in cases {
            assert_eq!(
                rendered(|w| write_str_repr(w, input.as_bytes())),
                *expected,
                "repr({input:?})"
            );
        }
    }

    #[test]
    fn str_repr_escapes_control_bytes_in_lower_case_hex() {
        assert_eq!(
            rendered(|w| write_str_repr(w, b"\x00\x01\x1f\x7f")),
            "\"\\x00\\x01\\x1f\\x7f\""
        );
        assert_eq!(
            rendered(|w| write_str_repr(w, b"\x0b\x0c")),
            "\"\\x0b\\x0c\""
        );
        assert_eq!(rendered(|w| write_str_repr(w, b"\x1b[0m")), "\"\\x1b[0m\"");
        // 0x20 and 0x7e are the printable boundaries and stay verbatim.
        assert_eq!(rendered(|w| write_str_repr(w, b" ~")), "\" ~\"");
        // A multi-byte sequence is written through byte for byte.
        assert_eq!(
            rendered(|w| write_str_repr(w, "\u{80}".as_bytes())),
            "\"\u{80}\""
        );
    }

    #[test]
    fn str_repr_is_the_container_form_of_design_4_6() {
        let s = rendered(|w| {
            w.write_all(b"[")?;
            write_str_repr(w, b"a")?;
            w.write_all(b", ")?;
            write_str_repr(w, b"b")?;
            w.write_all(b"]")
        });
        assert_eq!(s, "[\"a\", \"b\"]");
    }
}
