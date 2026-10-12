//! Lowering of the M3 containers: `dict<K, V>`, `set<T>`, `sorted(...)` and
//! `str` slicing (DESIGN §4.1, §4.6).
//!
//! The wasm twin of `crates/codegen/src/emit/dict.rs`, making the same runtime
//! calls in the same order. A dict or a set is the `i32` address of the
//! runtime's table header; `len` is read inline and everything else is a
//! `ty_dict_*` call that returns the address of a value slot.
//!
//! The key word is an `i64`: an `int` as is, a `bool` or a `str` address
//! zero-extended, and a tuple as the address of [`Rt::KeyBuf`] — the runtime's
//! static key buffer — after its members have been stored there. The native
//! backend uses a stack slot instead; wasm has no addressable stack, and the
//! buffer is filled immediately before the one call that reads it.

use typhoon_sema::hir;
use typhoon_sema::types::{Type, TypeId};
use wasm_encoder::{BlockType, Instruction, MemArg, ValType};

use super::Emitter;
use crate::rt::Rt;

impl<'p> Emitter<'p> {
    // -- keys and slots --------------------------------------------------------

    /// The runtime descriptor of the key (or `sorted` element) type `ty`.
    fn desc_of(&self, ty: TypeId) -> i64 {
        self.types
            .key_desc(ty)
            .expect("sema only lets hashable / sortable types through") as i64
    }

    /// Pushes the `i64` key word of the spilled key `value` of type `ty`.
    fn push_key_word(&mut self, ty: TypeId, value: &[u32]) {
        match self.types.get(ty) {
            Type::Int => self.ins(Instruction::LocalGet(value[0])),
            Type::Bool | Type::Str => {
                self.ins(Instruction::LocalGet(value[0]));
                self.ins(Instruction::I64ExtendI32U);
            }
            _ => {
                self.call_rt(Rt::KeyBuf);
                let buf = self.scratch(ValType::I32);
                self.ins(Instruction::LocalSet(buf));
                self.store_value(buf, ty, 0, value);
                self.ins(Instruction::LocalGet(buf));
                self.ins(Instruction::I64ExtendI32U);
            }
        }
    }

    /// `rt(table, key word)`, leaving the returned slot address in a fresh
    /// local.
    fn table_call(&mut self, rt: Rt, table: u32, key_ty: TypeId, key: &[u32]) -> u32 {
        self.ins(Instruction::LocalGet(table));
        self.push_key_word(key_ty, key);
        self.call_rt(rt);
        let slot = self.scratch(ValType::I32);
        self.ins(Instruction::LocalSet(slot));
        slot
    }

    // -- construction ----------------------------------------------------------

    /// `ty_dict_new(desc, val_size, val_align, val_atomic)`, into a fresh local.
    fn table_alloc(&mut self, key: TypeId, value: Option<TypeId>) -> u32 {
        let (size, align, atomic) = match value {
            Some(value) => (
                self.layout.size_of(value),
                self.layout.align_of(value),
                self.atomic_flag(value),
            ),
            None => (0, 1, 1),
        };
        self.ins(Instruction::I64Const(self.desc_of(key)));
        self.ins(Instruction::I64Const(i64::from(size)));
        self.ins(Instruction::I64Const(i64::from(align)));
        self.ins(Instruction::I32Const(atomic));
        self.call_rt(Rt::DictNew);
        let table = self.scratch(ValType::I32);
        self.ins(Instruction::LocalSet(table));
        table
    }

    /// A dict literal: one allocation, then every entry inserted in order.
    pub(crate) fn dict_new(
        &mut self,
        func: &'p hir::Function,
        key: TypeId,
        value: TypeId,
        entries: &[(hir::Expr, hir::Expr)],
    ) {
        let table = self.table_alloc(key, Some(value));
        for (k, v) in entries {
            let k_value = self.spill_expr(func, k);
            let v_value = self.spill_expr(func, v);
            let slot = self.table_call(Rt::DictInsert, table, key, &k_value);
            self.store_value(slot, value, 0, &v_value);
        }
        self.ins(Instruction::LocalGet(table));
    }

    /// A set literal or `set()`.
    pub(crate) fn set_new(&mut self, func: &'p hir::Function, elem: TypeId, items: &[hir::Expr]) {
        let table = self.table_alloc(elem, None);
        for item in items {
            let value = self.spill_expr(func, item);
            self.table_call(Rt::DictInsert, table, elem, &value);
        }
        self.ins(Instruction::LocalGet(table));
    }

