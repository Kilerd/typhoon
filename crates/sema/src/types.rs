//! The type table: every type the checker knows is interned into a
//! [`TypeId`] (DESIGN §6.1, "类型经 interning 表示为 `TypeId`").
//!
//! M2 adds the data types of DESIGN §4.1 to M1's scalars: `list<T>`,
//! `tuple<A, B, …>`, user classes, and the nullable class reference
//! `C | None` (an early slice of M3's `T | None`, see DESIGN §3.8). Composite
//! types stay [`Copy`] by referring to side tables:
//!
//! * a `tuple` holds a [`TupleId`] into [`TypeTable::tuple_members`];
//! * a class holds a [`ClassId`] into [`TypeTable::class`].

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
    /// `list<T>`, a growable array of `T` on the GC heap (DESIGN §4.1).
    List(TypeId),
    /// `tuple<A, B, …>`, a fixed-length value aggregate.
    Tuple(TupleId),
    /// An instance of a user class, a GC reference (DESIGN §3.8).
    Class(ClassId),
    /// `C | None`: a class reference that may be absent, represented as a
    /// possibly-null pointer. Narrowed by `is None` (DESIGN §3.8).
    Optional(ClassId),
}

/// A handle into a [`TypeTable`].
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct TypeId(pub u32);

/// A handle to one tuple's member list inside a [`TypeTable`].
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct TupleId(pub u32);

impl TupleId {
    /// The raw index.
    pub fn index(self) -> usize {
        self.0 as usize
    }
}

/// A handle to one user class inside a [`TypeTable`].
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct ClassId(pub u32);

impl ClassId {
    /// The raw index.
    pub fn index(self) -> usize {
        self.0 as usize
    }
}

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

/// One field of a user class, in declaration order.
#[derive(Debug, Clone)]
pub struct FieldInfo {
    /// The field name as written.
    pub name: String,
    /// The field type.
    pub ty: TypeId,
}

/// The layout and members of one user class (DESIGN §3.8).
#[derive(Debug, Clone)]
pub struct ClassInfo {
    /// The class name as written.
    pub name: String,
    /// The fields, in declaration order; the generated keyword constructor
    /// takes exactly these.
    pub fields: Vec<FieldInfo>,
    /// Whether any field can hold a GC pointer. An instance whose fields are
    /// all pointer-free is allocated with `ty_alloc_atomic` and never scanned
    /// (DESIGN §5.1).
    pub has_pointers: bool,
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
///
/// let ints = types.list_of(TypeId::INT);
/// assert_eq!(types.name(ints), "list<int>");
/// let pair = types.tuple_of(&[TypeId::INT, TypeId::STR]);
/// assert_eq!(types.name(pair), "tuple<int, str>");
/// ```
#[derive(Debug, Clone)]
pub struct TypeTable {
    types: Vec<Type>,
    index: HashMap<Type, TypeId>,
    tuples: Vec<Vec<TypeId>>,
    tuple_index: HashMap<Vec<TypeId>, TupleId>,
    classes: Vec<ClassInfo>,
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
            tuples: Vec::new(),
            tuple_index: HashMap::new(),
            classes: Vec::new(),
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

    // -- composite types ---------------------------------------------------

    /// Interns `list<elem>`.
    pub fn list_of(&mut self, elem: TypeId) -> TypeId {
        self.intern(Type::List(elem))
    }

    /// Interns `tuple<members…>`.
    ///
    /// # Panics
    ///
    /// Panics if `members` is empty; M2 has no unit tuple (`()` is rejected by
    /// the checker before this point).
    pub fn tuple_of(&mut self, members: &[TypeId]) -> TypeId {
        assert!(!members.is_empty(), "M2 has no zero-length tuple");
        let id = match self.tuple_index.get(members) {
            Some(id) => *id,
            None => {
                let id = TupleId(self.tuples.len() as u32);
                self.tuples.push(members.to_vec());
                self.tuple_index.insert(members.to_vec(), id);
                id
            }
        };
        self.intern(Type::Tuple(id))
    }

    /// The member types of a tuple.
    pub fn tuple_members(&self, id: TupleId) -> &[TypeId] {
        &self.tuples[id.index()]
    }

    /// The member types of `ty` when it is a tuple.
    pub fn as_tuple(&self, ty: TypeId) -> Option<&[TypeId]> {
        match self.get(ty) {
            Type::Tuple(id) => Some(self.tuple_members(id)),
            _ => None,
        }
    }

    /// The element type of `ty` when it is a `list<T>`.
    pub fn as_list(&self, ty: TypeId) -> Option<TypeId> {
        match self.get(ty) {
            Type::List(elem) => Some(elem),
            _ => None,
        }
    }

    // -- classes -----------------------------------------------------------

