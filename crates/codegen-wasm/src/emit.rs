//! The wasm module emitter: typed HIR in, a binary module out.
//!
//! The shape of the emitted module (DESIGN §6.2.1):
//!
//! ```text
//! (import "env" "memory" (memory 0))        ;; the runtime instance's heap
//! (import "env" "ty_print_int" (func …))    ;; every runtime call site used
//! (global $lit.0 (mut i32) (i32.const 0))   ;; one per string literal
//! (data …)                                  ;; passive, copied by _start
//! (func $ty_user_main …)                    ;; one per user function
//! (func $_start (export "_start") …)        ;; init, run main, exit
//! ```
//!
//! Everything is a direct translation of the LLVM backend
//! (`crates/codegen/src/emit.rs`), instruction for instruction, so that the two
//! backends agree on every observable: wrapping integer arithmetic, flooring
//! `//` and `%`, the zero-divisor panic, list bounds checks with the negative
//! index fix-up, `print` formatting, and the evaluation order of arguments.
//! Where wasm has no equivalent instruction — `pow`, saturating float-to-int,
//! `abs` on `i64` — the same computation is spelled out or handed to the
//! runtime.
//!
//! # Values
//!
//! `int` is `i64`, `float` is `f64`, `bool` is `i32` in a local and one byte in
//! memory, and every reference (`str`, `list<T>`, a class instance, `C | None`)
//! is an `i32` address into the shared linear memory, with `None` as 0.
//!
//! A `tuple` is a value aggregate with no wasm counterpart, so it is
//! *scalarised*: a tuple-typed expression leaves one wasm value per (recursive)
//! member on the operand stack, a tuple-typed local occupies that many wasm
//! locals, and a tuple-typed parameter or result becomes that many parameters
//! or results (wasm's multi-value returns). Tuples only reach memory when they
//! are stored into a list element, a class field or another tuple already in
//! memory, and then they are written member by member at the offsets
//! [`crate::layout`] computes.

use std::collections::HashMap;

use typhoon_sema::hir;
use typhoon_sema::types::{Type, TypeId, TypeTable};
use wasm_encoder::{
    BlockType, CodeSection, ConstExpr, DataCountSection, DataSection, EntityType, ExportKind,
    ExportSection, Function, FunctionSection, GlobalSection, GlobalType, ImportSection,
    Instruction, MemArg, MemoryType, Module, NameMap, NameSection, TypeSection, ValType,
};

use crate::layout::{Layout, STR_CHAR_LEN_OFFSET, STR_HEADER};
use crate::rt::Rt;

mod data;
mod exprs;

/// Placeholder base for a call to a runtime import, resolved by
/// [`Emitter::resolve_calls`] once the import list is known.
const RT_CALL_BASE: u32 = 0x8000_0000;
/// Placeholder base for a call to a user function.
const USER_CALL_BASE: u32 = 0x4000_0000;

/// The name the entry point is exported under.
pub const START_EXPORT: &str = "_start";

/// Emits the whole program as one binary wasm module.
pub fn emit_module(program: &hir::Program) -> Vec<u8> {
    let mut emitter = Emitter::new(program);
    emitter.emit_program();
    emitter.finish()
}

/// A string literal or raw byte blob materialised by `_start`.
///
/// The bytes live in a passive data segment and are copied into a fresh
/// allocation at start-up (DESIGN §6.2.1): the user module shares the
/// runtime's memory, so it has no static address space of its own to put them
/// at.
struct Literal {
    /// The mutable `i32` global holding the address.
    global: u32,
    /// The passive data segment holding the bytes.
    segment: u32,
    /// The payload.
    bytes: Vec<u8>,
    /// `Some(char_len)` for a `str`, which gets a `{ byte_len, char_len }`
    /// header; `None` for a bare run of bytes (a panic message, the `[`, `, `
    /// and `]` `print` writes around a list).
    char_len: Option<i64>,
}

