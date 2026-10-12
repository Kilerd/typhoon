//! Lowering of the M3 containers: `dict<K, V>`, `set<T>`, `sorted(...)` and
//! `str` slicing (DESIGN §4.1, §4.6).
//!
//! A dict or a set is a pointer to the runtime's `TyDict` header
//! (`crates/runtime/src/dict.rs`). Only `len` — the first field — is read
//! inline; every other operation is one runtime call that hashes the key and
//! hands back a pointer to the value slot, which is then loaded or stored
//! here with the value's ordinary memory layout. A set is the same table with
//! zero-byte values.
//!
//! A key crosses the C ABI as one `i64` **key word**: an `int` is itself, a
//! `bool` is zero-extended, a `str` is its address, and a tuple is the address
//! of a stack slot holding it (laid out exactly like a tuple in a list).
//! The key type is described to the runtime by the descriptor
//! [`typhoon_sema::types::TypeTable::key_desc`] computes.

use std::fmt::Write;

use typhoon_sema::hir;
use typhoon_sema::types::{Type, TypeId};

use super::{Emitter, Tbaa};

impl<'p> Emitter<'p> {
    // -- keys and slots --------------------------------------------------------

    /// The runtime descriptor of the key (or `sorted` element) type `ty`.
    fn desc_of(&self, ty: TypeId) -> u64 {
        self.types
            .key_desc(ty)
            .expect("sema only lets hashable / sortable types through")
    }

    /// Turns a key value into the `i64` key word the runtime takes.
    fn key_word(&mut self, ty: TypeId, value: &str) -> String {
        match self.types.get(ty) {
            Type::Int => value.to_string(),
            Type::Bool => {
                let out = self.fresh();
                self.emit(&format!("{out} = zext i1 {value} to i64"));
                out
            }
            Type::Str => {
                let out = self.fresh();
                self.emit(&format!("{out} = ptrtoint ptr {value} to i64"));
                out
            }
            _ => {
                // A tuple key is passed by address: a stack slot laid out like
                // the tuple in memory, which is what the runtime reads.
                let llvm = self.llvm_ty(ty);
                let slot = self.scratch(&llvm, self.align_of(ty));
                self.emit(&format!(
                    "store {llvm} {value}, ptr {slot}, align {}",
                    self.align_of(ty)
                ));
                let out = self.fresh();
                self.emit(&format!("{out} = ptrtoint ptr {slot} to i64"));
                out
            }
        }
    }

    /// `call ptr @<name>(ptr table, i64 word)`, declaring it.
    fn table_call(&mut self, name: &'static str, table: &str, word: &str) -> String {
        self.decls.insert(match name {
            "ty_dict_find" => "declare ptr @ty_dict_find(ptr, i64) nounwind",
            "ty_dict_get" => "declare ptr @ty_dict_get(ptr, i64) nounwind",
            "ty_dict_insert" => "declare ptr @ty_dict_insert(ptr, i64) nounwind",
            "ty_dict_remove" => "declare ptr @ty_dict_remove(ptr, i64) nounwind",
            _ => "declare ptr @ty_dict_pop(ptr, i64) nounwind",
        });
        let out = self.fresh();
        self.emit(&format!(
            "{out} = call ptr @{name}(ptr {table}, i64 {word})"
        ));
        out
    }

    /// Loads a value of type `ty` from a dict value slot. The access is left
    /// untagged: the slot lives in the runtime's entry array, which no other
    /// TBAA class describes.
    fn load_value_slot(&mut self, slot: &str, ty: TypeId) -> String {
        let store_ty = self.store_ty(ty);
        let align = self.store_align_of(ty);
        let raw = self.fresh();
        self.emit(&format!(
            "{raw} = load {store_ty}, ptr {slot}, align {align}"
        ));
        if self.types.get(ty) == Type::Bool {
            let out = self.fresh();
            self.emit(&format!("{out} = trunc i8 {raw} to i1"));
            return out;
        }
        raw
    }

    /// Stores a value of type `ty` into a dict value slot.
    fn store_value_slot(&mut self, slot: &str, ty: TypeId, value: &str) {
        let store_ty = self.store_ty(ty);
        let align = self.store_align_of(ty);
        let value = self.widen(ty, value);
        self.emit(&format!(
            "store {store_ty} {value}, ptr {slot}, align {align}"
        ));
    }

