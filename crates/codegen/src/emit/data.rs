//! Lowering of the M2 data types: `list<T>`, `str`, `tuple` and class
//! instances (DESIGN §3.8, §4.1, §4.2).
//!
//! Everything on a hot path is emitted **inline**: the bounds check, the
//! element load and store, `len`, the `append` fast path, field access and the
//! `str` header reads. The runtime is only called for the slow paths — growth,
//! allocation, string algorithms and formatting (DESIGN §2.2).
//!
//! Memory layouts:
//!
//! ```text
//! list<T>   ptr -> { i64 len, i64 cap, ptr data }   (%TyList, 24 bytes)
//!                                  data -> cap * sizeof(T) bytes
//! str       ptr -> { i64 byte_len, i64 char_len, [n x i8] }
//! class C   ptr -> %C.<n> = type { field storage types, in order }
//! tuple     a first-class LLVM aggregate, passed and returned by value
//! ```
//!
//! `bool` is `i1` in registers and `i8` in every one of those containers
//! (DESIGN §4.2), so element and field access `zext` on the way in and `trunc`
//! on the way out.

use std::fmt::Write;

use typhoon_sema::hir;
use typhoon_sema::types::{ClassId, Type, TypeId};

use super::{Emitter, STR_CHAR_LEN_OFFSET, STR_HEADER, Tbaa, class_type_name};

impl<'p> Emitter<'p> {
    // -- str ---------------------------------------------------------------

    /// The byte length of a `str`, read from its header.
    pub(super) fn str_byte_len(&mut self, value: &str) -> String {
        let out = self.fresh();
        let tbaa = self.tbaa(Tbaa::StrHeader);
        self.emit(&format!("{out} = load i64, ptr {value}, align 8{tbaa}"));
        out
    }

    /// The code-point count of a `str` — `len(s)`, O(1) (DESIGN §4.3).
    pub(super) fn str_char_len(&mut self, value: &str) -> String {
        let field = self.fresh();
        let out = self.fresh();
        self.emit(&format!(
            "{field} = getelementptr inbounds i8, ptr {value}, i64 {STR_CHAR_LEN_OFFSET}"
        ));
        let tbaa = self.tbaa(Tbaa::StrHeader);
        self.emit(&format!("{out} = load i64, ptr {field}, align 8{tbaa}"));
        out
    }

    /// Splits a `str` value into the pointer to its bytes and its byte length.
    pub(super) fn str_parts(&mut self, value: &str) -> (String, String) {
        let len = self.str_byte_len(value);
        let bytes = self.fresh();
        self.emit(&format!(
            "{bytes} = getelementptr inbounds i8, ptr {value}, i64 {STR_HEADER}"
        ));
        (bytes, len)
    }

    /// One of the `str` methods of DESIGN §4.6, all of them runtime calls:
    /// none is on a hot path and all of them allocate.
    pub(super) fn str_method(
        &mut self,
        func: &'p hir::Function,
        op: hir::StrMethod,
        recv: &hir::Expr,
        args: &[hir::Expr],
    ) -> String {
        let recv = self.expr(func, recv);
        let values: Vec<String> = args.iter().map(|a| self.expr(func, a)).collect();
        let (name, decl, ret) = match op {
            hir::StrMethod::Upper => (
                "ty_str_upper",
                "declare noalias ptr @ty_str_upper(ptr nocapture readonly) nounwind",
                "ptr",
            ),
            hir::StrMethod::Lower => (
                "ty_str_lower",
                "declare noalias ptr @ty_str_lower(ptr nocapture readonly) nounwind",
                "ptr",
            ),
            hir::StrMethod::Strip => (
                "ty_str_strip",
                "declare noalias ptr @ty_str_strip(ptr nocapture readonly) nounwind",
                "ptr",
            ),
            hir::StrMethod::Split => (
                "ty_str_split",
                "declare noalias ptr @ty_str_split(ptr nocapture readonly, ptr nocapture readonly) nounwind",
                "ptr",
            ),
            hir::StrMethod::Join => (
                "ty_str_join",
                "declare noalias ptr @ty_str_join(ptr nocapture readonly, ptr nocapture readonly) nounwind",
                "ptr",
            ),
            hir::StrMethod::StartsWith => (
                "ty_str_startswith",
                "declare i8 @ty_str_startswith(ptr nocapture readonly, ptr nocapture readonly) nounwind willreturn",
                "i8",
            ),
            hir::StrMethod::EndsWith => (
                "ty_str_endswith",
                "declare i8 @ty_str_endswith(ptr nocapture readonly, ptr nocapture readonly) nounwind willreturn",
                "i8",
            ),
            hir::StrMethod::Find => (
                "ty_str_find",
                "declare i64 @ty_str_find(ptr nocapture readonly, ptr nocapture readonly) nounwind willreturn",
                "i64",
            ),
            hir::StrMethod::Replace => (
                "ty_str_replace",
                "declare noalias ptr @ty_str_replace(ptr nocapture readonly, ptr nocapture readonly, ptr nocapture readonly) nounwind",
                "ptr",
            ),
        };
        self.decls.insert(decl);
        let mut operands = vec![format!("ptr {recv}")];
        for value in &values {
            operands.push(format!("ptr {value}"));
        }
        let raw = self.fresh();
        self.emit(&format!(
            "{raw} = call {ret} @{name}({})",
            operands.join(", ")
        ));
        if ret == "i8" {
            // The predicates return 0/1 as an `i8`; `bool` is `i1`.
            return self.binop("icmp ne", "i8", &raw, "0");
        }
        raw
    }