    /// Registers a class by name with no fields yet, returning its id and the
    /// [`TypeId`] of instances of it.
    ///
    /// Field types are filled in afterwards with [`TypeTable::set_fields`], so
    /// that classes may refer to each other and to themselves.
    pub fn declare_class(&mut self, name: &str) -> (ClassId, TypeId) {
        let id = ClassId(self.classes.len() as u32);
        self.classes.push(ClassInfo {
            name: name.to_string(),
            fields: Vec::new(),
            has_pointers: false,
        });
        let ty = self.intern(Type::Class(id));
        (id, ty)
    }

    /// Fills in the fields of a class declared by [`TypeTable::declare_class`].
    pub fn set_fields(&mut self, id: ClassId, fields: Vec<FieldInfo>) {
        let has_pointers = fields.iter().any(|f| !self.is_pointer_free(f.ty));
        let info = &mut self.classes[id.index()];
        info.fields = fields;
        info.has_pointers = has_pointers;
    }

    /// The class behind `id`.
    pub fn class(&self, id: ClassId) -> &ClassInfo {
        &self.classes[id.index()]
    }

    /// Every declared class, in declaration order.
    pub fn classes(&self) -> &[ClassInfo] {
        &self.classes
    }

    /// The id of the field named `name`, when the class has one.
    pub fn field_index(&self, id: ClassId, name: &str) -> Option<u32> {
        self.class(id)
            .fields
            .iter()
            .position(|f| f.name == name)
            .map(|i| i as u32)
    }

    /// The type instances of `id` have.
    pub fn class_ty(&mut self, id: ClassId) -> TypeId {
        self.intern(Type::Class(id))
    }

    /// The type of `C | None` for the class `id`.
    pub fn optional_ty(&mut self, id: ClassId) -> TypeId {
        self.intern(Type::Optional(id))
    }

    // -- classification ----------------------------------------------------

    /// Whether a value of `ty` can never contain a pointer the GC must trace.
    ///
    /// Pointer-free payloads are allocated with `ty_alloc_atomic`, which the
    /// collector does not scan (DESIGN §5.1).
    pub fn is_pointer_free(&self, ty: TypeId) -> bool {
        match self.get(ty) {
            Type::Error | Type::Unit | Type::Int | Type::Float | Type::Bool => true,
            Type::Str | Type::List(_) | Type::Class(_) | Type::Optional(_) => false,
            Type::Tuple(id) => self
                .tuple_members(id)
                .iter()
                .all(|m| self.is_pointer_free(*m)),
        }
    }

    /// Whether `print` and `==` are defined for `ty`.
    ///
    /// M2 has no repr or equality protocol for class instances, so a class —
    /// and anything containing one — is neither printable nor comparable
    /// (DESIGN §3.8; the protocols arrive in M3).
    pub fn is_printable(&self, ty: TypeId) -> bool {
        match self.get(ty) {
            Type::Int | Type::Float | Type::Bool | Type::Str => true,
            Type::List(elem) => self.is_printable(elem),
            Type::Tuple(id) => self.tuple_members(id).iter().all(|m| self.is_printable(*m)),
            Type::Error | Type::Unit | Type::Class(_) | Type::Optional(_) => false,
        }
    }

    /// Whether `ty` is a reference to a GC object, i.e. an LLVM `ptr`.
    pub fn is_reference(&self, ty: TypeId) -> bool {
        matches!(
            self.get(ty),
            Type::Str | Type::List(_) | Type::Class(_) | Type::Optional(_)
        )
    }

    /// The class a value of `ty` refers to, whether or not it may be `None`.
    pub fn class_of(&self, ty: TypeId) -> Option<ClassId> {
        match self.get(ty) {
            Type::Class(id) | Type::Optional(id) => Some(id),
            _ => None,
        }
    }

