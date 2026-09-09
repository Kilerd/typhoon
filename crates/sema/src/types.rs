//! The type table: every type the checker knows is interned into a
//! [`TypeId`] (DESIGN §6.1, "类型经 interning 表示为 `TypeId`").
//!
//! M0/M1 only has the scalar types of DESIGN §4.1 plus the unit type, so the
//! table is tiny; it exists in this shape so that `list<T>`, `dict<K, V>` and
//! user classes can be added in M2 without touching every use site.

use std::collections::HashMap;

/// A structural type, the key the [`TypeTable`] interns.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Type {
    /// A type that could not be determined; a diagnostic was already emitted.
    ///
    /// Every operation on an `Error` operand silently yields `Error` again, so
    /// one mistake produces exactly one diagnostic instead of a cascade.
    Error,
    /// The unit type, written `None` (DESIGN §4.1).
    Unit,
    /// `int`, a wrapping 64-bit signed integer.
    Int,
    /// `float`, an IEEE-754 binary64.
    Float,
    /// `bool`, `True` or `False`, with no truthiness conversions.
    Bool,
    /// `str`, an immutable UTF-8 string on the GC heap.
    Str,
}

/// A handle into a [`TypeTable`].
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct TypeId(pub u32);

impl TypeId {
    /// The error type; always interned first.
    pub const ERROR: TypeId = TypeId(0);
    /// The unit type `None`.
    pub const UNIT: TypeId = TypeId(1);
    /// `int`.
    pub const INT: TypeId = TypeId(2);
    /// `float`.
    pub const FLOAT: TypeId = TypeId(3);
    /// `bool`.
    pub const BOOL: TypeId = TypeId(4);
    /// `str`.
    pub const STR: TypeId = TypeId(5);

    /// The raw index of this type inside its table.
    pub fn index(self) -> usize {
        self.0 as usize
    }
}

/// The interning table mapping [`Type`]s to [`TypeId`]s and back.
///
/// The scalar types are pre-interned in a fixed order, so the associated
/// constants of [`TypeId`] are valid for every table:
///
/// ```
/// use typhoon_sema::types::{Type, TypeId, TypeTable};
///
/// let mut types = TypeTable::new();
/// assert_eq!(types.intern(Type::Int), TypeId::INT);
/// assert_eq!(types.get(TypeId::STR), Type::Str);
/// assert_eq!(types.name(TypeId::FLOAT), "float");
/// ```
#[derive(Debug, Clone)]
pub struct TypeTable {
    types: Vec<Type>,
    index: HashMap<Type, TypeId>,
}

impl Default for TypeTable {
    fn default() -> Self {
        Self::new()
    }
}

impl TypeTable {
    /// Creates a table with the built-in types already interned.
    pub fn new() -> TypeTable {
        let mut table = TypeTable {
            types: Vec::new(),
            index: HashMap::new(),
        };
        for ty in [
            Type::Error,
            Type::Unit,
            Type::Int,
            Type::Float,
            Type::Bool,
            Type::Str,
        ] {
            table.intern(ty);
        }
        table
    }

    /// Interns `ty`, returning the existing id when it is already known.
    pub fn intern(&mut self, ty: Type) -> TypeId {
        if let Some(id) = self.index.get(&ty) {
            return *id;
        }
        let id = TypeId(self.types.len() as u32);
        self.types.push(ty);
        self.index.insert(ty, id);
        id
    }

    /// The type behind `id`.
    ///
    /// # Panics
    ///
    /// Panics if `id` does not come from this table.
    pub fn get(&self, id: TypeId) -> Type {
        self.types[id.index()]
    }

    /// Number of interned types.
    pub fn len(&self) -> usize {
        self.types.len()
    }

    /// Whether the table is empty; never true for a table from [`TypeTable::new`].
    pub fn is_empty(&self) -> bool {
        self.types.is_empty()
    }

    /// The name of `id` as it is spelled in source and in diagnostics.
    pub fn name(&self, id: TypeId) -> &'static str {
        type_name(self.get(id))
    }
}

/// The source spelling of a type, used in every diagnostic.
pub fn type_name(ty: Type) -> &'static str {
    match ty {
        Type::Error => "{unknown}",
        Type::Unit => "None",
        Type::Int => "int",
        Type::Float => "float",
        Type::Bool => "bool",
        Type::Str => "str",
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn builtin_ids_are_stable() {
        let types = TypeTable::new();
        assert_eq!(types.get(TypeId::ERROR), Type::Error);
        assert_eq!(types.get(TypeId::UNIT), Type::Unit);
        assert_eq!(types.get(TypeId::INT), Type::Int);
        assert_eq!(types.get(TypeId::FLOAT), Type::Float);
        assert_eq!(types.get(TypeId::BOOL), Type::Bool);
        assert_eq!(types.get(TypeId::STR), Type::Str);
        assert_eq!(types.len(), 6);
        assert!(!types.is_empty());
    }

    #[test]
    fn interning_is_idempotent() {
        let mut types = TypeTable::new();
        let before = types.len();
        assert_eq!(types.intern(Type::Bool), TypeId::BOOL);
        assert_eq!(types.intern(Type::Bool), TypeId::BOOL);
        assert_eq!(types.len(), before);
    }

    #[test]
    fn names_match_the_source_spelling() {
        let types = TypeTable::new();
        assert_eq!(types.name(TypeId::INT), "int");
        assert_eq!(types.name(TypeId::FLOAT), "float");
        assert_eq!(types.name(TypeId::BOOL), "bool");
        assert_eq!(types.name(TypeId::STR), "str");
        assert_eq!(types.name(TypeId::UNIT), "None");
        assert_eq!(types.name(TypeId::ERROR), "{unknown}");
    }

    #[test]
    fn ids_index_their_table() {
        assert_eq!(TypeId::INT.index(), 2);
        let mut types = TypeTable::default();
        assert_eq!(types.intern(Type::Str).index(), 5);
    }
}