    // -- list --------------------------------------------------------------

    /// `len(xs)`, read from the list header.
    pub(super) fn list_len(&mut self, list: &str) -> String {
        let out = self.fresh();
        let tbaa = self.tbaa(Tbaa::ListHeader);
        self.emit(&format!("{out} = load i64, ptr {list}, align 8{tbaa}"));
        out
    }

    /// Overwrites a list's length, the only header field the emitter writes
    /// itself (growth is the runtime's job).
    fn set_list_len(&mut self, list: &str, len: &str) {
        let tbaa = self.tbaa(Tbaa::ListHeader);
        self.emit(&format!("store i64 {len}, ptr {list}, align 8{tbaa}"));
    }

    /// The pointer to a list's element buffer.
    pub(super) fn list_data(&mut self, list: &str) -> String {
        let field = self.fresh();
        let out = self.fresh();
        let list_ty = self.list_type();
        self.emit(&format!(
            "{field} = getelementptr inbounds {list_ty}, ptr {list}, i32 0, i32 2"
        ));
        let tbaa = self.tbaa(Tbaa::ListHeader);
        self.emit(&format!("{out} = load ptr, ptr {field}, align 8{tbaa}"));
        out
    }

    /// Turns a possibly-negative index into a checked one (DESIGN §4.3:
    /// Python's negative indices, a panic when out of range).
    ///
    /// The fast path is a single unsigned compare — exactly what Go emits for
    /// a slice — because an unsigned `<` against the length rejects a negative
    /// index at the same time. Only that rejected case runs the `+ len`
    /// fix-up, in a block off the hot path, so a loop that never uses a
    /// negative index pays nothing for the feature.
    pub(super) fn checked_index(&mut self, index: &str, len: &str) -> String {
        let slot = self.scratch("i64", 8);
        self.emit(&format!("store i64 {index}, ptr {slot}, align 8"));
        let in_range = self.binop("icmp ult", "i64", index, len);

        let adjust = self.fresh_label("index.neg");
        let cont = self.fresh_label("index.ok");
        self.cond_br(&in_range, &cont, &adjust);

        self.start_block(&adjust);
        let negative = self.binop("icmp slt", "i64", index, "0");
        let wrapped = self.binop("add", "i64", index, len);
        self.emit(&format!("store i64 {wrapped}, ptr {slot}, align 8"));
        let wrapped_ok = self.binop("icmp ult", "i64", &wrapped, len);
        let valid = self.binop("and", "i1", &negative, &wrapped_ok);
        let bad = self.binop("xor", "i1", &valid, "true");
        self.panic_if(&bad, "list index out of range");
        self.br(&cont);

        self.start_block(&cont);
        let out = self.fresh();
        self.emit(&format!("{out} = load i64, ptr {slot}, align 8"));
        out
    }

    /// The address of element `index` of `data`, whose elements are `elem`.
    pub(super) fn elem_slot(&mut self, data: &str, elem: TypeId, index: &str) -> String {
        let ty = self.store_ty(elem);
        let out = self.fresh();
        self.emit(&format!(
            "{out} = getelementptr inbounds {ty}, ptr {data}, i64 {index}"
        ));
        out
    }

