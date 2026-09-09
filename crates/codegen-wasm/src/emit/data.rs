//! Lowering of the M2 data types: `list<T>`, `str`, `tuple` and class
//! instances (DESIGN §3.8, §4.1, §4.2).
//!
//! This is the wasm twin of `crates/codegen/src/emit/data.rs` and keeps its
//! structure: everything on a hot path is emitted inline — the bounds check,
//! the element load and store, `len`, the `append` fast path, field access and
//! the `str` header reads — and the runtime is only called for the slow paths.
//!
//! Memory layouts (DESIGN §6.2.1: a pointer is four bytes here, so only
//! pointer-shaped members move relative to the native backend):
//!
//! ```text
//! list<T>   ptr -> { i64 len, i64 cap, i32 data }   (24 bytes, data at 16)
//!                                  data -> cap * sizeof(T) bytes
//! str       ptr -> { i64 byte_len, i64 char_len, [n x i8] }
//! class C   ptr -> the fields at the offsets `layout` computes
//! tuple     scalarised into locals; written member by member when it has to
//!           live in memory
//! ```
//!
//! `bool` is `i32` in a local and one byte in every one of those containers,
//! so element and field access widen on the way in and narrow on the way out.

use typhoon_sema::hir;
use typhoon_sema::types::{ClassId, Type, TypeId};
use wasm_encoder::{BlockType, Instruction, MemArg, ValType};

use super::Emitter;
use crate::layout::{
    LIST_CAP_OFFSET, LIST_DATA_OFFSET, LIST_LEN_OFFSET, STR_CHAR_LEN_OFFSET, STR_HEADER,
};
use crate::rt::Rt;

/// A `MemArg` for a naturally aligned access to memory 0.
fn mem(offset: u32, size: u32) -> MemArg {
    MemArg {
        offset: u64::from(offset),
        align: size.trailing_zeros(),
        memory_index: 0,
    }
}

impl<'p> Emitter<'p> {
    // -- memory ----------------------------------------------------------

    /// Pushes the value of type `ty` stored at `address + offset`.
    ///
    /// A tuple is read member by member, which is what makes a tuple in a list
    /// or a class field work at all: it has no wasm representation of its own.
    pub(crate) fn load_value(&mut self, address: u32, ty: TypeId, offset: u32) {
        match self.types.get(ty) {
            Type::Int => {
                self.ins(Instruction::LocalGet(address));
                self.ins(Instruction::I64Load(mem(offset, 8)));
            }
            Type::Float => {
                self.ins(Instruction::LocalGet(address));
                self.ins(Instruction::F64Load(mem(offset, 8)));
            }
            Type::Bool => {
                self.ins(Instruction::LocalGet(address));
                self.ins(Instruction::I32Load8U(mem(offset, 1)));
            }
            Type::Str | Type::List(_) | Type::Class(_) | Type::Optional(_) => {
                self.ins(Instruction::LocalGet(address));
                self.ins(Instruction::I32Load(mem(offset, 4)));
            }
            Type::Tuple(_) => {
                let members = self.layout.tuple_members(ty);
                let offsets = self.layout.field_offsets(&members);
                for (member, member_offset) in members.iter().zip(offsets) {
                    self.load_value(address, *member, offset + member_offset);
                }
            }
            Type::Unit | Type::Error => {}
        }
    }

