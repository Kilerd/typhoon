//! Memory layout for the wasm target.
//!
//! The rules are the C ones the LLVM backend relies on — every member at the
//! next multiple of its own alignment, the whole rounded up to the widest
//! member's alignment — with one difference: a pointer is **four** bytes wide
//! and four-byte aligned, because wasm32 linear memory is addressed with `i32`
//! (DESIGN §6.2.1).
//!
//! `bool` is one byte in memory and `int` / `float` are eight, exactly as
//! natively, so only pointer-shaped members move. The `str` and `list` headers
//! are laid out so that they do *not* move: both start with two `i64`s, which
//! keeps the pointer that follows at offset 16 on both targets (the runtime
//! pins this with a compile-time assertion next to `TyList`).

use typhoon_sema::types::{ClassId, Type, TypeId, TypeTable};

/// Size in bytes of a pointer into wasm linear memory.
pub const PTR_SIZE: u32 = 4;

/// Byte offset of a `list<T>`'s `len` field.
pub const LIST_LEN_OFFSET: u32 = 0;
/// Byte offset of a `list<T>`'s `cap` field.
pub const LIST_CAP_OFFSET: u32 = 8;
/// Byte offset of a `list<T>`'s `data` field, the same on wasm32 as natively
/// because the two `i64` lengths in front of it are eight-byte aligned.
pub const LIST_DATA_OFFSET: u32 = 16;

/// Byte offset of a `str`'s code-point count; its byte length is at 0.
pub const STR_CHAR_LEN_OFFSET: u32 = 8;

/// Size of the `str` header, `{ byte_len: i64, char_len: i64 }`.
pub const STR_HEADER: u32 = 16;

/// Layout queries over one program's type table.
#[derive(Debug, Clone, Copy)]
pub struct Layout<'t> {
    types: &'t TypeTable,
}