    /// Loads a value of type `ty` out of a container slot, narrowing a stored
    /// `i8` back to the `i1` a `bool` is in registers.
    pub(super) fn load_slot(&mut self, slot: &str, ty: TypeId, tag: Tbaa) -> String {
        let store_ty = self.store_ty(ty);
        let align = self.store_align_of(ty);
        let tbaa = self.tbaa(tag);
        let raw = self.fresh();
        self.emit(&format!(
            "{raw} = load {store_ty}, ptr {slot}, align {align}{tbaa}"
        ));
        if self.types.get(ty) == Type::Bool {
            let out = self.fresh();
            self.emit(&format!("{out} = trunc i8 {raw} to i1"));
            return out;
        }
        raw
    }

    /// Stores a register value into a container slot, widening a `bool`.
    pub(super) fn store_slot(&mut self, slot: &str, ty: TypeId, value: &str, tag: Tbaa) {
        let store_ty = self.store_ty(ty);
        let align = self.store_align_of(ty);
        let value = self.widen(ty, value);
        let tbaa = self.tbaa(tag);
        self.emit(&format!(
            "store {store_ty} {value}, ptr {slot}, align {align}{tbaa}"
        ));
    }

    /// `zext`s an `i1` to the `i8` a `bool` is stored as; other types pass
    /// through unchanged.
    pub(super) fn widen(&mut self, ty: TypeId, value: &str) -> String {
        if self.types.get(ty) != Type::Bool {
            return value.to_string();
        }
        let out = self.fresh();
        self.emit(&format!("{out} = zext i1 {value} to i8"));
        out
    }

    /// A list literal: one allocation, then the elements stored in order.
    pub(super) fn list_new(
        &mut self,
        func: &'p hir::Function,
        elem: TypeId,
        items: &[hir::Expr],
    ) -> String {
        let values: Vec<String> = items.iter().map(|item| self.expr(func, item)).collect();
        let list = self.list_alloc(elem, items.len() as u64);
        if !values.is_empty() {
            let data = self.list_data(&list);
            for (index, value) in values.iter().enumerate() {
                let slot = self.elem_slot(&data, elem, &index.to_string());
                self.store_slot(&slot, elem, value, Tbaa::ListData);
            }
            self.emit(&format!("store i64 {}, ptr {list}, align 8", values.len()));
        }
        list
    }

    /// `ty_list_new(elem_size, atomic, cap)`.
    fn list_alloc(&mut self, elem: TypeId, cap: u64) -> String {
        self.decls
            .insert("declare noalias ptr @ty_list_new(i64, i8, i64) nounwind");
        let out = self.fresh();
        self.emit(&format!(
            "{out} = call ptr @ty_list_new(i64 {}, i8 {}, i64 {cap})",
            self.size_of(elem),
            self.atomic_flag(elem)
        ));
        out
    }

    /// `xs[i]`: bounds check, then one load.
    pub(super) fn list_get(
        &mut self,
        func: &'p hir::Function,
        list: &hir::Expr,
        index: &hir::Expr,
        elem: TypeId,
    ) -> String {
        let list = self.expr(func, list);
        let index = self.expr(func, index);
        let len = self.list_len(&list);
        let resolved = self.checked_index(&index, &len);
        let data = self.list_data(&list);
        let slot = self.elem_slot(&data, elem, &resolved);
        self.load_slot(&slot, elem, Tbaa::ListData)
    }

    /// `xs.append(v)`: inline fast path, `ty_list_grow` only when full.
    pub(super) fn list_append(
        &mut self,
        func: &'p hir::Function,
        list: &hir::Expr,
        value: &hir::Expr,
    ) {
        let elem = value.ty;
        let list = self.expr(func, list);
        let value = self.expr(func, value);
        let len = self.list_len(&list);
        let cap_field = self.fresh();
        let cap = self.fresh();
        let list_ty = self.list_type();
        self.emit(&format!(
            "{cap_field} = getelementptr inbounds {list_ty}, ptr {list}, i32 0, i32 1"
        ));
        let tbaa = self.tbaa(Tbaa::ListHeader);
        self.emit(&format!("{cap} = load i64, ptr {cap_field}, align 8{tbaa}"));
        let next = self.binop("add", "i64", &len, "1");
        let full = self.binop("icmp sge", "i64", &len, &cap);

        let grow = self.fresh_label("append.grow");
        let store = self.fresh_label("append.store");
        self.cond_br(&full, &grow, &store);
        self.start_block(&grow);
        self.decls
            .insert("declare void @ty_list_grow(ptr, i64, i8, i64) nounwind");
        self.emit(&format!(
            "call void @ty_list_grow(ptr {list}, i64 {}, i8 {}, i64 {next})",
            self.size_of(elem),
            self.atomic_flag(elem)
        ));
        self.br(&store);

        self.start_block(&store);
        let data = self.list_data(&list);
        let slot = self.elem_slot(&data, elem, &len);
        self.store_slot(&slot, elem, &value, Tbaa::ListData);
        self.set_list_len(&list, &next);
    }