    /// Stores the previously spilled `value` of type `ty` at `address + offset`.
    pub(crate) fn store_value(&mut self, address: u32, ty: TypeId, offset: u32, value: &[u32]) {
        match self.types.get(ty) {
            Type::Int => {
                self.ins(Instruction::LocalGet(address));
                self.ins(Instruction::LocalGet(value[0]));
                self.ins(Instruction::I64Store(mem(offset, 8)));
            }
            Type::Float => {
                self.ins(Instruction::LocalGet(address));
                self.ins(Instruction::LocalGet(value[0]));
                self.ins(Instruction::F64Store(mem(offset, 8)));
            }
            Type::Bool => {
                self.ins(Instruction::LocalGet(address));
                self.ins(Instruction::LocalGet(value[0]));
                self.ins(Instruction::I32Store8(mem(offset, 1)));
            }
            Type::Str | Type::List(_) | Type::Class(_) | Type::Optional(_) => {
                self.ins(Instruction::LocalGet(address));
                self.ins(Instruction::LocalGet(value[0]));
                self.ins(Instruction::I32Store(mem(offset, 4)));
            }
            Type::Tuple(_) => {
                let members = self.layout.tuple_members(ty);
                let offsets = self.layout.field_offsets(&members);
                let mut start = 0;
                for (member, member_offset) in members.iter().zip(offsets) {
                    let width = self.flat(*member).len();
                    let slice: Vec<u32> = value[start..start + width].to_vec();
                    self.store_value(address, *member, offset + member_offset, &slice);
                    start += width;
                }
            }
            Type::Unit | Type::Error => {}
        }
    }

    // -- str ---------------------------------------------------------------

    /// Replaces the `str` on the stack with its byte length.
    pub(crate) fn str_byte_len(&mut self) {
        self.ins(Instruction::I64Load(mem(0, 8)));
    }

    /// Replaces the `str` on the stack with its code-point count — `len(s)`,
    /// O(1) (DESIGN §4.3).
    pub(crate) fn str_char_len(&mut self) {
        self.ins(Instruction::I64Load(mem(STR_CHAR_LEN_OFFSET, 8)));
    }

    /// Pushes the pointer to a `str`'s bytes and its byte length, the two
    /// arguments `ty_print_str` takes.
    fn str_parts(&mut self, value: u32) {
        self.ins(Instruction::LocalGet(value));
        self.ins(Instruction::I32Const(STR_HEADER as i32));
        self.ins(Instruction::I32Add);
        self.ins(Instruction::LocalGet(value));
        self.str_byte_len();
        self.ins(Instruction::I32WrapI64);
    }

    /// One of the `str` methods of DESIGN §4.6, all of them runtime calls:
    /// none is on a hot path and all of them allocate.
    pub(crate) fn str_method(
        &mut self,
        func: &'p hir::Function,
        op: hir::StrMethod,
        recv: &hir::Expr,
        args: &[hir::Expr],
    ) {
        self.expr(func, recv);
        for arg in args {
            self.expr(func, arg);
        }
        self.call_rt(match op {
            hir::StrMethod::Upper => Rt::StrUpper,
            hir::StrMethod::Lower => Rt::StrLower,
            hir::StrMethod::Strip => Rt::StrStrip,
            hir::StrMethod::Split => Rt::StrSplit,
            hir::StrMethod::Join => Rt::StrJoin,
            hir::StrMethod::StartsWith => Rt::StrStartsWith,
            hir::StrMethod::EndsWith => Rt::StrEndsWith,
            hir::StrMethod::Find => Rt::StrFind,
            hir::StrMethod::Replace => Rt::StrReplace,
        });
        // The predicates already hand back 0 or 1, which is what a `bool` is.
    }

    // -- list --------------------------------------------------------------

    /// Replaces the `list<T>` on the stack with its length.
    pub(crate) fn list_len_on_stack(&mut self) {
        self.ins(Instruction::I64Load(mem(LIST_LEN_OFFSET, 8)));
    }

    /// Pushes the length of the list held in `list`.
    fn list_len(&mut self, list: u32) {
        self.ins(Instruction::LocalGet(list));
        self.list_len_on_stack();
    }

    /// Overwrites a list's length, the only header field the emitter writes
    /// itself (growth is the runtime's job).
    fn set_list_len(&mut self, list: u32, len: u32) {
        self.ins(Instruction::LocalGet(list));
        self.ins(Instruction::LocalGet(len));
        self.ins(Instruction::I64Store(mem(LIST_LEN_OFFSET, 8)));
    }