    /// The name of `id` as it is spelled in source and in diagnostics.
    pub fn name(&self, id: TypeId) -> String {
        match self.get(id) {
            Type::Error => "{unknown}".to_string(),
            Type::Unit => "None".to_string(),
            Type::Int => "int".to_string(),
            Type::Float => "float".to_string(),
            Type::Bool => "bool".to_string(),
            Type::Str => "str".to_string(),
            Type::List(elem) => format!("list<{}>", self.name(elem)),
            Type::Tuple(members) => {
                let inner: Vec<String> = self
                    .tuple_members(members)
                    .iter()
                    .map(|m| self.name(*m))
                    .collect();
                format!("tuple<{}>", inner.join(", "))
            }
            Type::Class(c) => self.class(c).name.clone(),
            Type::Optional(c) => format!("{} | None", self.class(c).name),
        }
    }
}

/// The source spelling of a scalar type, used where no table is at hand.
pub fn type_name(ty: Type) -> &'static str {
    match ty {
        Type::Error => "{unknown}",
        Type::Unit => "None",
        Type::Int => "int",
        Type::Float => "float",
        Type::Bool => "bool",
        Type::Str => "str",
        Type::List(_) => "list",
        Type::Tuple(_) => "tuple",
        Type::Class(_) => "class",
        Type::Optional(_) => "optional",
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

    #[test]
    fn lists_and_tuples_intern_structurally() {
        let mut types = TypeTable::new();
        let a = types.list_of(TypeId::INT);
        let b = types.list_of(TypeId::INT);
        assert_eq!(a, b);
        let nested = types.list_of(a);
        assert_eq!(types.name(nested), "list<list<int>>");
        assert_eq!(types.as_list(nested), Some(a));
        assert_eq!(types.as_list(TypeId::INT), None);

        let t1 = types.tuple_of(&[TypeId::INT, TypeId::STR]);
        let t2 = types.tuple_of(&[TypeId::INT, TypeId::STR]);
        let t3 = types.tuple_of(&[TypeId::STR, TypeId::INT]);
        assert_eq!(t1, t2);
        assert_ne!(t1, t3);
        assert_eq!(types.name(t3), "tuple<str, int>");
        assert_eq!(types.as_tuple(t1), Some(&[TypeId::INT, TypeId::STR][..]));
    }

    #[test]
    #[should_panic(expected = "zero-length tuple")]
    fn the_empty_tuple_is_rejected() {
        TypeTable::new().tuple_of(&[]);
    }

    #[test]
    fn classes_may_refer_to_themselves() {
        let mut types = TypeTable::new();
        let (node, node_ty) = types.declare_class("Node");
        let optional = types.optional_ty(node);
        types.set_fields(
            node,
            vec![
                FieldInfo {
                    name: "value".to_string(),
                    ty: TypeId::INT,
                },
                FieldInfo {
                    name: "next".to_string(),
                    ty: optional,
                },
            ],
        );
        assert_eq!(types.name(node_ty), "Node");
        assert_eq!(types.name(optional), "Node | None");
        assert_eq!(types.field_index(node, "next"), Some(1));
        assert_eq!(types.field_index(node, "missing"), None);
        assert!(types.class(node).has_pointers);
        assert_eq!(types.class_of(optional), Some(node));
        assert_eq!(types.class_of(TypeId::INT), None);
        assert_eq!(types.classes().len(), 1);
        assert_eq!(types.class_ty(node), node_ty);
    }

    #[test]
    fn pointer_freedom_follows_the_members() {
        let mut types = TypeTable::new();
        assert!(types.is_pointer_free(TypeId::INT));
        assert!(types.is_pointer_free(TypeId::BOOL));
        assert!(!types.is_pointer_free(TypeId::STR));
        let ints = types.list_of(TypeId::INT);
        assert!(!types.is_pointer_free(ints));
        let scalars = types.tuple_of(&[TypeId::INT, TypeId::FLOAT]);
        assert!(types.is_pointer_free(scalars));
        let with_str = types.tuple_of(&[TypeId::INT, TypeId::STR]);
        assert!(!types.is_pointer_free(with_str));

        let (point, point_ty) = types.declare_class("Point");
        types.set_fields(
            point,
            vec![FieldInfo {
                name: "x".to_string(),
                ty: TypeId::FLOAT,
            }],
        );
        assert!(!types.class(point).has_pointers);
        assert!(!types.is_pointer_free(point_ty));
        assert!(types.is_reference(point_ty));
        assert!(!types.is_reference(TypeId::INT));
    }

    #[test]
    fn printability_stops_at_class_instances() {
        let mut types = TypeTable::new();
        assert!(types.is_printable(TypeId::STR));
        assert!(!types.is_printable(TypeId::UNIT));
        let strs = types.list_of(TypeId::STR);
        assert!(types.is_printable(strs));

        let (point, point_ty) = types.declare_class("Point");
        types.set_fields(point, vec![]);
        assert!(!types.is_printable(point_ty));
        let points = types.list_of(point_ty);
        assert!(!types.is_printable(points));
        let pair = types.tuple_of(&[TypeId::INT, point_ty]);
        assert!(!types.is_printable(pair));
    }

    #[test]
    fn scalar_names_are_available_without_a_table() {
        assert_eq!(type_name(Type::Int), "int");
        assert_eq!(type_name(Type::List(TypeId::INT)), "list");
        assert_eq!(type_name(Type::Class(ClassId(0))), "class");
        assert_eq!(type_name(Type::Optional(ClassId(0))), "optional");
        assert_eq!(type_name(Type::Tuple(TupleId(0))), "tuple");
        assert_eq!(TupleId(3).index(), 3);
        assert_eq!(ClassId(2).index(), 2);
    }
}