    /// `xs.pop()`: panics on an empty list, otherwise one load and one store.
    pub(super) fn list_pop(
        &mut self,
        func: &'p hir::Function,
        list: &hir::Expr,
        elem: TypeId,
    ) -> String {
        let list = self.expr(func, list);
        let len = self.list_len(&list);
        let empty = self.binop("icmp sle", "i64", &len, "0");
        self.panic_if(&empty, "pop from empty list");
        let last = self.binop("sub", "i64", &len, "1");
        let data = self.list_data(&list);
        let slot = self.elem_slot(&data, elem, &last);
        let value = self.load_slot(&slot, elem, Tbaa::ListData);
        self.set_list_len(&list, &last);
        value
    }

    /// `xs.insert(i, v)`: the runtime makes room and hands back the slot.
    pub(super) fn list_insert(
        &mut self,
        func: &'p hir::Function,
        list: &hir::Expr,
        index: &hir::Expr,
        value: &hir::Expr,
    ) {
        let elem = value.ty;
        let list = self.expr(func, list);
        let index = self.expr(func, index);
        let value = self.expr(func, value);
        self.decls
            .insert("declare ptr @ty_list_insert_slot(ptr, i64, i64, i8) nounwind");
        let slot = self.fresh();
        self.emit(&format!(
            "{slot} = call ptr @ty_list_insert_slot(ptr {list}, i64 {index}, i64 {}, i8 {})",
            self.size_of(elem),
            self.atomic_flag(elem)
        ));
        self.store_slot(&slot, elem, &value, Tbaa::ListData);
    }

    /// `xs.clear()`: the buffer is kept, only the length is reset.
    pub(super) fn list_clear(&mut self, func: &'p hir::Function, list: &hir::Expr) {
        let list = self.expr(func, list);
        self.set_list_len(&list, "0");
    }

    /// `xs + ys`: a fresh list holding both.
    pub(super) fn list_concat(
        &mut self,
        func: &'p hir::Function,
        lhs: &hir::Expr,
        rhs: &hir::Expr,
        elem: TypeId,
    ) -> String {
        let lhs = self.expr(func, lhs);
        let rhs = self.expr(func, rhs);
        self.decls
            .insert("declare noalias ptr @ty_list_concat(ptr, ptr, i64, i8) nounwind");
        let out = self.fresh();
        self.emit(&format!(
            "{out} = call ptr @ty_list_concat(ptr {lhs}, ptr {rhs}, i64 {}, i8 {})",
            self.size_of(elem),
            self.atomic_flag(elem)
        ));
        out
    }

    /// `xs[i] = v`.
    pub(super) fn emit_set_index(
        &mut self,
        func: &'p hir::Function,
        list: &hir::Expr,
        index: &hir::Expr,
        value: &hir::Expr,
    ) {
        let elem = value.ty;
        let list_value = self.expr(func, list);
        let index_value = self.expr(func, index);
        let value = self.expr(func, value);
        let len = self.list_len(&list_value);
        let resolved = self.checked_index(&index_value, &len);
        let data = self.list_data(&list_value);
        let slot = self.elem_slot(&data, elem, &resolved);
        self.store_slot(&slot, elem, &value, Tbaa::ListData);
    }

    // -- class -------------------------------------------------------------

    /// The address of one field of a class instance.
    pub(super) fn field_slot(&mut self, obj: &str, class: ClassId, field: u32) -> String {
        let out = self.fresh();
        self.emit(&format!(
            "{out} = getelementptr inbounds {}, ptr {obj}, i32 0, i32 {field}",
            class_type_name(class)
        ));
        out
    }