    /// A fresh local holding the address of a list's element buffer.
    fn list_data(&mut self, list: u32) -> u32 {
        let data = self.scratch(ValType::I32);
        self.ins(Instruction::LocalGet(list));
        self.ins(Instruction::I32Load(mem(LIST_DATA_OFFSET, 4)));
        self.ins(Instruction::LocalSet(data));
        data
    }

    /// Turns a possibly-negative index into a checked one (DESIGN §4.3:
    /// Python's negative indices, a panic when out of range), returning a
    /// fresh local holding it.
    ///
    /// The fast path is a single unsigned compare, because an unsigned `<`
    /// against the length rejects a negative index at the same time; only the
    /// rejected case runs the `+ len` fix-up.
    fn checked_index(&mut self, index: u32, len: u32) -> u32 {
        let slot = self.scratch(ValType::I64);
        self.ins(Instruction::LocalGet(index));
        self.ins(Instruction::LocalSet(slot));

        self.ins(Instruction::LocalGet(index));
        self.ins(Instruction::LocalGet(len));
        self.ins(Instruction::I64LtU);
        self.ins(Instruction::I32Eqz);
        self.open_if(BlockType::Empty);
        self.ins(Instruction::LocalGet(index));
        self.ins(Instruction::LocalGet(len));
        self.ins(Instruction::I64Add);
        self.ins(Instruction::LocalSet(slot));
        // valid = index < 0 && index + len < len (unsigned)
        self.ins(Instruction::LocalGet(index));
        self.ins(Instruction::I64Const(0));
        self.ins(Instruction::I64LtS);
        self.ins(Instruction::LocalGet(slot));
        self.ins(Instruction::LocalGet(len));
        self.ins(Instruction::I64LtU);
        self.ins(Instruction::I32And);
        self.ins(Instruction::I32Eqz);
        self.panic_if("list index out of range");
        self.end();

        slot
    }

    /// A fresh local holding the address of element `index` of `data`.
    fn elem_slot(&mut self, data: u32, elem: TypeId, index: u32) -> u32 {
        let size = self.layout.size_of(elem);
        let slot = self.scratch(ValType::I32);
        self.ins(Instruction::LocalGet(data));
        self.ins(Instruction::LocalGet(index));
        self.ins(Instruction::I32WrapI64);
        self.ins(Instruction::I32Const(size as i32));
        self.ins(Instruction::I32Mul);
        self.ins(Instruction::I32Add);
        self.ins(Instruction::LocalSet(slot));
        slot
    }

    /// A list literal: one allocation, then the elements stored in order.
    pub(crate) fn list_new(&mut self, func: &'p hir::Function, elem: TypeId, items: &[hir::Expr]) {
        let values: Vec<Vec<u32>> = items
            .iter()
            .map(|item| self.spill_expr(func, item))
            .collect();
        let list = self.list_alloc(elem, items.len() as i64);
        if !values.is_empty() {
            let data = self.list_data(list);
            let size = self.layout.size_of(elem);
            for (index, value) in values.iter().enumerate() {
                let slot = self.scratch(ValType::I32);
                self.ins(Instruction::LocalGet(data));
                self.ins(Instruction::I32Const((index as u32 * size) as i32));
                self.ins(Instruction::I32Add);
                self.ins(Instruction::LocalSet(slot));
                self.store_value(slot, elem, 0, value);
            }
            let len = self.scratch(ValType::I64);
            self.ins(Instruction::I64Const(values.len() as i64));
            self.ins(Instruction::LocalSet(len));
            self.set_list_len(list, len);
        }
        self.ins(Instruction::LocalGet(list));
    }