    // -- construction ----------------------------------------------------------

    /// `ty_dict_new(desc, val_size, val_align, val_atomic)`.
    fn table_alloc(&mut self, key: TypeId, value: Option<TypeId>) -> String {
        self.decls
            .insert("declare noalias ptr @ty_dict_new(i64, i64, i64, i8) nounwind");
        let desc = self.desc_of(key);
        let (size, align, atomic) = match value {
            Some(value) => (
                self.size_of(value),
                self.store_align_of(value),
                self.atomic_flag(value),
            ),
            None => (0, 1, 1),
        };
        let out = self.fresh();
        self.emit(&format!(
            "{out} = call ptr @ty_dict_new(i64 {desc}, i64 {size}, i64 {align}, i8 {atomic})"
        ));
        out
    }

    /// A dict literal: one allocation, then every entry inserted in order.
    pub(super) fn dict_new(
        &mut self,
        func: &'p hir::Function,
        key: TypeId,
        value: TypeId,
        entries: &[(hir::Expr, hir::Expr)],
    ) -> String {
        let table = self.table_alloc(key, Some(value));
        for (k, v) in entries {
            let k_value = self.expr(func, k);
            let v_value = self.expr(func, v);
            let word = self.key_word(key, &k_value);
            let slot = self.table_call("ty_dict_insert", &table, &word);
            self.store_value_slot(&slot, value, &v_value);
        }
        table
    }

    /// A set literal or `set()`.
    pub(super) fn set_new(
        &mut self,
        func: &'p hir::Function,
        elem: TypeId,
        items: &[hir::Expr],
    ) -> String {
        let table = self.table_alloc(elem, None);
        for item in items {
            let value = self.expr(func, item);
            let word = self.key_word(elem, &value);
            self.table_call("ty_dict_insert", &table, &word);
        }
        table
    }

    // -- access ------------------------------------------------------------------

    /// `d[k]`: panics in the runtime when the key is absent.
    pub(super) fn dict_get(
        &mut self,
        func: &'p hir::Function,
        dict: &hir::Expr,
        key: &hir::Expr,
        value_ty: TypeId,
    ) -> String {
        let table = self.expr(func, dict);
        let k = self.expr(func, key);
        let word = self.key_word(key.ty, &k);
        let slot = self.table_call("ty_dict_get", &table, &word);
        self.load_value_slot(&slot, value_ty)
    }

    /// `d.get(k, default)` and `d.pop(k, default)`: the value slot when the
    /// runtime finds one, the default otherwise.
    fn slot_or_default(&mut self, slot: &str, value_ty: TypeId, default: &str) -> String {
        let llvm = self.llvm_ty(value_ty);
        let align = self.align_of(value_ty);
        let result = self.scratch(&llvm, align);
        let found = self.binop("icmp ne", "ptr", slot, "null");
        let hit = self.fresh_label("get.hit");
        let miss = self.fresh_label("get.miss");
        let end = self.fresh_label("get.end");
        self.cond_br(&found, &hit, &miss);

        self.start_block(&hit);
        let loaded = self.load_value_slot(slot, value_ty);
        self.emit(&format!(
            "store {llvm} {loaded}, ptr {result}, align {align}"
        ));
        self.br(&end);

        self.start_block(&miss);
        self.emit(&format!(
            "store {llvm} {default}, ptr {result}, align {align}"
        ));
        self.br(&end);

        self.start_block(&end);
        let out = self.fresh();
        self.emit(&format!("{out} = load {llvm}, ptr {result}, align {align}"));
        out
    }

    /// `d.get(k, default)`; the default is evaluated either way, as in Python.
    pub(super) fn dict_get_or(
        &mut self,
        func: &'p hir::Function,
        dict: &hir::Expr,
        key: &hir::Expr,
        default: &hir::Expr,
        value_ty: TypeId,
    ) -> String {
        let table = self.expr(func, dict);
        let k = self.expr(func, key);
        let fallback = self.expr(func, default);
        let word = self.key_word(key.ty, &k);
        let slot = self.table_call("ty_dict_find", &table, &word);
        self.slot_or_default(&slot, value_ty, &fallback)
    }