    /// `C(field=…)`: one allocation, then every field stored in order.
    ///
    /// A class with no pointer-holding field goes into an unscanned block
    /// (DESIGN §5.1).
    pub(super) fn class_new(
        &mut self,
        func: &'p hir::Function,
        class: ClassId,
        fields: &[hir::Expr],
        eval_order: &[u32],
    ) -> String {
        // Written order first, then the constant defaults (DESIGN §3.2).
        let mut values: Vec<Option<String>> = vec![None; fields.len()];
        for index in eval_order {
            let index = *index as usize;
            values[index] = Some(self.expr(func, &fields[index]));
        }
        for (index, value) in values.iter_mut().enumerate() {
            if value.is_none() {
                *value = Some(self.expr(func, &fields[index]));
            }
        }
        let values: Vec<String> = values
            .into_iter()
            .map(|v| v.expect("filled above"))
            .collect();
        let types: Vec<TypeId> = self
            .types
            .class(class)
            .fields
            .iter()
            .map(|f| f.ty)
            .collect();
        let size = self.struct_size(&types).max(1);
        let atomic = !self.types.class(class).has_pointers;
        let name = if atomic {
            "ty_alloc_atomic"
        } else {
            "ty_alloc"
        };
        self.decls.insert(if atomic {
            "declare noalias ptr @ty_alloc_atomic(i64) nounwind"
        } else {
            "declare noalias ptr @ty_alloc(i64) nounwind"
        });
        let obj = self.fresh();
        self.emit(&format!("{obj} = call ptr @{name}(i64 {size})"));
        for (index, value) in values.iter().enumerate() {
            let slot = self.field_slot(&obj, class, index as u32);
            self.store_slot(&slot, types[index], value, Tbaa::ClassField);
        }
        obj
    }

    /// `obj.field = value`.
    pub(super) fn emit_set_field(
        &mut self,
        func: &'p hir::Function,
        obj: &hir::Expr,
        class: ClassId,
        field: u32,
        value: &hir::Expr,
    ) {
        let ty = self.types.class(class).fields[field as usize].ty;
        let obj = self.expr(func, obj);
        let value = self.expr(func, value);
        let slot = self.field_slot(&obj, class, field);
        self.store_slot(&slot, ty, &value, Tbaa::ClassField);
    }

    // -- tuple -------------------------------------------------------------

    /// A tuple literal: an LLVM first-class aggregate built with
    /// `insertvalue`, passed and returned by value (DESIGN §4.2).
    pub(super) fn tuple_new(
        &mut self,
        func: &'p hir::Function,
        ty: TypeId,
        items: &[hir::Expr],
    ) -> String {
        let llvm = self.llvm_ty(ty);
        let mut acc = "poison".to_string();
        for (index, item) in items.iter().enumerate() {
            let member_ty = item.ty;
            let value = self.expr(func, item);
            let value = self.widen(member_ty, &value);
            let store_ty = self.store_ty(member_ty);
            let out = self.fresh();
            self.emit(&format!(
                "{out} = insertvalue {llvm} {acc}, {store_ty} {value}, {index}"
            ));
            acc = out;
        }
        acc
    }

    /// `t[k]`: one `extractvalue`, the index proved in range by the checker.
    pub(super) fn tuple_get(
        &mut self,
        func: &'p hir::Function,
        tuple: &hir::Expr,
        index: u32,
        member: TypeId,
    ) -> String {
        let llvm = self.llvm_ty(tuple.ty);
        let value = self.expr(func, tuple);
        let raw = self.fresh();
        self.emit(&format!("{raw} = extractvalue {llvm} {value}, {index}"));
        if self.types.get(member) == Type::Bool {
            let out = self.fresh();
            self.emit(&format!("{out} = trunc i8 {raw} to i1"));
            return out;
        }
        raw
    }

    // -- iteration ----------------------------------------------------------