    /// `ty_list_new(elem_size, atomic, cap)`, into a fresh local.
    fn list_alloc(&mut self, elem: TypeId, cap: i64) -> u32 {
        let size = i64::from(self.layout.size_of(elem));
        let atomic = self.atomic_flag(elem);
        self.ins(Instruction::I64Const(size));
        self.ins(Instruction::I32Const(atomic));
        self.ins(Instruction::I64Const(cap));
        self.call_rt(Rt::ListNew);
        let list = self.scratch(ValType::I32);
        self.ins(Instruction::LocalSet(list));
        list
    }

    /// `xs[i]`: bounds check, then one load.
    pub(crate) fn list_get(
        &mut self,
        func: &'p hir::Function,
        list: &hir::Expr,
        index: &hir::Expr,
        elem: TypeId,
    ) {
        let list = self.spill_expr(func, list)[0];
        let index = self.spill_expr(func, index)[0];
        let len = self.scratch(ValType::I64);
        self.list_len(list);
        self.ins(Instruction::LocalSet(len));
        let resolved = self.checked_index(index, len);
        let data = self.list_data(list);
        let slot = self.elem_slot(data, elem, resolved);
        self.load_value(slot, elem, 0);
    }

    /// `xs.append(v)`: inline fast path, `ty_list_grow` only when full.
    pub(crate) fn list_append(
        &mut self,
        func: &'p hir::Function,
        list: &hir::Expr,
        value: &hir::Expr,
    ) {
        let elem = value.ty;
        let list = self.spill_expr(func, list)[0];
        let value = self.spill_expr(func, value);

        let len = self.scratch(ValType::I64);
        self.list_len(list);
        self.ins(Instruction::LocalSet(len));
        let next = self.scratch(ValType::I64);
        self.ins(Instruction::LocalGet(len));
        self.ins(Instruction::I64Const(1));
        self.ins(Instruction::I64Add);
        self.ins(Instruction::LocalSet(next));

        // full = len >= cap
        self.ins(Instruction::LocalGet(len));
        self.ins(Instruction::LocalGet(list));
        self.ins(Instruction::I64Load(mem(LIST_CAP_OFFSET, 8)));
        self.ins(Instruction::I64GeS);
        self.open_if(BlockType::Empty);
        let size = i64::from(self.layout.size_of(elem));
        let atomic = self.atomic_flag(elem);
        self.ins(Instruction::LocalGet(list));
        self.ins(Instruction::I64Const(size));
        self.ins(Instruction::I32Const(atomic));
        self.ins(Instruction::LocalGet(next));
        self.call_rt(Rt::ListGrow);
        self.end();

        let data = self.list_data(list);
        let slot = self.elem_slot(data, elem, len);
        self.store_value(slot, elem, 0, &value);
        self.set_list_len(list, next);
    }

    /// `xs.pop()`: panics on an empty list, otherwise one load and one store.
    pub(crate) fn list_pop(&mut self, func: &'p hir::Function, list: &hir::Expr, elem: TypeId) {
        let list = self.spill_expr(func, list)[0];
        let len = self.scratch(ValType::I64);
        self.list_len(list);
        self.ins(Instruction::LocalSet(len));

        self.ins(Instruction::LocalGet(len));
        self.ins(Instruction::I64Const(0));
        self.ins(Instruction::I64LeS);
        self.panic_if("pop from empty list");

        let last = self.scratch(ValType::I64);
        self.ins(Instruction::LocalGet(len));
        self.ins(Instruction::I64Const(1));
        self.ins(Instruction::I64Sub);
        self.ins(Instruction::LocalSet(last));

        let data = self.list_data(list);
        let slot = self.elem_slot(data, elem, last);
        self.load_value(slot, elem, 0);
        let value = self.spill(elem);
        self.set_list_len(list, last);
        self.push_locals(&value);
    }

