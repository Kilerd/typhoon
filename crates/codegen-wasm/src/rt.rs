//! The runtime functions the generated module imports.
//!
//! A Typhoon wasm program is two modules (DESIGN §6.2.1): `crates/runtime`
//! compiled for `wasm32-unknown-unknown`, and the user module emitted here.
//! The user module imports the runtime's linear memory as `env.memory` and
//! every `ty_*` function it calls from `env`, so both halves work on the same
//! heap and the runtime is shared byte for byte with the native backend.
//!
//! The signatures below are the C ones of `crates/runtime/src/lib.rs` read
//! through the wasm32 C ABI: a pointer and a `usize` are `i32`, a `u8` is
//! `i32`, an `i64` is `i64` and an `f64` is `f64`. They are checked against the
//! real module at link time — an instantiation with a mismatched import fails
//! loudly — and the golden differential suite runs every program through it.

use wasm_encoder::ValType;

const I: ValType = ValType::I32;
const L: ValType = ValType::I64;
const F: ValType = ValType::F64;

/// One runtime entry point the generated module can import.
///
/// The order of the variants is the order the imports appear in the module,
/// which keeps the emitted binary (and its WAT snapshot) stable.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum Rt {
    /// `ty_rt_init()`: start the runtime.
    Init,
    /// `ty_rt_exit(code)`: flush stdout, tell the host, trap.
    Exit,
    /// `ty_panic(msg, len)`: abort with `panic: <msg>` and exit code 101.
    Panic,
    /// `ty_alloc(size)`: a traced allocation.
    Alloc,
    /// `ty_alloc_atomic(size)`: a pointer-free allocation.
    AllocAtomic,
    /// `ty_print_int(value)`.
    PrintInt,
    /// `ty_print_float(value)`.
    PrintFloat,
    /// `ty_print_bool(value)`.
    PrintBool,
    /// `ty_print_str(ptr, len)`: raw bytes, no quoting.
    PrintStr,
    /// `ty_print_str_repr(str)`: the quoted form used inside containers.
    PrintStrRepr,
    /// `ty_print_sep()`: the single space between `print` arguments.
    PrintSep,
    /// `ty_print_end()`: the trailing newline.
    PrintEnd,
    /// `ty_int_pow(base, exp)`.
    IntPow,
    /// `ty_float_pow(base, exp)`: what the native backend gets from
    /// `llvm.pow.f64`; wasm has no `pow` instruction.
    FloatPow,
    /// `ty_str_concat(a, b)`.
    StrConcat,
    /// `ty_str_eq(a, b)`.
    StrEq,
    /// `ty_str_cmp(a, b)`.
    StrCmp,
    /// `ty_str_contains(haystack, needle)`.
    StrContains,
    /// `ty_str_startswith(s, prefix)`.
    StrStartsWith,
    /// `ty_str_endswith(s, suffix)`.
    StrEndsWith,
    /// `ty_str_find(s, needle)`.
    StrFind,
    /// `ty_str_upper(s)`.
    StrUpper,
    /// `ty_str_lower(s)`.
    StrLower,
    /// `ty_str_strip(s)`.
    StrStrip,
    /// `ty_str_split(s, sep)`.
    StrSplit,
    /// `ty_str_join(sep, parts)`.
    StrJoin,
    /// `ty_str_replace(s, old, new)`.
    StrReplace,
    /// `ty_str_char_at(s, byte_offset)`.
    StrCharAt,
    /// `ty_str_from_int(value)`.
    StrFromInt,
    /// `ty_str_from_float(value)`.
    StrFromFloat,
    /// `ty_str_from_bool(value)`.
    StrFromBool,
    /// `ty_str_from_float_fixed(value, precision)`.
    StrFromFloatFixed,
    /// `ty_list_new(elem_size, atomic, cap)`.
    ListNew,
    /// `ty_list_grow(list, elem_size, atomic, needed)`.
    ListGrow,
    /// `ty_list_insert_slot(list, index, elem_size, atomic)`.
    ListInsertSlot,
    /// `ty_list_concat(a, b, elem_size, atomic)`.
    ListConcat,
}

impl Rt {
    /// Every runtime function, in import order.
    pub const ALL: [Rt; 36] = [
        Rt::Init,
        Rt::Exit,
        Rt::Panic,
        Rt::Alloc,
        Rt::AllocAtomic,
        Rt::PrintInt,
        Rt::PrintFloat,
        Rt::PrintBool,
        Rt::PrintStr,
        Rt::PrintStrRepr,
        Rt::PrintSep,
        Rt::PrintEnd,
        Rt::IntPow,
        Rt::FloatPow,
        Rt::StrConcat,
        Rt::StrEq,
        Rt::StrCmp,
        Rt::StrContains,
        Rt::StrStartsWith,
        Rt::StrEndsWith,
        Rt::StrFind,
        Rt::StrUpper,
        Rt::StrLower,
        Rt::StrStrip,
        Rt::StrSplit,
        Rt::StrJoin,
        Rt::StrReplace,
        Rt::StrCharAt,
        Rt::StrFromInt,
        Rt::StrFromFloat,
        Rt::StrFromBool,
        Rt::StrFromFloatFixed,
        Rt::ListNew,
        Rt::ListGrow,
        Rt::ListInsertSlot,
        Rt::ListConcat,
    ];