    /// `d.pop(k)` / `d.pop(k, default)`. The popped entry's value stays
    /// readable until the next insertion, and it is read right away.
    pub(super) fn dict_pop(
        &mut self,
        func: &'p hir::Function,
        dict: &hir::Expr,
        key: &hir::Expr,
        default: Option<&hir::Expr>,
        value_ty: TypeId,
    ) -> String {
        let table = self.expr(func, dict);
        let k = self.expr(func, key);
        let fallback = default.map(|d| self.expr(func, d));
        let word = self.key_word(key.ty, &k);
        match fallback {
            Some(fallback) => {
                let slot = self.table_call("ty_dict_remove", &table, &word);
                self.slot_or_default(&slot, value_ty, &fallback)
            }
            None => {
                let slot = self.table_call("ty_dict_pop", &table, &word);
                self.load_value_slot(&slot, value_ty)
            }
        }
    }

    /// `d[k] = v`: the runtime finds or inserts the key, then the value is
    /// stored into the slot it returns.
    pub(super) fn emit_dict_set(
        &mut self,
        func: &'p hir::Function,
        dict: &hir::Expr,
        key: &hir::Expr,
        value: &hir::Expr,
    ) {
        let table = self.expr(func, dict);
        let k = self.expr(func, key);
        let v = self.expr(func, value);
        let word = self.key_word(key.ty, &k);
        let slot = self.table_call("ty_dict_insert", &table, &word);
        self.store_value_slot(&slot, value.ty, &v);
    }

    /// `k in d` / `x in s`, and their `not in` forms.
    pub(super) fn hash_contains(
        &mut self,
        func: &'p hir::Function,
        table: &hir::Expr,
        key: &hir::Expr,
        negated: bool,
    ) -> String {
        let t = self.expr(func, table);
        let k = self.expr(func, key);
        let word = self.key_word(key.ty, &k);
        let slot = self.table_call("ty_dict_find", &t, &word);
        let pred = if negated { "icmp eq" } else { "icmp ne" };
        self.binop(pred, "ptr", &slot, "null")
    }

    /// `s.add(x)`, `s.remove(x)` and `s.discard(x)`.
    pub(super) fn set_update(
        &mut self,
        func: &'p hir::Function,
        set: &hir::Expr,
        value: &hir::Expr,
        call: &'static str,
    ) {
        let t = self.expr(func, set);
        let v = self.expr(func, value);
        let word = self.key_word(value.ty, &v);
        self.table_call(call, &t, &word);
    }

    /// `len(d)`, read from the header like a list's length.
    pub(super) fn hash_len(&mut self, table: &str) -> String {
        let out = self.fresh();
        self.emit(&format!("{out} = load i64, ptr {table}, align 8"));
        out
    }

    /// `keys()`, `values()` or `items()`: a fresh list from the runtime.
    pub(super) fn table_list(&mut self, table: &str, name: &'static str) -> String {
        self.decls.insert(match name {
            "ty_dict_keys" => "declare noalias ptr @ty_dict_keys(ptr) nounwind",
            "ty_dict_values" => "declare noalias ptr @ty_dict_values(ptr) nounwind",
            _ => "declare noalias ptr @ty_dict_items(ptr) nounwind",
        });
        let out = self.fresh();
        self.emit(&format!("{out} = call ptr @{name}(ptr {table})"));
        out
    }

    // -- builtins ------------------------------------------------------------------

    /// `sorted(xs)`: the runtime sorts a copy, guided by the element
    /// descriptor.
    pub(super) fn sorted(&mut self, func: &'p hir::Function, list: &hir::Expr) -> String {
        let elem = self.types.as_list(list.ty).expect("`sorted` takes a list");
        let value = self.expr(func, list);
        let desc = self.desc_of(elem);
        self.decls
            .insert("declare noalias ptr @ty_list_sorted(ptr, i64) nounwind");
        let out = self.fresh();
        self.emit(&format!(
            "{out} = call ptr @ty_list_sorted(ptr {value}, i64 {desc})"
        ));
        out
    }