    /// `xs.insert(i, v)`: the runtime makes room and hands back the slot.
    pub(crate) fn list_insert(
        &mut self,
        func: &'p hir::Function,
        list: &hir::Expr,
        index: &hir::Expr,
        value: &hir::Expr,
    ) {
        let elem = value.ty;
        let list = self.spill_expr(func, list)[0];
        let index = self.spill_expr(func, index)[0];
        let value = self.spill_expr(func, value);

        let size = i64::from(self.layout.size_of(elem));
        let atomic = self.atomic_flag(elem);
        self.ins(Instruction::LocalGet(list));
        self.ins(Instruction::LocalGet(index));
        self.ins(Instruction::I64Const(size));
        self.ins(Instruction::I32Const(atomic));
        self.call_rt(Rt::ListInsertSlot);
        let slot = self.scratch(ValType::I32);
        self.ins(Instruction::LocalSet(slot));
        self.store_value(slot, elem, 0, &value);
    }

    /// `xs.clear()`: the buffer is kept, only the length is reset.
    pub(crate) fn list_clear(&mut self, func: &'p hir::Function, list: &hir::Expr) {
        let list = self.spill_expr(func, list)[0];
        let zero = self.scratch(ValType::I64);
        self.ins(Instruction::I64Const(0));
        self.ins(Instruction::LocalSet(zero));
        self.set_list_len(list, zero);
    }

    /// `xs + ys`: a fresh list holding both.
    pub(crate) fn list_concat(
        &mut self,
        func: &'p hir::Function,
        lhs: &hir::Expr,
        rhs: &hir::Expr,
        elem: TypeId,
    ) {
        self.expr(func, lhs);
        self.expr(func, rhs);
        let size = i64::from(self.layout.size_of(elem));
        let atomic = self.atomic_flag(elem);
        self.ins(Instruction::I64Const(size));
        self.ins(Instruction::I32Const(atomic));
        self.call_rt(Rt::ListConcat);
    }

    /// `xs[i] = v`.
    pub(crate) fn emit_set_index(
        &mut self,
        func: &'p hir::Function,
        list: &hir::Expr,
        index: &hir::Expr,
        value: &hir::Expr,
    ) {
        let elem = value.ty;
        let list = self.spill_expr(func, list)[0];
        let index = self.spill_expr(func, index)[0];
        let value = self.spill_expr(func, value);

        let len = self.scratch(ValType::I64);
        self.list_len(list);
        self.ins(Instruction::LocalSet(len));
        let resolved = self.checked_index(index, len);
        let data = self.list_data(list);
        let slot = self.elem_slot(data, elem, resolved);
        self.store_value(slot, elem, 0, &value);
    }

    // -- class -------------------------------------------------------------

    /// `C(field=…)`: one allocation, then every field stored in order.
    ///
    /// A class with no pointer-holding field goes into an unscanned block
    /// (DESIGN §5.1) — which on wasm is the same block as any other, since
    /// nothing is ever collected, but the flag is still passed so that the two
    /// backends make the same runtime calls.
    pub(crate) fn class_new(
        &mut self,
        func: &'p hir::Function,
        class: ClassId,
        fields: &[hir::Expr],
        eval_order: &[u32],
    ) {
        // Written order first, then the constant defaults (DESIGN §3.2).
        let mut values: Vec<Option<Vec<u32>>> = vec![None; fields.len()];
        for index in eval_order {
            let index = *index as usize;
            values[index] = Some(self.spill_expr(func, &fields[index]));
        }
        for (index, value) in values.iter_mut().enumerate() {
            if value.is_none() {
                let field = &fields[index];
                self.expr(func, field);
                *value = Some(self.spill(field.ty));
            }
        }

        let size = self.layout.class_size(class);
        let atomic = !self.types.class(class).has_pointers;
        self.ins(Instruction::I32Const(size as i32));
        self.call_rt(if atomic { Rt::AllocAtomic } else { Rt::Alloc });
        let obj = self.scratch(ValType::I32);
        self.ins(Instruction::LocalSet(obj));

        let types = self.layout.class_fields(class);
        let offsets = self.layout.class_offsets(class);
        for (index, value) in values.iter().enumerate() {
            let value = value.clone().expect("filled above");
            self.store_value(obj, types[index], offsets[index], &value);
        }
        self.ins(Instruction::LocalGet(obj));
    }