    // -- access ------------------------------------------------------------------

    /// `d[k]`: panics in the runtime when the key is absent.
    pub(crate) fn dict_get(
        &mut self,
        func: &'p hir::Function,
        dict: &hir::Expr,
        key: &hir::Expr,
        value_ty: TypeId,
    ) {
        let table = self.spill_expr(func, dict)[0];
        let k = self.spill_expr(func, key);
        let slot = self.table_call(Rt::DictGet, table, key.ty, &k);
        self.load_value(slot, value_ty, 0);
    }

    /// Pushes the value in `slot`, or the spilled `default` when the runtime
    /// returned no slot.
    fn slot_or_default(&mut self, slot: u32, value_ty: TypeId, default: &[u32]) {
        let flat = self.flat(value_ty);
        let result: Vec<u32> = flat.iter().map(|vt| self.scratch(*vt)).collect();
        self.ins(Instruction::LocalGet(slot));
        self.ins(Instruction::I32Eqz);
        self.open_if(BlockType::Empty);
        self.push_locals(default);
        for local in result.iter().rev() {
            self.ins(Instruction::LocalSet(*local));
        }
        self.else_();
        self.load_value(slot, value_ty, 0);
        for local in result.iter().rev() {
            self.ins(Instruction::LocalSet(*local));
        }
        self.end();
        self.push_locals(&result);
    }

    /// `d.get(k, default)`; the default is evaluated either way, as in Python.
    pub(crate) fn dict_get_or(
        &mut self,
        func: &'p hir::Function,
        dict: &hir::Expr,
        key: &hir::Expr,
        default: &hir::Expr,
        value_ty: TypeId,
    ) {
        let table = self.spill_expr(func, dict)[0];
        let k = self.spill_expr(func, key);
        let fallback = self.spill_expr(func, default);
        let slot = self.table_call(Rt::DictFind, table, key.ty, &k);
        self.slot_or_default(slot, value_ty, &fallback);
    }

    /// `d.pop(k)` / `d.pop(k, default)`.
    pub(crate) fn dict_pop(
        &mut self,
        func: &'p hir::Function,
        dict: &hir::Expr,
        key: &hir::Expr,
        default: Option<&hir::Expr>,
        value_ty: TypeId,
    ) {
        let table = self.spill_expr(func, dict)[0];
        let k = self.spill_expr(func, key);
        let fallback = default.map(|d| self.spill_expr(func, d));
        match fallback {
            Some(fallback) => {
                let slot = self.table_call(Rt::DictRemove, table, key.ty, &k);
                self.slot_or_default(slot, value_ty, &fallback);
            }
            None => {
                let slot = self.table_call(Rt::DictPop, table, key.ty, &k);
                self.load_value(slot, value_ty, 0);
            }
        }
    }

    /// `d[k] = v`.
    pub(crate) fn emit_dict_set(
        &mut self,
        func: &'p hir::Function,
        dict: &hir::Expr,
        key: &hir::Expr,
        value: &hir::Expr,
    ) {
        let table = self.spill_expr(func, dict)[0];
        let k = self.spill_expr(func, key);
        let v = self.spill_expr(func, value);
        let slot = self.table_call(Rt::DictInsert, table, key.ty, &k);
        self.store_value(slot, value.ty, 0, &v);
    }

    /// `k in d` / `x in s`, and their `not in` forms.
    pub(crate) fn hash_contains(
        &mut self,
        func: &'p hir::Function,
        table: &hir::Expr,
        key: &hir::Expr,
        negated: bool,
    ) {
        let t = self.spill_expr(func, table)[0];
        let k = self.spill_expr(func, key);
        let slot = self.table_call(Rt::DictFind, t, key.ty, &k);
        self.ins(Instruction::LocalGet(slot));
        if negated {
            self.ins(Instruction::I32Eqz);
        } else {
            self.ins(Instruction::I32Const(0));
            self.ins(Instruction::I32Ne);
        }
    }

    /// `s.add(x)`, `s.remove(x)` and `s.discard(x)`.
    pub(crate) fn set_update(
        &mut self,
        func: &'p hir::Function,
        set: &hir::Expr,
        value: &hir::Expr,
        rt: Rt,
    ) {
        let t = self.spill_expr(func, set)[0];
        let v = self.spill_expr(func, value);
        self.table_call(rt, t, value.ty, &v);
    }