    /// `for x in <list or str>` (DESIGN §3.5).
    ///
    /// The cursor advances at the top of the body, before the user's
    /// statements, so that `continue` needs no work of its own and the loaded
    /// element never has to outlive the block that produced it.
    pub(super) fn emit_for_each(
        &mut self,
        func: &'p hir::Function,
        var: hir::LocalId,
        iter: &hir::Expr,
        over: hir::IterKind,
        body: &hir::Block,
    ) {
        let iterable = self.expr(func, iter);
        let bound = match over {
            hir::IterKind::List => String::new(),
            // A `str` is immutable, so its byte length is loop-invariant.
            hir::IterKind::Str => self.str_byte_len(&iterable),
        };

        self.lbl += 1;
        let cursor = format!("%iter.{}", self.lbl);
        let _ = writeln!(self.allocas, "  {cursor} = alloca i64, align 8");
        self.emit(&format!("store i64 0, ptr {cursor}, align 8"));

        let head = self.fresh_label("each.cond");
        let body_label = self.fresh_label("each.body");
        let next = self.fresh_label("each.step");
        let end = self.fresh_label("each.end");

        self.br(&head);
        self.start_block(&head);
        let position = self.fresh();
        self.emit(&format!("{position} = load i64, ptr {cursor}, align 8"));
        // A list may grow or shrink inside the body, exactly as in Python, so
        // its length is re-read every time round.
        let limit = match over {
            hir::IterKind::List => self.list_len(&iterable),
            hir::IterKind::Str => bound.clone(),
        };
        let more = self.binop("icmp slt", "i64", &position, &limit);
        self.cond_br(&more, &body_label, &end);

        self.start_block(&body_label);
        let slot = self.slots[var.index()].clone();
        let var_ty = func.locals[var.index()].ty;
        match over {
            hir::IterKind::List => {
                let data = self.list_data(&iterable);
                let element = self.elem_slot(&data, var_ty, &position);
                let value = self.load_slot(&element, var_ty, Tbaa::ListData);
                self.emit(&format!(
                    "store {} {value}, ptr {slot}, align {}",
                    self.llvm_ty(var_ty),
                    self.align_of(var_ty)
                ));
                let step = self.binop("add", "i64", &position, "1");
                self.emit(&format!("store i64 {step}, ptr {cursor}, align 8"));
            }
            hir::IterKind::Str => {
                // The runtime hands back a length-1 `str`; ASCII comes from a
                // static table, so an ASCII scan allocates nothing.
                self.decls
                    .insert("declare ptr @ty_str_char_at(ptr, i64) nounwind");
                let ch = self.fresh();
                self.emit(&format!(
                    "{ch} = call ptr @ty_str_char_at(ptr {iterable}, i64 {position})"
                ));
                self.emit(&format!("store ptr {ch}, ptr {slot}, align 8"));
                let width = self.str_byte_len(&ch);
                let step = self.binop("add", "i64", &position, &width);
                self.emit(&format!("store i64 {step}, ptr {cursor}, align 8"));
            }
        }
        self.loops.push((end.clone(), next.clone()));
        self.block(func, body);
        self.loops.pop();
        if !self.terminated {
            self.br(&next);
        }

        self.start_block(&next);
        self.br(&head);
        self.start_block(&end);
    }

    // -- equality and membership -------------------------------------------

    /// `a == b` for two values of the same type, as an `i1`.
    ///
    /// Recurses through lists and tuples, emitting a loop for every list level
    /// (DESIGN §4.6: containers compare element by element).
    pub(super) fn values_equal(&mut self, ty: TypeId, lhs: &str, rhs: &str) -> String {
        match self.types.get(ty) {
            Type::Int => self.binop("icmp eq", "i64", lhs, rhs),
            Type::Float => self.binop("fcmp oeq", "double", lhs, rhs),
            Type::Bool => self.binop("icmp eq", "i1", lhs, rhs),
            Type::Str => {
                self.decls
                    .insert("declare i8 @ty_str_eq(ptr nocapture readonly, ptr nocapture readonly) nounwind willreturn");
                let raw = self.fresh();
                self.emit(&format!("{raw} = call i8 @ty_str_eq(ptr {lhs}, ptr {rhs})"));
                self.binop("icmp ne", "i8", &raw, "0")
            }
            Type::Tuple(id) => {
                let members = self.types.tuple_members(id).to_vec();
                let llvm = self.llvm_ty(ty);
                let mut acc: Option<String> = None;
                for (index, member) in members.iter().enumerate() {
                    let a = self.fresh();
                    let b = self.fresh();
                    self.emit(&format!("{a} = extractvalue {llvm} {lhs}, {index}"));
                    self.emit(&format!("{b} = extractvalue {llvm} {rhs}, {index}"));
                    let (a, b) = if self.types.get(*member) == Type::Bool {
                        let a1 = self.fresh();
                        let b1 = self.fresh();
                        self.emit(&format!("{a1} = trunc i8 {a} to i1"));
                        self.emit(&format!("{b1} = trunc i8 {b} to i1"));
                        (a1, b1)
                    } else {
                        (a, b)
                    };
                    let eq = self.values_equal(*member, &a, &b);
                    acc = Some(match acc {
                        Some(prev) => self.binop("and", "i1", &prev, &eq),
                        None => eq,
                    });
                }
                acc.unwrap_or_else(|| "true".to_string())
            }
            Type::List(elem) => self.lists_equal(elem, lhs, rhs),
            _ => "false".to_string(),
        }
    }