    /// `obj.field = value`.
    pub(crate) fn emit_set_field(
        &mut self,
        func: &'p hir::Function,
        obj: &hir::Expr,
        class: ClassId,
        field: u32,
        value: &hir::Expr,
    ) {
        let ty = self.types.class(class).fields[field as usize].ty;
        let obj = self.spill_expr(func, obj)[0];
        let value = self.spill_expr(func, value);
        let offset = self.layout.class_offsets(class)[field as usize];
        self.store_value(obj, ty, offset, &value);
    }

    // -- iteration ----------------------------------------------------------

    /// `for x in <list or str>` (DESIGN §3.5).
    ///
    /// The cursor advances at the top of the body, before the user's
    /// statements, so that `continue` needs no work of its own and can branch
    /// straight back to the loop header.
    pub(crate) fn emit_for_each(
        &mut self,
        func: &'p hir::Function,
        var: hir::LocalId,
        iter: &hir::Expr,
        over: hir::IterKind,
        body: &hir::Block,
    ) {
        let iterable = self.spill_expr(func, iter)[0];
        // A `str` is immutable, so its byte length is loop-invariant.
        let bound = match over {
            hir::IterKind::List => None,
            hir::IterKind::Str => {
                let bound = self.scratch(ValType::I64);
                self.ins(Instruction::LocalGet(iterable));
                self.str_byte_len();
                self.ins(Instruction::LocalSet(bound));
                Some(bound)
            }
        };

        let cursor = self.scratch(ValType::I64);
        self.ins(Instruction::I64Const(0));
        self.ins(Instruction::LocalSet(cursor));

        let break_label = self.open_block(BlockType::Empty);
        let head = self.open_loop(BlockType::Empty);

        self.ins(Instruction::LocalGet(cursor));
        // A list may grow or shrink inside the body, exactly as in Python, so
        // its length is re-read every time round.
        match bound {
            Some(bound) => self.ins(Instruction::LocalGet(bound)),
            None => self.list_len(iterable),
        }
        self.ins(Instruction::I64LtS);
        self.ins(Instruction::I32Eqz);
        self.br_if(break_label);

        let var_ty = func.locals[var.index()].ty;
        match over {
            hir::IterKind::List => {
                let data = self.list_data(iterable);
                let slot = self.elem_slot(data, var_ty, cursor);
                self.load_value(slot, var_ty, 0);
                self.store_local(var);
                self.ins(Instruction::LocalGet(cursor));
                self.ins(Instruction::I64Const(1));
                self.ins(Instruction::I64Add);
                self.ins(Instruction::LocalSet(cursor));
            }
            hir::IterKind::Str => {
                // The runtime hands back a length-1 `str`; ASCII comes from a
                // static table, so an ASCII scan allocates nothing.
                self.ins(Instruction::LocalGet(iterable));
                self.ins(Instruction::LocalGet(cursor));
                self.call_rt(Rt::StrCharAt);
                let ch = self.scratch(ValType::I32);
                self.ins(Instruction::LocalTee(ch));
                self.store_local(var);
                self.ins(Instruction::LocalGet(cursor));
                self.ins(Instruction::LocalGet(ch));
                self.str_byte_len();
                self.ins(Instruction::I64Add);
                self.ins(Instruction::LocalSet(cursor));
            }
        }

        self.loops.push((break_label, head));
        self.block(func, body);
        self.loops.pop();

        self.br(head);
        self.end();
        self.end();
    }

    // -- equality and membership -------------------------------------------