    /// Replaces the table on the stack with its length.
    pub(crate) fn hash_len_on_stack(&mut self) {
        self.ins(Instruction::I64Load(MemArg {
            offset: 0,
            align: 3,
            memory_index: 0,
        }));
    }

    // -- builtins ------------------------------------------------------------------

    /// `sorted(xs)`.
    pub(crate) fn sorted(&mut self, func: &'p hir::Function, list: &hir::Expr) {
        let elem = self.types.as_list(list.ty).expect("`sorted` takes a list");
        self.expr(func, list);
        self.ins(Instruction::I64Const(self.desc_of(elem)));
        self.call_rt(Rt::ListSorted);
    }

    /// `s[a:b]`.
    pub(crate) fn str_slice(
        &mut self,
        func: &'p hir::Function,
        string: &hir::Expr,
        start: &hir::Expr,
        stop: &hir::Expr,
    ) {
        self.expr(func, string);
        self.expr(func, start);
        self.expr(func, stop);
        self.call_rt(Rt::StrSlice);
    }

    // -- printing --------------------------------------------------------------------

    /// `{"a": 1, "b": 2}` (and `{}`): the keys and the values are fetched as
    /// two lists and written pairwise.
    pub(crate) fn print_dict(&mut self, key: TypeId, value: TypeId, dict: u32) {
        self.ins(Instruction::LocalGet(dict));
        self.call_rt(Rt::DictKeys);
        let keys = self.scratch(ValType::I32);
        self.ins(Instruction::LocalSet(keys));
        self.ins(Instruction::LocalGet(dict));
        self.call_rt(Rt::DictValues);
        let values = self.scratch(ValType::I32);
        self.ins(Instruction::LocalSet(values));
        self.print_raw("{");
        self.print_elements(keys, key, Some((values, value)));
        self.print_raw("}");
    }

    /// `{1, 2}`, and `set()` for the empty set.
    pub(crate) fn print_set(&mut self, elem: TypeId, set: u32) {
        self.ins(Instruction::LocalGet(set));
        self.call_rt(Rt::DictKeys);
        let keys = self.scratch(ValType::I32);
        self.ins(Instruction::LocalSet(keys));
        self.list_len(keys);
        self.ins(Instruction::I64Eqz);
        self.open_if(BlockType::Empty);
        self.print_raw("set()");
        self.else_();
        self.print_raw("{");
        self.print_elements(keys, elem, None);
        self.print_raw("}");
        self.end();
    }

    /// Writes the elements of `list` separated by `, ` — or, with `values`,
    /// `key: value` pairs taken from two parallel lists.
    fn print_elements(&mut self, list: u32, elem: TypeId, values: Option<(u32, TypeId)>) {
        let len = self.scratch(ValType::I64);
        self.list_len(list);
        self.ins(Instruction::LocalSet(len));
        let cursor = self.scratch(ValType::I64);
        self.ins(Instruction::I64Const(0));
        self.ins(Instruction::LocalSet(cursor));

        let done = self.open_block(BlockType::Empty);
        let head = self.open_loop(BlockType::Empty);

        self.ins(Instruction::LocalGet(cursor));
        self.ins(Instruction::LocalGet(len));
        self.ins(Instruction::I64LtS);
        self.ins(Instruction::I32Eqz);
        self.br_if(done);

        self.ins(Instruction::LocalGet(cursor));
        self.ins(Instruction::I64Const(0));
        self.ins(Instruction::I64Ne);
        self.open_if(BlockType::Empty);
        self.print_raw(", ");
        self.end();

        let data = self.list_data(list);
        let slot = self.elem_slot(data, elem, cursor);
        self.load_value(slot, elem, 0);
        let key = self.spill(elem);
        let pair = values.map(|(values, value_ty)| {
            let data = self.list_data(values);
            let slot = self.elem_slot(data, value_ty, cursor);
            self.load_value(slot, value_ty, 0);
            (self.spill(value_ty), value_ty)
        });
        self.ins(Instruction::LocalGet(cursor));
        self.ins(Instruction::I64Const(1));
        self.ins(Instruction::I64Add);
        self.ins(Instruction::LocalSet(cursor));
        self.print_value(elem, &key, true);
        if let Some((value, value_ty)) = pair {
            self.print_raw(": ");
            self.print_value(value_ty, &value, true);
        }
        self.br(head);

        self.end();
        self.end();
    }
}