    /// Element-by-element list equality, as an inline loop.
    fn lists_equal(&mut self, elem: TypeId, lhs: &str, rhs: &str) -> String {
        let result = self.scratch("i1", 1);
        self.emit(&format!("store i1 false, ptr {result}, align 1"));
        let left_len = self.list_len(lhs);
        let right_len = self.list_len(rhs);
        let same_len = self.binop("icmp eq", "i64", &left_len, &right_len);

        let head = self.fresh_label("eq.cond");
        let body = self.fresh_label("eq.body");
        let equal = self.fresh_label("eq.true");
        let end = self.fresh_label("eq.end");

        self.lbl += 1;
        let cursor = format!("%eq.iv.{}", self.lbl);
        let _ = writeln!(self.allocas, "  {cursor} = alloca i64, align 8");
        self.emit(&format!("store i64 0, ptr {cursor}, align 8"));
        self.cond_br(&same_len, &head, &end);

        self.start_block(&head);
        let index = self.fresh();
        self.emit(&format!("{index} = load i64, ptr {cursor}, align 8"));
        let more = self.binop("icmp slt", "i64", &index, &left_len);
        self.cond_br(&more, &body, &equal);

        self.start_block(&body);
        let left_data = self.list_data(lhs);
        let right_data = self.list_data(rhs);
        let left_slot = self.elem_slot(&left_data, elem, &index);
        let right_slot = self.elem_slot(&right_data, elem, &index);
        let a = self.load_slot(&left_slot, elem, Tbaa::ListData);
        let b = self.load_slot(&right_slot, elem, Tbaa::ListData);
        let step = self.binop("add", "i64", &index, "1");
        self.emit(&format!("store i64 {step}, ptr {cursor}, align 8"));
        let eq = self.values_equal(elem, &a, &b);
        self.cond_br(&eq, &head, &end);

        self.start_block(&equal);
        self.emit(&format!("store i1 true, ptr {result}, align 1"));
        self.br(&end);

        self.start_block(&end);
        let out = self.fresh();
        self.emit(&format!("{out} = load i1, ptr {result}, align 1"));
        out
    }

    /// `v in xs` / `v in s`, and their `not in` forms.
    pub(super) fn contains(
        &mut self,
        func: &'p hir::Function,
        haystack: &hir::Expr,
        needle: &hir::Expr,
        negated: bool,
    ) -> String {
        let container_ty = haystack.ty;
        let container = self.expr(func, haystack);
        let elem_ty = needle.ty;
        let value = self.expr(func, needle);
        let found = match self.types.get(container_ty) {
            // A substring search is a string algorithm, not a hot loop.
            Type::Str => {
                self.decls
                    .insert("declare i8 @ty_str_contains(ptr nocapture readonly, ptr nocapture readonly) nounwind willreturn");
                let raw = self.fresh();
                self.emit(&format!(
                    "{raw} = call i8 @ty_str_contains(ptr {container}, ptr {value})"
                ));
                self.binop("icmp ne", "i8", &raw, "0")
            }
            _ => self.list_contains(&container, elem_ty, &value),
        };
        if negated {
            return self.binop("xor", "i1", &found, "true");
        }
        found
    }