impl Literal {
    /// Bytes to allocate: the payload plus a `str`'s header.
    fn alloc_size(&self) -> u32 {
        let header = if self.char_len.is_some() {
            STR_HEADER
        } else {
            0
        };
        header + self.bytes.len() as u32
    }
}

/// One emitted function body, before call indices are resolved.
struct Body {
    /// The locals *after* the parameters, in declaration order.
    locals: Vec<ValType>,
    /// The instruction stream, still carrying placeholder call indices.
    code: Vec<Instruction<'static>>,
    /// The module type index of the function's signature.
    type_index: u32,
    /// The name recorded in the emitted `name` custom section — nothing reads
    /// it programmatically, but it makes a WAT dump readable.
    name: String,
}

/// Emitter state: the module tables plus the function being emitted.
pub(crate) struct Emitter<'p> {
    program: &'p hir::Program,
    types: &'p TypeTable,
    layout: Layout<'p>,

    // -- module ------------------------------------------------------------
    /// Deduplicated function signatures, `(params, results)` to type index.
    signatures: Vec<(Vec<ValType>, Vec<ValType>)>,
    /// Runtime functions actually called, in first-use order.
    used_rt: Vec<Rt>,
    /// Interned string literals, by contents.
    strings: HashMap<String, usize>,
    /// Interned raw byte blobs, by contents.
    blobs: HashMap<String, usize>,
    /// Every literal, in the order `_start` initialises them.
    literals: Vec<Literal>,
    /// One emitted body per user function, then `_start`.
    bodies: Vec<Body>,

    // -- per function ------------------------------------------------------
    code: Vec<Instruction<'static>>,
    /// Every wasm local of the function, parameters first.
    locals: Vec<ValType>,
    /// How many of `locals` are parameters.
    n_params: u32,
    /// The wasm locals holding each HIR local.
    slots: Vec<Vec<u32>>,
    /// How many nested `block` / `loop` / `if` frames are open.
    depth: u32,
    /// `(break target, continue target)` label levels for every enclosing loop.
    loops: Vec<(u32, u32)>,
}