    /// `s[a:b]`.
    pub(super) fn str_slice(
        &mut self,
        func: &'p hir::Function,
        string: &hir::Expr,
        start: &hir::Expr,
        stop: &hir::Expr,
    ) -> String {
        let s = self.expr(func, string);
        let a = self.expr(func, start);
        let b = self.expr(func, stop);
        self.decls
            .insert("declare ptr @ty_str_slice(ptr nocapture readonly, i64, i64) nounwind");
        let out = self.fresh();
        self.emit(&format!(
            "{out} = call ptr @ty_str_slice(ptr {s}, i64 {a}, i64 {b})"
        ));
        out
    }

    // -- printing --------------------------------------------------------------------

    /// `{"a": 1, "b": 2}` (and `{}`): the keys and the values are fetched as
    /// two lists and written pairwise.
    pub(super) fn print_dict(&mut self, key: TypeId, value: TypeId, dict: &str) {
        let keys = self.table_list(dict, "ty_dict_keys");
        let values = self.table_list(dict, "ty_dict_values");
        self.print_raw("{");
        self.print_elements(&keys, key, Some((&values, value)));
        self.print_raw("}");
    }

    /// `{1, 2}`, and `set()` for the empty set — Python's spelling, since `{}`
    /// is an empty dict.
    pub(super) fn print_set(&mut self, elem: TypeId, set: &str) {
        let keys = self.table_list(set, "ty_dict_keys");
        let len = self.list_len(&keys);
        let empty = self.binop("icmp eq", "i64", &len, "0");
        let empty_label = self.fresh_label("set.empty");
        let full_label = self.fresh_label("set.full");
        let end = self.fresh_label("set.end");
        self.cond_br(&empty, &empty_label, &full_label);

        self.start_block(&empty_label);
        self.print_raw("set()");
        self.br(&end);

        self.start_block(&full_label);
        self.print_raw("{");
        self.print_elements(&keys, elem, None);
        self.print_raw("}");
        self.br(&end);

        self.start_block(&end);
    }

    /// Writes the elements of `list` separated by `, ` — or, with `values`,
    /// `key: value` pairs taken from two parallel lists.
    fn print_elements(&mut self, list: &str, elem: TypeId, values: Option<(&str, TypeId)>) {
        let len = self.list_len(list);
        self.lbl += 1;
        let cursor = format!("%repr.iv.{}", self.lbl);
        let _ = writeln!(self.allocas, "  {cursor} = alloca i64, align 8");
        self.emit(&format!("store i64 0, ptr {cursor}, align 8"));

        let head = self.fresh_label("repr.cond");
        let body = self.fresh_label("repr.body");
        let sep = self.fresh_label("repr.sep");
        let item = self.fresh_label("repr.item");
        let end = self.fresh_label("repr.end");

        self.br(&head);
        self.start_block(&head);
        let index = self.fresh();
        self.emit(&format!("{index} = load i64, ptr {cursor}, align 8"));
        let more = self.binop("icmp slt", "i64", &index, &len);
        self.cond_br(&more, &body, &end);

        self.start_block(&body);
        let first = self.binop("icmp eq", "i64", &index, "0");
        self.cond_br(&first, &item, &sep);
        self.start_block(&sep);
        self.print_raw(", ");
        self.br(&item);

        self.start_block(&item);
        let data = self.list_data(list);
        let slot = self.elem_slot(&data, elem, &index);
        let key = self.load_slot(&slot, elem, Tbaa::ListData);
        let pair = values.map(|(values, value_ty)| {
            let data = self.list_data(values);
            let slot = self.elem_slot(&data, value_ty, &index);
            (self.load_slot(&slot, value_ty, Tbaa::ListData), value_ty)
        });
        let step = self.binop("add", "i64", &index, "1");
        self.emit(&format!("store i64 {step}, ptr {cursor}, align 8"));
        self.print_value(elem, &key, true);
        if let Some((value, value_ty)) = pair {
            self.print_raw(": ");
            self.print_value(value_ty, &value, true);
        }
        self.br(&head);

        self.start_block(&end);
    }
}