    /// `a == b` for two spilled values of the same type, leaving an `i32`.
    ///
    /// Recurses through lists and tuples, emitting a loop for every list level
    /// (DESIGN §4.6: containers compare element by element).
    pub(crate) fn values_equal(&mut self, ty: TypeId, lhs: &[u32], rhs: &[u32]) {
        match self.types.get(ty) {
            Type::Int => {
                self.ins(Instruction::LocalGet(lhs[0]));
                self.ins(Instruction::LocalGet(rhs[0]));
                self.ins(Instruction::I64Eq);
            }
            Type::Float => {
                self.ins(Instruction::LocalGet(lhs[0]));
                self.ins(Instruction::LocalGet(rhs[0]));
                self.ins(Instruction::F64Eq);
            }
            Type::Bool => {
                self.ins(Instruction::LocalGet(lhs[0]));
                self.ins(Instruction::LocalGet(rhs[0]));
                self.ins(Instruction::I32Eq);
            }
            Type::Str => {
                self.ins(Instruction::LocalGet(lhs[0]));
                self.ins(Instruction::LocalGet(rhs[0]));
                self.call_rt(Rt::StrEq);
            }
            Type::Tuple(_) => {
                let members = self.layout.tuple_members(ty);
                let mut start = 0;
                let mut first = true;
                for member in &members {
                    let width = self.flat(*member).len();
                    let a: Vec<u32> = lhs[start..start + width].to_vec();
                    let b: Vec<u32> = rhs[start..start + width].to_vec();
                    self.values_equal(*member, &a, &b);
                    if !first {
                        self.ins(Instruction::I32And);
                    }
                    first = false;
                    start += width;
                }
                if first {
                    self.ins(Instruction::I32Const(1));
                }
            }
            Type::List(elem) => self.lists_equal(elem, lhs[0], rhs[0]),
            _ => self.ins(Instruction::I32Const(0)),
        }
    }

    /// Element-by-element list equality, as an inline loop.
    fn lists_equal(&mut self, elem: TypeId, lhs: u32, rhs: u32) {
        let result = self.scratch(ValType::I32);
        self.ins(Instruction::I32Const(0));
        self.ins(Instruction::LocalSet(result));

        let left_len = self.scratch(ValType::I64);
        self.list_len(lhs);
        self.ins(Instruction::LocalSet(left_len));
        let cursor = self.scratch(ValType::I64);
        self.ins(Instruction::I64Const(0));
        self.ins(Instruction::LocalSet(cursor));

        self.ins(Instruction::LocalGet(left_len));
        self.list_len(rhs);
        self.ins(Instruction::I64Eq);
        self.open_if(BlockType::Empty);

        let done = self.open_block(BlockType::Empty);
        let head = self.open_loop(BlockType::Empty);

        self.ins(Instruction::LocalGet(cursor));
        self.ins(Instruction::LocalGet(left_len));
        self.ins(Instruction::I64LtS);
        self.ins(Instruction::I32Eqz);
        self.open_if(BlockType::Empty);
        self.ins(Instruction::I32Const(1));
        self.ins(Instruction::LocalSet(result));
        self.br(done);
        self.end();

        let left_data = self.list_data(lhs);
        let right_data = self.list_data(rhs);
        let left_slot = self.elem_slot(left_data, elem, cursor);
        let right_slot = self.elem_slot(right_data, elem, cursor);
        self.load_value(left_slot, elem, 0);
        let a = self.spill(elem);
        self.load_value(right_slot, elem, 0);
        let b = self.spill(elem);
        self.ins(Instruction::LocalGet(cursor));
        self.ins(Instruction::I64Const(1));
        self.ins(Instruction::I64Add);
        self.ins(Instruction::LocalSet(cursor));

        self.values_equal(elem, &a, &b);
        self.ins(Instruction::I32Eqz);
        self.br_if(done);
        self.br(head);

        self.end();
        self.end();
        self.end();

        self.ins(Instruction::LocalGet(result));
    }