impl<'p> Emitter<'p> {
    fn new(program: &'p hir::Program) -> Emitter<'p> {
        Emitter {
            program,
            types: &program.types,
            layout: Layout::new(&program.types),
            signatures: Vec::new(),
            used_rt: Vec::new(),
            strings: HashMap::new(),
            blobs: HashMap::new(),
            literals: Vec::new(),
            bodies: Vec::new(),
            code: Vec::new(),
            locals: Vec::new(),
            n_params: 0,
            slots: Vec::new(),
            depth: 0,
            loops: Vec::new(),
        }
    }

    // -- module assembly ---------------------------------------------------

    fn emit_program(&mut self) {
        for func in &self.program.functions {
            self.emit_function(func);
        }
        self.emit_entry_point();
    }

    /// Assembles the sections into the final module.
    fn finish(mut self) -> Vec<u8> {
        self.resolve_calls();

        let order = self.import_order();
        let n_imports = order.len() as u32;
        let mut module = Module::new();

        let mut types = TypeSection::new();
        for (params, results) in &self.signatures {
            types
                .ty()
                .function(params.iter().copied(), results.iter().copied());
        }
        module.section(&types);

        let mut imports = ImportSection::new();
        // A minimum of zero pages accepts whatever memory the runtime module
        // created; the import only has to be no more demanding than the real
        // one.
        imports.import(
            "env",
            "memory",
            EntityType::Memory(MemoryType {
                minimum: 0,
                maximum: None,
                memory64: false,
                shared: false,
                page_size_log2: None,
            }),
        );
        for rt in &order {
            let (params, results) = rt.signature();
            let index = self
                .signatures
                .iter()
                .position(|(p, r)| p == params && r == results)
                .expect("every used signature was interned") as u32;
            imports.import("env", rt.name(), EntityType::Function(index));
        }
        module.section(&imports);

        let mut functions = FunctionSection::new();
        for body in &self.bodies {
            functions.function(body.type_index);
        }
        module.section(&functions);

        if !self.literals.is_empty() {
            let mut globals = GlobalSection::new();
            for _ in &self.literals {
                globals.global(
                    GlobalType {
                        val_type: ValType::I32,
                        mutable: true,
                        shared: false,
                    },
                    &ConstExpr::i32_const(0),
                );
            }
            module.section(&globals);
        }

        let mut exports = ExportSection::new();
        let start_index = n_imports + self.bodies.len() as u32 - 1;
        exports.export(START_EXPORT, ExportKind::Func, start_index);
        module.section(&exports);

        if !self.literals.is_empty() {
            module.section(&DataCountSection {
                count: self.literals.len() as u32,
            });
        }

        let mut code = CodeSection::new();
        for body in &self.bodies {
            let mut function = Function::new_with_locals_types(body.locals.iter().copied());
            for instruction in &body.code {
                function.instruction(instruction);
            }
            function.instruction(&Instruction::End);
            code.function(&function);
        }
        module.section(&code);

        if !self.literals.is_empty() {
            let mut data = DataSection::new();
            for literal in &self.literals {
                data.passive(literal.bytes.iter().copied());
            }
            module.section(&data);
        }

        // A `name` section costs a few hundred bytes and turns a WAT dump — the
        // snapshots of this crate, a browser stack trace, `wasm-objdump` —
        // from indices into the names the programmer wrote.
        let mut names = NameSection::new();
        names.module("typhoon");
        let mut functions = NameMap::new();
        for (index, rt) in order.iter().enumerate() {
            functions.append(index as u32, rt.name());
        }
        for (index, body) in self.bodies.iter().enumerate() {
            functions.append(n_imports + index as u32, &body.name);
        }
        names.functions(&functions);
        module.section(&names);

        module.finish()
    }

    /// The runtime functions this module imports, in the order they appear in
    /// the import section.
    ///
    /// [`Rt::ALL`] rather than the order calls were emitted in, so that adding
    /// a call site somewhere in the middle of a program cannot reshuffle the
    /// imports of the whole module.
    fn import_order(&self) -> Vec<Rt> {
        Rt::ALL
            .iter()
            .copied()
            .filter(|rt| self.used_rt.contains(rt))
            .collect()
    }

    /// Rewrites the placeholder call indices of every body into real function
    /// indices, now that the import list is complete.
    fn resolve_calls(&mut self) {
        let order = self.import_order();
        let n_imports = order.len() as u32;
        let mut rt_index = HashMap::new();
        for (index, rt) in order.iter().enumerate() {
            rt_index.insert(*rt as u32, index as u32);
        }
        for body in &mut self.bodies {
            for instruction in &mut body.code {
                if let Instruction::Call(target) = instruction {
                    if *target >= RT_CALL_BASE {
                        let ordinal = *target - RT_CALL_BASE;
                        *target = *rt_index
                            .get(&ordinal)
                            .expect("a call implies the import was recorded");
                    } else if *target >= USER_CALL_BASE {
                        *target = n_imports + (*target - USER_CALL_BASE);
                    }
                }
            }
        }
    }

    /// The module type index of a signature, interning it on first use.
    fn signature_index(&mut self, params: &[ValType], results: &[ValType]) -> u32 {
        if let Some(index) = self
            .signatures
            .iter()
            .position(|(p, r)| p == params && r == results)
        {
            return index as u32;
        }
        self.signatures.push((params.to_vec(), results.to_vec()));
        (self.signatures.len() - 1) as u32
    }

    /// Records that `rt` is called and returns the placeholder call index.
    pub(crate) fn rt_index(&mut self, rt: Rt) -> u32 {
        if !self.used_rt.contains(&rt) {
            let (params, results) = rt.signature();
            self.signature_index(params, results);
            self.used_rt.push(rt);
        }
        RT_CALL_BASE + rt as u32
    }

    /// Emits a call to a runtime function.
    pub(crate) fn call_rt(&mut self, rt: Rt) {
        let index = self.rt_index(rt);
        self.ins(Instruction::Call(index));
    }

    // -- types -------------------------------------------------------------

    /// The wasm values a typhoon value of `ty` occupies: one for a scalar,
    /// none for unit, and one per (recursive) member for a tuple.
    pub(crate) fn flat(&self, ty: TypeId) -> Vec<ValType> {
        let mut out = Vec::new();
        self.flatten_into(ty, &mut out);
        out
    }

    fn flatten_into(&self, ty: TypeId, out: &mut Vec<ValType>) {
        match self.types.get(ty) {
            Type::Int => out.push(ValType::I64),
            Type::Float => out.push(ValType::F64),
            Type::Bool => out.push(ValType::I32),
            Type::Str | Type::List(_) | Type::Class(_) | Type::Optional(_) => {
                out.push(ValType::I32)
            }
            Type::Tuple(id) => {
                for member in self.types.tuple_members(id).to_vec() {
                    self.flatten_into(member, out);
                }
            }
            Type::Unit | Type::Error => {}
        }
    }

    /// The `i32` flag `ty_alloc*` and the list runtime take: 1 when a value of
    /// `ty` holds no pointer (DESIGN §5.1).
    pub(crate) fn atomic_flag(&self, ty: TypeId) -> i32 {
        i32::from(self.types.is_pointer_free(ty))
    }

    // -- instruction helpers ------------------------------------------------

    /// Appends one instruction.
    pub(crate) fn ins(&mut self, instruction: Instruction<'static>) {
        self.code.push(instruction);
    }

    /// Opens a `block`, returning the label level branches target it with.
    pub(crate) fn open_block(&mut self, ty: BlockType) -> u32 {
        self.ins(Instruction::Block(ty));
        self.depth += 1;
        self.depth
    }

    /// Opens a `loop`, returning the label level branches target it with.
    pub(crate) fn open_loop(&mut self, ty: BlockType) -> u32 {
        self.ins(Instruction::Loop(ty));
        self.depth += 1;
        self.depth
    }

    /// Opens an `if`, returning its label level.
    pub(crate) fn open_if(&mut self, ty: BlockType) -> u32 {
        self.ins(Instruction::If(ty));
        self.depth += 1;
        self.depth
    }

    /// Starts the `else` arm of the innermost `if`.
    pub(crate) fn else_(&mut self) {
        self.ins(Instruction::Else);
    }

    /// Closes the innermost frame.
    pub(crate) fn end(&mut self) {
        self.ins(Instruction::End);
        self.depth -= 1;
    }

    /// `br` to the frame opened at `label`.
    pub(crate) fn br(&mut self, label: u32) {
        let relative = self.depth - label;
        self.ins(Instruction::Br(relative));
    }

    /// `br_if` to the frame opened at `label`.
    pub(crate) fn br_if(&mut self, label: u32) {
        let relative = self.depth - label;
        self.ins(Instruction::BrIf(relative));
    }

    // -- locals -------------------------------------------------------------

    /// A fresh local of type `ty`.
    ///
    /// Scratch locals are never recycled: an expression that needs one asks for
    /// it while it is being emitted, so the count is bounded by the size of the
    /// function's source, and wasm locals cost nothing but a slot in the
    /// prologue.
    pub(crate) fn scratch(&mut self, ty: ValType) -> u32 {
        self.locals.push(ty);
        (self.locals.len() - 1) as u32
    }

    /// Pops a value of type `ty` off the operand stack into fresh locals,
    /// returning them in member order.
    pub(crate) fn spill(&mut self, ty: TypeId) -> Vec<u32> {
        let flat = self.flat(ty);
        let locals: Vec<u32> = flat.iter().map(|vt| self.scratch(*vt)).collect();
        // The last member is on top of the stack.
        for local in locals.iter().rev() {
            self.ins(Instruction::LocalSet(*local));
        }
        locals
    }

    /// Pushes previously spilled locals back onto the operand stack.
    pub(crate) fn push_locals(&mut self, locals: &[u32]) {
        for local in locals {
            self.ins(Instruction::LocalGet(*local));
        }
    }

    /// Pops and discards a value of type `ty`.
    pub(crate) fn drop_value(&mut self, ty: TypeId) {
        for _ in 0..self.flat(ty).len() {
            self.ins(Instruction::Drop);
        }
    }

    // -- functions ----------------------------------------------------------

    fn emit_function(&mut self, func: &'p hir::Function) {
        // Parameters are declared before any other local, so the wasm
        // parameters and the first HIR locals line up one for one.
        for (index, param) in func.params.iter().enumerate() {
            assert_eq!(
                param.index(),
                index,
                "sema declares parameters as the first locals"
            );
        }

        self.code = Vec::new();
        self.locals = Vec::new();
        self.slots = Vec::new();
        self.depth = 0;
        self.loops = Vec::new();

        for local in &func.locals {
            let flat = self.flat(local.ty);
            let start = self.locals.len() as u32;
            self.locals.extend(flat.iter().copied());
            self.slots.push((start..self.locals.len() as u32).collect());
        }
        self.n_params = func
            .params
            .iter()
            .map(|p| self.slots[p.index()].len() as u32)
            .sum();

        let params: Vec<ValType> = self.locals[..self.n_params as usize].to_vec();
        let results = self.flat(func.ret);
        let type_index = self.signature_index(&params, &results);

        self.block(func, &func.body);

        // Falling off the end of a function that returns a value is
        // unreachable — sema proved every path returns — but wasm still wants
        // the body to leave the result on the stack, so say so.
        if !results.is_empty() {
            self.ins(Instruction::Unreachable);
        }

        let body = Body {
            locals: self.locals[self.n_params as usize..].to_vec(),
            code: std::mem::take(&mut self.code),
            type_index,
            name: func.symbol.clone(),
        };
        self.bodies.push(body);
    }

    /// `_start`: start the runtime, materialise the literals, run the user's
    /// `main`, flush and exit.
    fn emit_entry_point(&mut self) {
        self.code = Vec::new();
        self.locals = Vec::new();
        self.slots = Vec::new();
        self.depth = 0;
        self.n_params = 0;

        self.call_rt(Rt::Init);

        for index in 0..self.literals.len() {
            self.emit_literal_init(index);
        }

        let main = USER_CALL_BASE + self.program.main.index() as u32;
        self.ins(Instruction::Call(main));

        self.ins(Instruction::I32Const(0));
        self.call_rt(Rt::Exit);
        // `ty_rt_exit` traps rather than returning; wasm cannot express that,
        // so make the fall-through path a trap too.
        self.ins(Instruction::Unreachable);

        let type_index = self.signature_index(&[], &[]);
        let body = Body {
            locals: self.locals.clone(),
            code: std::mem::take(&mut self.code),
            type_index,
            name: START_EXPORT.to_string(),
        };
        self.bodies.push(body);
    }

    /// Allocates one literal and copies its bytes in from the passive segment.
    fn emit_literal_init(&mut self, index: usize) {
        let (global, segment, size, char_len, len) = {
            let literal = &self.literals[index];
            (
                literal.global,
                literal.segment,
                literal.alloc_size(),
                literal.char_len,
                literal.bytes.len() as u32,
            )
        };

        self.ins(Instruction::I32Const(size as i32));
        self.call_rt(Rt::AllocAtomic);
        self.ins(Instruction::GlobalSet(global));

        let mut payload_offset = 0;
        if let Some(char_len) = char_len {
            payload_offset = STR_HEADER;
            self.ins(Instruction::GlobalGet(global));
            self.ins(Instruction::I64Const(i64::from(len)));
            self.ins(Instruction::I64Store(MemArg {
                offset: 0,
                align: 3,
                memory_index: 0,
            }));
            self.ins(Instruction::GlobalGet(global));
            self.ins(Instruction::I64Const(char_len));
            self.ins(Instruction::I64Store(MemArg {
                offset: u64::from(STR_CHAR_LEN_OFFSET),
                align: 3,
                memory_index: 0,
            }));
        }

        if len > 0 {
            self.ins(Instruction::GlobalGet(global));
            if payload_offset > 0 {
                self.ins(Instruction::I32Const(payload_offset as i32));
                self.ins(Instruction::I32Add);
            }
            self.ins(Instruction::I32Const(0));
            self.ins(Instruction::I32Const(len as i32));
            self.ins(Instruction::MemoryInit {
                mem: 0,
                data_index: segment,
            });
        }
    }

    // -- literals ------------------------------------------------------------

    /// Interns a string literal, returning the global holding its address.
    ///
    /// The value is a `str`: `{ byte_len, char_len, bytes }`, exactly the
    /// layout the runtime and the native backend use.
    pub(crate) fn string_literal(&mut self, text: &str) -> u32 {
        if let Some(index) = self.strings.get(text) {
            return self.literals[*index].global;
        }
        let index = self.push_literal(text.as_bytes(), Some(text.chars().count() as i64));
        self.strings.insert(text.to_string(), index);
        self.literals[index].global
    }

    /// Interns a plain run of bytes, returning `(global, length)`.
    ///
    /// Used for panic messages and for the punctuation `print` writes around a
    /// container.
    pub(crate) fn raw_bytes(&mut self, text: &str) -> (u32, u32) {
        if let Some(index) = self.blobs.get(text) {
            let literal = &self.literals[*index];
            return (literal.global, literal.bytes.len() as u32);
        }
        let index = self.push_literal(text.as_bytes(), None);
        self.blobs.insert(text.to_string(), index);
        (self.literals[index].global, text.len() as u32)
    }

    fn push_literal(&mut self, bytes: &[u8], char_len: Option<i64>) -> usize {
        let index = self.literals.len();
        self.literals.push(Literal {
            global: index as u32,
            segment: index as u32,
            bytes: bytes.to_vec(),
            char_len,
        });
        index
    }

    // -- panics ---------------------------------------------------------------

    /// Emits an unconditional panic; nothing after it can run.
    pub(crate) fn panic(&mut self, message: &str) {
        let (global, len) = self.raw_bytes(message);
        self.ins(Instruction::GlobalGet(global));
        self.ins(Instruction::I32Const(len as i32));
        self.call_rt(Rt::Panic);
        self.ins(Instruction::Unreachable);
    }

    /// Emits `if cond: panic(message)`, consuming the `i32` condition on the
    /// stack.
    pub(crate) fn panic_if(&mut self, message: &str) {
        self.open_if(BlockType::Empty);
        self.panic(message);
        self.end();
    }

    // -- statements ------------------------------------------------------------

    fn block(&mut self, func: &'p hir::Function, block: &hir::Block) {
        for stmt in &block.stmts {
            self.stmt(func, stmt);
        }
    }

    fn stmt(&mut self, func: &'p hir::Function, stmt: &hir::Stmt) {
        match stmt {
            hir::Stmt::Assign { local, value } => {
                self.expr(func, value);
                self.store_local(*local);
            }
            hir::Stmt::Expr(expr) => {
                self.expr(func, expr);
                self.drop_value(expr.ty);
            }
            hir::Stmt::Return(None) => self.ins(Instruction::Return),
            hir::Stmt::Return(Some(value)) => {
                self.expr(func, value);
                self.ins(Instruction::Return);
            }
            hir::Stmt::If { cond, then, else_ } => self.emit_if(func, cond, then, else_.as_ref()),
            hir::Stmt::While { cond, body } => self.emit_while(func, cond, body),
            hir::Stmt::ForRange {
                var,
                start,
                stop,
                step,
                body,
            } => self.emit_for(func, *var, start, stop, step, body),
            hir::Stmt::ForEach {
                var,
                iter,
                over,
                body,
            } => self.emit_for_each(func, *var, iter, *over, body),
            hir::Stmt::SetIndex { list, index, value } => {
                self.emit_set_index(func, list, index, value)
            }
            hir::Stmt::SetField {
                obj,
                class,
                field,
                value,
            } => self.emit_set_field(func, obj, *class, *field, value),
            hir::Stmt::Group(block) => self.block(func, block),
            hir::Stmt::Break => {
                let target = self.loops.last().expect("`break` inside a loop").0;
                self.br(target);
            }
            hir::Stmt::Continue => {
                let target = self.loops.last().expect("`continue` inside a loop").1;
                self.br(target);
            }
        }
    }

    /// Pops a value off the stack into the wasm locals of a HIR local.
    pub(crate) fn store_local(&mut self, local: hir::LocalId) {
        let slots = self.slots[local.index()].clone();
        for slot in slots.iter().rev() {
            self.ins(Instruction::LocalSet(*slot));
        }
    }

    /// Pushes the value of a HIR local onto the stack.
    pub(crate) fn load_local(&mut self, local: hir::LocalId) {
        let slots = self.slots[local.index()].clone();
        self.push_locals(&slots);
    }

    fn emit_if(
        &mut self,
        func: &'p hir::Function,
        cond: &hir::Expr,
        then: &hir::Block,
        else_: Option<&hir::Block>,
    ) {
        self.expr(func, cond);
        self.open_if(BlockType::Empty);
        self.block(func, then);
        if let Some(else_) = else_ {
            self.else_();
            self.block(func, else_);
        }
        self.end();
    }

    fn emit_while(&mut self, func: &'p hir::Function, cond: &hir::Expr, body: &hir::Block) {
        let break_label = self.open_block(BlockType::Empty);
        let continue_label = self.open_loop(BlockType::Empty);

        self.expr(func, cond);
        self.ins(Instruction::I32Eqz);
        self.br_if(break_label);

        self.loops.push((break_label, continue_label));
        self.block(func, body);
        self.loops.pop();

        self.br(continue_label);
        self.end();
        self.end();
    }

    /// `for i in range(start, stop, step)` as a counting loop that allocates
    /// nothing (DESIGN §3.5).
    fn emit_for(
        &mut self,
        func: &'p hir::Function,
        var: hir::LocalId,
        start: &hir::Expr,
        stop: &hir::Expr,
        step: &hir::Expr,
        body: &hir::Block,
    ) {
        let iv = self.scratch(ValType::I64);
        self.expr(func, start);
        self.ins(Instruction::LocalSet(iv));

        let stop_local = self.scratch(ValType::I64);
        self.expr(func, stop);
        self.ins(Instruction::LocalSet(stop_local));

        let literal_step = match step.kind {
            hir::ExprKind::Int(v) => Some(v),
            _ => None,
        };
        let step_local = self.scratch(ValType::I64);
        self.expr(func, step);
        self.ins(Instruction::LocalSet(step_local));

        match literal_step {
            Some(0) => {
                self.panic("range() step must not be zero");
                return;
            }
            None => {
                self.ins(Instruction::LocalGet(step_local));
                self.ins(Instruction::I64Eqz);
                self.panic_if("range() step must not be zero");
            }
            Some(_) => {}
        }

        let break_label = self.open_block(BlockType::Empty);
        let head = self.open_loop(BlockType::Empty);

        // A literal step decides the direction of the test at emit time.
        match literal_step {
            Some(v) if v > 0 => {
                self.ins(Instruction::LocalGet(iv));
                self.ins(Instruction::LocalGet(stop_local));
                self.ins(Instruction::I64LtS);
            }
            Some(_) => {
                self.ins(Instruction::LocalGet(iv));
                self.ins(Instruction::LocalGet(stop_local));
                self.ins(Instruction::I64GtS);
            }
            None => {
                self.ins(Instruction::LocalGet(iv));
                self.ins(Instruction::LocalGet(stop_local));
                self.ins(Instruction::I64LtS);
                self.ins(Instruction::LocalGet(iv));
                self.ins(Instruction::LocalGet(stop_local));
                self.ins(Instruction::I64GtS);
                self.ins(Instruction::LocalGet(step_local));
                self.ins(Instruction::I64Const(0));
                self.ins(Instruction::I64GtS);
                self.ins(Instruction::Select);
            }
        }
        self.ins(Instruction::I32Eqz);
        self.br_if(break_label);

        self.ins(Instruction::LocalGet(iv));
        self.store_local(var);

        // `continue` runs the increment, so it targets a block wrapped around
        // the body rather than the loop itself.
        let continue_label = self.open_block(BlockType::Empty);
        self.loops.push((break_label, continue_label));
        self.block(func, body);
        self.loops.pop();
        self.end();

        self.ins(Instruction::LocalGet(iv));
        self.ins(Instruction::LocalGet(step_local));
        self.ins(Instruction::I64Add);
        self.ins(Instruction::LocalSet(iv));
        self.br(head);

        self.end();
        self.end();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn compile(source: &str) -> Vec<u8> {
        crate::compile_to_wasm("test.ty", source).expect("the program compiles")
    }

    #[test]
    fn hello_world_is_a_valid_module() {
        let module = compile("fn main():\n    print(\"hi\")\n");
        assert_eq!(&module[..4], b"\0asm");
        wasmparser::Validator::new()
            .validate_all(&module)
            .expect("valid");
    }

    #[test]
    fn the_entry_point_is_exported() {
        let module = compile("fn main():\n    print(1)\n");
        let mut exports = Vec::new();
        for payload in wasmparser::Parser::new(0).parse_all(&module) {
            if let wasmparser::Payload::ExportSection(section) = payload.unwrap() {
                for export in section {
                    exports.push(export.unwrap().name.to_string());
                }
            }
        }
        assert_eq!(exports, [START_EXPORT]);
    }

    /// `tests/run/runtime_surface.ty` exists to call every runtime entry point
    /// exactly once; if it stops doing so, part of the C ABI is no longer
    /// covered by the golden suite on either backend.
    #[test]
    fn the_runtime_surface_program_imports_every_runtime_function() {
        let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .parent()
            .and_then(std::path::Path::parent)
            .expect("crates/codegen-wasm always has two ancestors")
            .join("tests/run/runtime_surface.ty");
        let source = std::fs::read_to_string(&path)
            .unwrap_or_else(|err| panic!("cannot read {}: {err}", path.display()));
        let module = compile(&source);

        let mut imported = Vec::new();
        for payload in wasmparser::Parser::new(0).parse_all(&module) {
            if let wasmparser::Payload::ImportSection(section) = payload.unwrap() {
                for import in section.into_imports() {
                    imported.push(import.unwrap().name.to_string());
                }
            }
        }
        let missing: Vec<&str> = Rt::ALL
            .iter()
            .map(|rt| rt.name())
            .filter(|name| !imported.iter().any(|import| import == name))
            .collect();
        assert!(
            missing.is_empty(),
            "tests/run/runtime_surface.ty no longer calls {missing:?}"
        );
    }

    #[test]
    fn only_used_runtime_functions_are_imported() {
        let module = compile("fn main():\n    print(1)\n");
        let mut imports = Vec::new();
        for payload in wasmparser::Parser::new(0).parse_all(&module) {
            if let wasmparser::Payload::ImportSection(section) = payload.unwrap() {
                for import in section.into_imports() {
                    let import = import.unwrap();
                    imports.push(format!("{}.{}", import.module, import.name));
                }
            }
        }
        assert!(imports.contains(&"env.memory".to_string()), "{imports:?}");
        assert!(
            imports.contains(&"env.ty_print_int".to_string()),
            "{imports:?}"
        );
        assert!(
            !imports.contains(&"env.ty_str_split".to_string()),
            "{imports:?}"
        );
    }
}