    /// The exported symbol name.
    pub fn name(self) -> &'static str {
        match self {
            Rt::Init => "ty_rt_init",
            Rt::Exit => "ty_rt_exit",
            Rt::Panic => "ty_panic",
            Rt::Alloc => "ty_alloc",
            Rt::AllocAtomic => "ty_alloc_atomic",
            Rt::PrintInt => "ty_print_int",
            Rt::PrintFloat => "ty_print_float",
            Rt::PrintBool => "ty_print_bool",
            Rt::PrintStr => "ty_print_str",
            Rt::PrintStrRepr => "ty_print_str_repr",
            Rt::PrintSep => "ty_print_sep",
            Rt::PrintEnd => "ty_print_end",
            Rt::IntPow => "ty_int_pow",
            Rt::FloatPow => "ty_float_pow",
            Rt::StrConcat => "ty_str_concat",
            Rt::StrEq => "ty_str_eq",
            Rt::StrCmp => "ty_str_cmp",
            Rt::StrContains => "ty_str_contains",
            Rt::StrStartsWith => "ty_str_startswith",
            Rt::StrEndsWith => "ty_str_endswith",
            Rt::StrFind => "ty_str_find",
            Rt::StrUpper => "ty_str_upper",
            Rt::StrLower => "ty_str_lower",
            Rt::StrStrip => "ty_str_strip",
            Rt::StrSplit => "ty_str_split",
            Rt::StrJoin => "ty_str_join",
            Rt::StrReplace => "ty_str_replace",
            Rt::StrCharAt => "ty_str_char_at",
            Rt::StrFromInt => "ty_str_from_int",
            Rt::StrFromFloat => "ty_str_from_float",
            Rt::StrFromBool => "ty_str_from_bool",
            Rt::StrFromFloatFixed => "ty_str_from_float_fixed",
            Rt::ListNew => "ty_list_new",
            Rt::ListGrow => "ty_list_grow",
            Rt::ListInsertSlot => "ty_list_insert_slot",
            Rt::ListConcat => "ty_list_concat",
        }
    }

    /// The wasm signature: `(params, results)`.
    pub fn signature(self) -> (&'static [ValType], &'static [ValType]) {
        match self {
            Rt::Init | Rt::PrintSep | Rt::PrintEnd => (&[], &[]),
            Rt::Exit | Rt::PrintBool | Rt::PrintStrRepr => (&[I], &[]),
            Rt::Panic | Rt::PrintStr => (&[I, I], &[]),
            Rt::Alloc | Rt::AllocAtomic => (&[I], &[I]),
            Rt::PrintInt => (&[L], &[]),
            Rt::PrintFloat => (&[F], &[]),
            Rt::IntPow => (&[L, L], &[L]),
            Rt::FloatPow => (&[F, F], &[F]),
            Rt::StrConcat
            | Rt::StrSplit
            | Rt::StrJoin
            | Rt::StrEq
            | Rt::StrContains
            | Rt::StrStartsWith
            | Rt::StrEndsWith => (&[I, I], &[I]),
            Rt::StrCmp | Rt::StrFind => (&[I, I], &[L]),
            Rt::StrUpper | Rt::StrLower | Rt::StrStrip | Rt::StrFromBool => (&[I], &[I]),
            Rt::StrReplace => (&[I, I, I], &[I]),
            Rt::StrCharAt => (&[I, L], &[I]),
            Rt::StrFromInt => (&[L], &[I]),
            Rt::StrFromFloat => (&[F], &[I]),
            Rt::StrFromFloatFixed => (&[F, L], &[I]),
            Rt::ListNew => (&[L, I, L], &[I]),
            Rt::ListGrow => (&[I, L, I, L], &[]),
            Rt::ListInsertSlot => (&[I, L, L, I], &[I]),
            Rt::ListConcat => (&[I, I, L, I], &[I]),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_variant_is_listed_once() {
        let mut names: Vec<&str> = Rt::ALL.iter().map(|r| r.name()).collect();
        let count = names.len();
        names.sort_unstable();
        names.dedup();
        assert_eq!(names.len(), count, "duplicate runtime name");
        assert_eq!(count, Rt::ALL.len());
    }

    #[test]
    fn names_are_the_c_abi_symbols() {
        // Every import must be a `ty_*` symbol the runtime exports; a typo
        // here would only show up as a failed instantiation.
        for rt in Rt::ALL {
            assert!(rt.name().starts_with("ty_"), "{}", rt.name());
        }
        assert_eq!(Rt::AllocAtomic.name(), "ty_alloc_atomic");
        assert_eq!(Rt::ListInsertSlot.signature().0.len(), 4);
    }

    #[test]
    fn pointer_arguments_are_i32() {
        // `ty_print_str(ptr, len)` takes two wasm32 pointers-sized values.
        assert_eq!(Rt::PrintStr.signature(), (&[I, I][..], &[][..]));
        // `ty_alloc(size) -> ptr`.
        assert_eq!(Rt::Alloc.signature(), (&[I][..], &[I][..]));
        // `ty_str_char_at(str, byte_offset: i64) -> str`.
        assert_eq!(Rt::StrCharAt.signature(), (&[I, L][..], &[I][..]));
    }
}