    /// A linear scan over a list, emitted inline.
    fn list_contains(&mut self, list: &str, elem: TypeId, value: &str) -> String {
        let result = self.scratch("i1", 1);
        self.emit(&format!("store i1 false, ptr {result}, align 1"));
        let len = self.list_len(list);

        self.lbl += 1;
        let cursor = format!("%in.iv.{}", self.lbl);
        let _ = writeln!(self.allocas, "  {cursor} = alloca i64, align 8");
        self.emit(&format!("store i64 0, ptr {cursor}, align 8"));

        let head = self.fresh_label("in.cond");
        let body = self.fresh_label("in.body");
        let hit = self.fresh_label("in.found");
        let end = self.fresh_label("in.end");

        self.br(&head);
        self.start_block(&head);
        let index = self.fresh();
        self.emit(&format!("{index} = load i64, ptr {cursor}, align 8"));
        let more = self.binop("icmp slt", "i64", &index, &len);
        self.cond_br(&more, &body, &end);

        self.start_block(&body);
        let data = self.list_data(list);
        let slot = self.elem_slot(&data, elem, &index);
        let element = self.load_slot(&slot, elem, Tbaa::ListData);
        let step = self.binop("add", "i64", &index, "1");
        self.emit(&format!("store i64 {step}, ptr {cursor}, align 8"));
        let eq = self.values_equal(elem, &element, value);
        self.cond_br(&eq, &hit, &head);

        self.start_block(&hit);
        self.emit(&format!("store i1 true, ptr {result}, align 1"));
        self.br(&end);

        self.start_block(&end);
        let out = self.fresh();
        self.emit(&format!("{out} = load i1, ptr {result}, align 1"));
        out
    }

    // -- printing ----------------------------------------------------------

    /// Writes a literal run of bytes to stdout, for the punctuation around a
    /// container.
    pub(super) fn print_raw(&mut self, text: &str) {
        self.decls
            .insert("declare void @ty_print_str(ptr, i64) nounwind");
        let (global, len) = self.raw_bytes(text);
        self.emit(&format!("call void @ty_print_str(ptr {global}, i64 {len})"));
    }

    /// Writes one value the way `print` does (DESIGN §4.6).
    ///
    /// `quoted` is set inside a container, where a `str` prints as its repr —
    /// `["a", "b"]` — instead of verbatim.
    pub(super) fn print_value(&mut self, ty: TypeId, value: &str, quoted: bool) {
        match self.types.get(ty) {
            Type::Int => {
                self.decls
                    .insert("declare void @ty_print_int(i64) nounwind");
                self.emit(&format!("call void @ty_print_int(i64 {value})"));
            }
            Type::Float => {
                self.decls
                    .insert("declare void @ty_print_float(double) nounwind");
                self.emit(&format!("call void @ty_print_float(double {value})"));
            }
            Type::Bool => {
                self.decls
                    .insert("declare void @ty_print_bool(i8) nounwind");
                let byte = self.widen(ty, value);
                self.emit(&format!("call void @ty_print_bool(i8 {byte})"));
            }
            Type::Str if quoted => {
                self.decls
                    .insert("declare void @ty_print_str_repr(ptr) nounwind");
                self.emit(&format!("call void @ty_print_str_repr(ptr {value})"));
            }
            Type::Str => {
                self.decls
                    .insert("declare void @ty_print_str(ptr, i64) nounwind");
                let (bytes, len) = self.str_parts(value);
                self.emit(&format!("call void @ty_print_str(ptr {bytes}, i64 {len})"));
            }
            Type::List(elem) => self.print_list(elem, value),
            Type::Tuple(id) => {
                let members = self.types.tuple_members(id).to_vec();
                let llvm = self.llvm_ty(ty);
                self.print_raw("(");
                for (index, member) in members.iter().enumerate() {
                    if index > 0 {
                        self.print_raw(", ");
                    }
                    let raw = self.fresh();
                    self.emit(&format!("{raw} = extractvalue {llvm} {value}, {index}"));
                    let raw = if self.types.get(*member) == Type::Bool {
                        let out = self.fresh();
                        self.emit(&format!("{out} = trunc i8 {raw} to i1"));
                        out
                    } else {
                        raw
                    };
                    self.print_value(*member, &raw, true);
                }
                // Python spells a one-member tuple `(1,)`.
                if members.len() == 1 {
                    self.print_raw(",");
                }
                self.print_raw(")");
            }
            _ => {}
        }
    }

    /// `[1, 2]`: an inline loop writing the elements with `, ` between them.
    fn print_list(&mut self, elem: TypeId, list: &str) {
        self.print_raw("[");
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
        let value = self.load_slot(&slot, elem, Tbaa::ListData);
        let step = self.binop("add", "i64", &index, "1");
        self.emit(&format!("store i64 {step}, ptr {cursor}, align 8"));
        self.print_value(elem, &value, true);
        self.br(&head);

        self.start_block(&end);
        self.print_raw("]");
    }
}