impl<'t> Layout<'t> {
    /// Layout queries for `types`.
    pub fn new(types: &'t TypeTable) -> Layout<'t> {
        Layout { types }
    }

    /// The number of bytes a value of `ty` occupies in memory — inside a list,
    /// a tuple or a class instance.
    pub fn size_of(&self, ty: TypeId) -> u32 {
        match self.types.get(ty) {
            Type::Bool => 1,
            Type::Int | Type::Float => 8,
            Type::Str | Type::List(_) | Type::Class(_) | Type::Optional(_) => PTR_SIZE,
            Type::Tuple(id) => {
                let members = self.types.tuple_members(id).to_vec();
                self.struct_size(&members)
            }
            Type::Unit | Type::Error => 0,
        }
    }

    /// The alignment of a value of `ty` in memory.
    pub fn align_of(&self, ty: TypeId) -> u32 {
        match self.types.get(ty) {
            Type::Bool => 1,
            Type::Int | Type::Float => 8,
            Type::Str | Type::List(_) | Type::Class(_) | Type::Optional(_) => PTR_SIZE,
            Type::Tuple(id) => self
                .types
                .tuple_members(id)
                .iter()
                .map(|m| self.align_of(*m))
                .max()
                .unwrap_or(1),
            Type::Unit | Type::Error => 1,
        }
    }

    /// The size of a struct holding `members` back to back.
    pub fn struct_size(&self, members: &[TypeId]) -> u32 {
        let mut size: u32 = 0;
        let mut align: u32 = 1;
        for member in members {
            let member_align = self.align_of(*member);
            align = align.max(member_align);
            size = size.div_ceil(member_align) * member_align;
            size += self.size_of(*member);
        }
        size.div_ceil(align) * align
    }

    /// The byte offset of every member of a struct holding `members`.
    pub fn field_offsets(&self, members: &[TypeId]) -> Vec<u32> {
        let mut offsets = Vec::with_capacity(members.len());
        let mut size: u32 = 0;
        for member in members {
            let member_align = self.align_of(*member);
            size = size.div_ceil(member_align) * member_align;
            offsets.push(size);
            size += self.size_of(*member);
        }
        offsets
    }

    /// The member types of a tuple type.
    pub fn tuple_members(&self, ty: TypeId) -> Vec<TypeId> {
        match self.types.get(ty) {
            Type::Tuple(id) => self.types.tuple_members(id).to_vec(),
            _ => Vec::new(),
        }
    }

    /// The field types of a class, in declaration order.
    pub fn class_fields(&self, class: ClassId) -> Vec<TypeId> {
        self.types
            .class(class)
            .fields
            .iter()
            .map(|f| f.ty)
            .collect()
    }

    /// The byte offset of every field of a class instance.
    pub fn class_offsets(&self, class: ClassId) -> Vec<u32> {
        let fields = self.class_fields(class);
        self.field_offsets(&fields)
    }

    /// The size of a class instance, never zero: `ty_alloc(0)` would hand two
    /// distinct objects the same address, and `a is b` must stay false for
    /// two separately constructed instances.
    pub fn class_size(&self, class: ClassId) -> u32 {
        let fields = self.class_fields(class);
        self.struct_size(&fields).max(1)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn table() -> TypeTable {
        TypeTable::new()
    }

    #[test]
    fn scalars_have_c_sizes() {
        let types = table();
        let layout = Layout::new(&types);
        assert_eq!(layout.size_of(TypeId::INT), 8);
        assert_eq!(layout.size_of(TypeId::FLOAT), 8);
        assert_eq!(layout.size_of(TypeId::BOOL), 1);
        assert_eq!(layout.align_of(TypeId::BOOL), 1);
        assert_eq!(layout.size_of(TypeId::STR), PTR_SIZE);
        assert_eq!(layout.align_of(TypeId::STR), PTR_SIZE);
        assert_eq!(layout.size_of(TypeId::UNIT), 0);
    }

    #[test]
    fn a_pointer_is_four_bytes_here_and_eight_natively() {
        let mut types = table();
        let list = types.list_of(TypeId::INT);
        let layout = Layout::new(&types);
        assert_eq!(layout.size_of(list), 4);
        // The header itself is not affected: its two lengths are `i64`s.
        assert_eq!(LIST_DATA_OFFSET, 16);
        assert_eq!(STR_HEADER, 16);
    }

    #[test]
    fn struct_layout_pads_to_alignment() {
        let mut types = table();
        // { bool, int } => bool at 0, int at 8, size 16.
        let pair = types.tuple_of(&[TypeId::BOOL, TypeId::INT]);
        let layout = Layout::new(&types);
        assert_eq!(layout.field_offsets(&[TypeId::BOOL, TypeId::INT]), [0, 8]);
        assert_eq!(layout.size_of(pair), 16);

        // { bool, str } => bool at 0, pointer at 4, size 8 on wasm32.
        let mixed = types.tuple_of(&[TypeId::BOOL, TypeId::STR]);
        let layout = Layout::new(&types);
        assert_eq!(layout.field_offsets(&[TypeId::BOOL, TypeId::STR]), [0, 4]);
        assert_eq!(layout.size_of(mixed), 8);

        // { bool, bool, bool } => three bytes, no padding at all.
        let flags = types.tuple_of(&[TypeId::BOOL, TypeId::BOOL, TypeId::BOOL]);
        let layout = Layout::new(&types);
        assert_eq!(layout.size_of(flags), 3);
    }

    #[test]
    fn nested_tuples_are_flattened_by_the_c_rules() {
        let mut types = table();
        let inner = types.tuple_of(&[TypeId::INT, TypeId::BOOL]); // size 16
        let outer = types.tuple_of(&[TypeId::BOOL, inner]);
        let layout = Layout::new(&types);
        assert_eq!(layout.size_of(inner), 16);
        assert_eq!(layout.field_offsets(&[TypeId::BOOL, inner]), [0, 8]);
        assert_eq!(layout.size_of(outer), 24);
    }

    #[test]
    fn an_empty_class_still_occupies_a_byte() {
        let mut types = table();
        let (class, _) = types.declare_class("Empty");
        types.set_fields(class, Vec::new());
        let layout = Layout::new(&types);
        assert_eq!(layout.class_size(class), 1);
        assert!(layout.class_offsets(class).is_empty());
    }
}