    /// `v in xs` / `v in s`, and their `not in` forms.
    pub(crate) fn contains(
        &mut self,
        func: &'p hir::Function,
        haystack: &hir::Expr,
        needle: &hir::Expr,
        negated: bool,
    ) {
        let container_ty = haystack.ty;
        let elem_ty = needle.ty;
        match self.types.get(container_ty) {
            // A substring search is a string algorithm, not a hot loop.
            Type::Str => {
                self.expr(func, haystack);
                self.expr(func, needle);
                self.call_rt(Rt::StrContains);
            }
            _ => {
                let container = self.spill_expr(func, haystack)[0];
                let value = self.spill_expr(func, needle);
                self.list_contains(container, elem_ty, &value);
            }
        }
        if negated {
            self.ins(Instruction::I32Eqz);
        }
    }

    /// A linear scan over a list, emitted inline.
    fn list_contains(&mut self, list: u32, elem: TypeId, value: &[u32]) {
        let result = self.scratch(ValType::I32);
        self.ins(Instruction::I32Const(0));
        self.ins(Instruction::LocalSet(result));

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

        let data = self.list_data(list);
        let slot = self.elem_slot(data, elem, cursor);
        self.load_value(slot, elem, 0);
        let element = self.spill(elem);
        self.ins(Instruction::LocalGet(cursor));
        self.ins(Instruction::I64Const(1));
        self.ins(Instruction::I64Add);
        self.ins(Instruction::LocalSet(cursor));

        self.values_equal(elem, &element, value);
        self.open_if(BlockType::Empty);
        self.ins(Instruction::I32Const(1));
        self.ins(Instruction::LocalSet(result));
        self.br(done);
        self.end();
        self.br(head);

        self.end();
        self.end();

        self.ins(Instruction::LocalGet(result));
    }

    // -- printing ----------------------------------------------------------

    /// Writes a literal run of bytes to stdout, for the punctuation around a
    /// container.
    fn print_raw(&mut self, text: &str) {
        let (global, len) = self.raw_bytes(text);
        self.ins(Instruction::GlobalGet(global));
        self.ins(Instruction::I32Const(len as i32));
        self.call_rt(Rt::PrintStr);
    }

    /// Writes one spilled value the way `print` does (DESIGN §4.6).
    ///
    /// `quoted` is set inside a container, where a `str` prints as its repr —
    /// `["a", "b"]` — instead of verbatim.
    pub(crate) fn print_value(&mut self, ty: TypeId, value: &[u32], quoted: bool) {
        match self.types.get(ty) {
            Type::Int => {
                self.ins(Instruction::LocalGet(value[0]));
                self.call_rt(Rt::PrintInt);
            }
            Type::Float => {
                self.ins(Instruction::LocalGet(value[0]));
                self.call_rt(Rt::PrintFloat);
            }
            Type::Bool => {
                self.ins(Instruction::LocalGet(value[0]));
                self.call_rt(Rt::PrintBool);
            }
            Type::Str if quoted => {
                self.ins(Instruction::LocalGet(value[0]));
                self.call_rt(Rt::PrintStrRepr);
            }
            Type::Str => {
                self.str_parts(value[0]);
                self.call_rt(Rt::PrintStr);
            }
            Type::List(elem) => self.print_list(elem, value[0]),
            Type::Tuple(_) => {
                let members = self.layout.tuple_members(ty);
                self.print_raw("(");
                let mut start = 0;
                for (index, member) in members.iter().enumerate() {
                    if index > 0 {
                        self.print_raw(", ");
                    }
                    let width = self.flat(*member).len();
                    let slice: Vec<u32> = value[start..start + width].to_vec();
                    self.print_value(*member, &slice, true);
                    start += width;
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
    fn print_list(&mut self, elem: TypeId, list: u32) {
        self.print_raw("[");

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
        let value = self.spill(elem);
        self.ins(Instruction::LocalGet(cursor));
        self.ins(Instruction::I64Const(1));
        self.ins(Instruction::I64Add);
        self.ins(Instruction::LocalSet(cursor));
        self.print_value(elem, &value, true);
        self.br(head);

        self.end();
        self.end();

        self.print_raw("]");
    }
}
