//! The textual LLVM IR emitter.
//!
//! One [`hir::Function`] becomes one `define`, one local becomes one `alloca`
//! in the entry block plus loads and stores everywhere else. `clang -O2` runs
//! `mem2reg`, which turns those allocas back into SSA registers, so the emitter
//! never has to build phi nodes itself (DESIGN §6.2).

use std::collections::{BTreeSet, HashMap};
use std::fmt::Write;

use typhoon_sema::hir;
use typhoon_sema::types::{ClassId, Type, TypeId, TypeTable};

/// The smallest `int`, whose negation and division by `-1` both wrap.
const I64_MIN: &str = "-9223372036854775808";

/// Emits the whole program as one LLVM module.
pub fn emit_module(program: &hir::Program) -> String {
    let mut emitter = Emitter::new(program);
    emitter.emit_program();
    emitter.finish()
}

/// The LLVM struct type holding a `list<T>` (DESIGN §4.1). The payload lives
/// in a separate allocation so that growth never moves the header.
const LIST_TYPE: &str = "%TyList = type { i64, i64, ptr }";

/// Byte offset of a `str`'s code-point count; the byte length is at 0 and the
/// bytes themselves start at [`STR_HEADER`].
const STR_CHAR_LEN_OFFSET: u32 = 8;

/// Size of the `str` header, `{ byte_len: i64, char_len: i64 }`.
const STR_HEADER: u32 = 16;

/// The type-based alias analysis (TBAA) tags the emitter attaches to memory
/// accesses.
///
/// Every one of these lives in a separate GC allocation — a list header, its
/// element buffer, a class instance and a string are four different blocks —
/// so an access through one can never touch another. Saying so is what lets
/// LLVM hoist a list's `len` and `data` out of a loop that stores into its
/// elements; without it, every `xs[i] = v` looks like it might overwrite the
/// header and the loop re-loads both every iteration.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum Tbaa {
    /// The `{ len, cap, data }` header of a `list<T>`.
    ListHeader,
    /// A `list<T>`'s element buffer.
    ListData,
    /// A field of a class instance.
    ClassField,
    /// The `{ byte_len, char_len }` header of a `str`, which is immutable and
    /// therefore also `!invariant.load`.
    StrHeader,
}

impl Tbaa {
    /// The metadata suffix to append to a `load` or `store`.
    fn suffix(self) -> &'static str {
        match self {
            Tbaa::ListHeader => ", !tbaa !5",
            Tbaa::ListData => ", !tbaa !6",
            Tbaa::ClassField => ", !tbaa !7",
            Tbaa::StrHeader => ", !tbaa !8, !invariant.load !9",
        }
    }
}

/// The TBAA type tree the [`Tbaa`] tags refer to.
const TBAA_METADATA: &str = "\n; -- type-based alias analysis -----------------------------------\n\
     !0 = !{!\"typhoon\"}\n\
     !1 = !{!\"list.header\", !0}\n\
     !2 = !{!\"list.data\", !0}\n\
     !3 = !{!\"class.field\", !0}\n\
     !4 = !{!\"str.header\", !0}\n\
     !5 = !{!1, !1, i64 0}\n\
     !6 = !{!2, !2, i64 0}\n\
     !7 = !{!3, !3, i64 0}\n\
     !8 = !{!4, !4, i64 0}\n\
     !9 = !{}\n";

/// Emitter state: module-level tables plus the function being emitted.
struct Emitter<'p> {
    program: &'p hir::Program,
    /// The program's type table, for layouts and LLVM type names.
    types: &'p TypeTable,
    /// `%C.<name> = type { … }` for every class, emitted before the functions.
    type_defs: String,
    /// Global definitions (string literals, panic messages).
    globals: String,
    /// The runtime and intrinsic declarations actually used.
    decls: BTreeSet<&'static str>,
    /// String literal contents to the global holding them.
    strings: HashMap<String, String>,
    /// Panic messages to `(global, length)`.
    panics: HashMap<String, (String, usize)>,
    next_global: u32,
    /// Every emitted function.
    functions: String,
    /// Whether any access carried a TBAA tag, so the metadata is only emitted
    /// when it is referenced.
    uses_tbaa: bool,
    /// Whether any instruction referred to `%TyList`, so its definition is only
    /// emitted when it is used. A `list<T>` can appear without any list-typed
    /// local — `print("a-b".split("-"))` has none — so this cannot be derived
    /// from the local types.
    uses_list_type: bool,

    // -- per function -----------------------------------------------------
    allocas: String,
    body: String,
    tmp: u32,
    lbl: u32,
    slots: Vec<String>,
    terminated: bool,
    /// `(break target, continue target)` for every enclosing loop.
    loops: Vec<(String, String)>,
}

impl<'p> Emitter<'p> {
    fn new(program: &'p hir::Program) -> Emitter<'p> {
        Emitter {
            program,
            types: &program.types,
            type_defs: String::new(),
            globals: String::new(),
            decls: BTreeSet::new(),
            strings: HashMap::new(),
            panics: HashMap::new(),
            next_global: 0,
            functions: String::new(),
            uses_tbaa: false,
            uses_list_type: false,
            allocas: String::new(),
            body: String::new(),
            tmp: 0,
            lbl: 0,
            slots: Vec::new(),
            terminated: false,
            loops: Vec::new(),
        }
    }

    fn finish(self) -> String {
        let mut out = String::new();
        out.push_str("; Generated by the typhoon compiler (DESIGN.md section 6.2).\n");
        out.push_str("; LLVM 18+ textual IR, opaque pointers.\n\n");
        if self.uses_list_type {
            out.push_str(LIST_TYPE);
            out.push('\n');
        }
        if !self.type_defs.is_empty() {
            out.push_str(&self.type_defs);
            out.push('\n');
        }
        if self.uses_list_type && self.type_defs.is_empty() {
            out.push('\n');
        }
        if !self.globals.is_empty() {
            out.push_str(&self.globals);
            out.push('\n');
        }
        out.push_str(&self.functions);
        if !self.decls.is_empty() {
            out.push_str("\n; -- runtime and intrinsics ---------------------------------------\n");
            for decl in &self.decls {
                out.push_str(decl);
                out.push('\n');
            }
        }
        if self.uses_tbaa {
            out.push_str(TBAA_METADATA);
        }
        out
    }

    // -- module -----------------------------------------------------------

    fn emit_program(&mut self) {
        self.emit_type_defs();
        for (index, func) in self.program.functions.iter().enumerate() {
            self.emit_function(hir::FuncId(index as u32), func);
        }
        self.emit_entry_point();
    }

    /// One named LLVM struct per class.
    ///
    /// Class fields are all scalars or pointers, so no class type is ever
    /// recursive even when the class is (`Node | None` is just a `ptr`).
    fn emit_type_defs(&mut self) {
        for (index, class) in self.types.classes().iter().enumerate() {
            let fields: Vec<String> = class.fields.iter().map(|f| self.store_ty(f.ty)).collect();
            let _ = writeln!(
                self.type_defs,
                "{} = type {{ {} }}",
                class_type_name(ClassId(index as u32)),
                fields.join(", ")
            );
        }
    }

    /// The name of the list header type, recording that its definition is
    /// needed.
    pub(super) fn list_type(&mut self) -> &'static str {
        self.uses_list_type = true;
        "%TyList"
    }

    // -- types and layout --------------------------------------------------

    /// The LLVM type a value of `ty` is held in, in a register or an `alloca`.
    ///
    /// `bool` is `i1` here and `i8` in memory (DESIGN §4.2), which is what
    /// [`Emitter::store_ty`] returns.
    pub(super) fn llvm_ty(&self, ty: TypeId) -> String {
        match self.types.get(ty) {
            Type::Int => "i64".to_string(),
            Type::Float => "double".to_string(),
            Type::Bool => "i1".to_string(),
            Type::Str | Type::List(_) | Type::Class(_) | Type::Optional(_) => "ptr".to_string(),
            Type::Tuple(id) => {
                let members: Vec<String> = self
                    .types
                    .tuple_members(id)
                    .iter()
                    .map(|m| self.store_ty(*m))
                    .collect();
                format!("{{ {} }}", members.join(", "))
            }
            Type::Unit | Type::Error => "void".to_string(),
        }
    }

    /// The LLVM type a value of `ty` occupies inside a class field, a list
    /// element or a tuple member: like [`Emitter::llvm_ty`] except that `bool`
    /// becomes `i8` (DESIGN §4.2).
    pub(super) fn store_ty(&self, ty: TypeId) -> String {
        match self.types.get(ty) {
            Type::Bool => "i8".to_string(),
            _ => self.llvm_ty(ty),
        }
    }

    /// The LLVM return type for a typhoon return type.
    fn llvm_ret_ty(&self, ty: TypeId) -> String {
        if ty == TypeId::UNIT {
            "void".to_string()
        } else {
            self.llvm_ty(ty)
        }
    }

    /// The alignment of a value of `ty` in a register or an `alloca`.
    pub(super) fn align_of(&self, ty: TypeId) -> u32 {
        match self.types.get(ty) {
            Type::Bool => 1,
            Type::Tuple(id) => self
                .types
                .tuple_members(id)
                .iter()
                .map(|m| self.store_align_of(*m))
                .max()
                .unwrap_or(1),
            _ => 8,
        }
    }

    /// The alignment of a value of `ty` in memory (`bool` is one byte).
    pub(super) fn store_align_of(&self, ty: TypeId) -> u32 {
        match self.types.get(ty) {
            Type::Bool => 1,
            _ => self.align_of(ty),
        }
    }

    /// The number of bytes a value of `ty` occupies in memory.
    ///
    /// The layout is the usual C one — every member at the next multiple of
    /// its own alignment, the whole rounded up — which is exactly what LLVM
    /// computes for the struct types this emitter writes.
    pub(super) fn size_of(&self, ty: TypeId) -> u64 {
        match self.types.get(ty) {
            Type::Bool => 1,
            Type::Tuple(id) => {
                let members = self.types.tuple_members(id).to_vec();
                self.struct_size(&members)
            }
            Type::Unit | Type::Error => 0,
            _ => 8,
        }
    }

    /// The size of a struct holding `members` back to back.
    pub(super) fn struct_size(&self, members: &[TypeId]) -> u64 {
        let mut size = 0u64;
        let mut align = 1u64;
        for member in members {
            let member_align = u64::from(self.store_align_of(*member));
            align = align.max(member_align);
            size = size.div_ceil(member_align) * member_align;
            size += self.size_of(*member);
        }
        size.div_ceil(align) * align
    }

    /// The metadata suffix for an access to `tag`, recording that the module
    /// needs the TBAA type tree.
    pub(super) fn tbaa(&mut self, tag: Tbaa) -> &'static str {
        self.uses_tbaa = true;
        tag.suffix()
    }

    /// The `i8` flag `ty_alloc*` and the list runtime take: 1 when a value of
    /// `ty` holds no pointer and may live in an unscanned block (DESIGN §5.1).
    pub(super) fn atomic_flag(&self, ty: TypeId) -> u8 {
        u8::from(self.types.is_pointer_free(ty))
    }

    /// The C `main`: start the runtime, run the user's `main`, flush and exit.
    fn emit_entry_point(&mut self) {
        let user_main = &self.program.function(self.program.main).symbol;
        self.decls.insert("declare void @ty_rt_init()");
        self.decls
            .insert("declare void @ty_rt_exit(i32) noreturn nounwind");
        let _ = write!(
            self.functions,
            "\n; the process entry point\n\
             define i32 @main() nounwind {{\n\
             entry:\n\
             \x20 call void @ty_rt_init()\n\
             \x20 call void @{user_main}()\n\
             \x20 call void @ty_rt_exit(i32 0)\n\
             \x20 unreachable\n\
             }}\n"
        );
    }

    fn emit_function(&mut self, id: hir::FuncId, func: &'p hir::Function) {
        self.allocas.clear();
        self.body.clear();
        self.tmp = 0;
        self.lbl = 0;
        self.terminated = false;
        self.loops.clear();
        self.slots = func
            .locals
            .iter()
            .map(|local| format!("%{}.addr", local.name))
            .collect();

        // Entry block: one alloca per local, then the incoming parameters.
        for (index, local) in func.locals.iter().enumerate() {
            let ty = self.llvm_ty(local.ty);
            let _ = writeln!(
                self.allocas,
                "  {} = alloca {ty}, align {}",
                self.slots[index],
                self.align_of(local.ty)
            );
        }
        for param in &func.params {
            let local = &func.locals[param.index()];
            let ty = self.llvm_ty(local.ty);
            let _ = writeln!(
                self.allocas,
                "  store {ty} %{}.arg, ptr {}, align {}",
                local.name,
                self.slots[param.index()],
                self.align_of(local.ty)
            );
        }

        self.block(func, &func.body);

        // Fall off the end: `main` and other unit functions return, a function
        // with a return type cannot get here (sema proved it returns).
        if !self.terminated {
            if func.ret == TypeId::UNIT {
                self.emit("ret void");
            } else {
                self.emit("unreachable");
            }
        }

        let params: Vec<String> = func
            .params
            .iter()
            .map(|id| {
                let local = &func.locals[id.index()];
                format!("{} %{}.arg", self.llvm_ty(local.ty), local.name)
            })
            .collect();
        let signature: Vec<String> = func
            .params
            .iter()
            .map(|id| {
                let local = &func.locals[id.index()];
                format!("{}: {}", local.name, self.program.types.name(local.ty))
            })
            .collect();
        let _ = write!(
            self.functions,
            "\n; fn {}({}) -> {}\ndefine internal {} @{}({}) nounwind {{\nentry:\n{}{}}}\n",
            func.name,
            signature.join(", "),
            self.program.types.name(func.ret),
            self.llvm_ret_ty(func.ret),
            func.symbol,
            params.join(", "),
            self.allocas,
            self.body,
        );
        let _ = id;
    }

    // -- emitting primitives ----------------------------------------------

    /// Appends one instruction, opening a fresh block first when the current
    /// one already ended.
    fn emit(&mut self, text: &str) {
        if self.terminated {
            let label = self.fresh_label("dead");
            let _ = write!(self.body, "\n{label}:\n");
            self.terminated = false;
        }
        let _ = writeln!(self.body, "  {text}");
    }

    fn comment(&mut self, text: &str) {
        let _ = writeln!(self.body, "  ; {text}");
    }

    fn start_block(&mut self, label: &str) {
        let _ = write!(self.body, "\n{label}:\n");
        self.terminated = false;
    }

    fn br(&mut self, label: &str) {
        self.emit(&format!("br label %{label}"));
        self.terminated = true;
    }

    fn cond_br(&mut self, cond: &str, then: &str, else_: &str) {
        self.emit(&format!("br i1 {cond}, label %{then}, label %{else_}"));
        self.terminated = true;
    }

    fn fresh(&mut self) -> String {
        self.tmp += 1;
        format!("%t{}", self.tmp)
    }

    fn fresh_label(&mut self, base: &str) -> String {
        self.lbl += 1;
        format!("{base}.{}", self.lbl)
    }

    /// An `alloca` in the entry block, for the result of a short-circuit
    /// operator.
    fn scratch(&mut self, ty: &str, align: u32) -> String {
        self.lbl += 1;
        let name = format!("%scratch.{}", self.lbl);
        let _ = writeln!(self.allocas, "  {name} = alloca {ty}, align {align}");
        name
    }

    // -- statements -------------------------------------------------------

    fn block(&mut self, func: &'p hir::Function, block: &hir::Block) {
        for stmt in &block.stmts {
            self.stmt(func, stmt);
        }
    }

    fn stmt(&mut self, func: &'p hir::Function, stmt: &hir::Stmt) {
        match stmt {
            hir::Stmt::Assign { local, value } => {
                let ty = func.locals[local.index()].ty;
                let value = self.expr(func, value);
                let slot = self.slots[local.index()].clone();
                self.emit(&format!(
                    "store {} {value}, ptr {slot}, align {}",
                    self.llvm_ty(ty),
                    self.align_of(ty)
                ));
            }
            hir::Stmt::Expr(expr) => {
                self.expr(func, expr);
            }
            hir::Stmt::Return(None) => {
                self.emit("ret void");
                self.terminated = true;
            }
            hir::Stmt::Return(Some(value)) => {
                let ty = self.llvm_ty(value.ty);
                let value = self.expr(func, value);
                self.emit(&format!("ret {ty} {value}"));
                self.terminated = true;
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
                let target = self.loops.last().expect("`break` inside a loop").0.clone();
                self.br(&target);
            }
            hir::Stmt::Continue => {
                let target = self
                    .loops
                    .last()
                    .expect("`continue` inside a loop")
                    .1
                    .clone();
                self.br(&target);
            }
        }
    }

    fn emit_if(
        &mut self,
        func: &'p hir::Function,
        cond: &hir::Expr,
        then: &hir::Block,
        else_: Option<&hir::Block>,
    ) {
        let cond = self.expr(func, cond);
        let then_label = self.fresh_label("if.then");
        let else_label = self.fresh_label("if.else");
        let end_label = self.fresh_label("if.end");

        self.cond_br(
            &cond,
            &then_label,
            if else_.is_some() {
                &else_label
            } else {
                &end_label
            },
        );

        self.start_block(&then_label);
        self.block(func, then);
        if !self.terminated {
            self.br(&end_label);
        }

        if let Some(else_) = else_ {
            self.start_block(&else_label);
            self.block(func, else_);
            if !self.terminated {
                self.br(&end_label);
            }
        }

        self.start_block(&end_label);
    }

    fn emit_while(&mut self, func: &'p hir::Function, cond: &hir::Expr, body: &hir::Block) {
        let head = self.fresh_label("while.cond");
        let body_label = self.fresh_label("while.body");
        let end = self.fresh_label("while.end");

        self.br(&head);
        self.start_block(&head);
        let cond = self.expr(func, cond);
        self.cond_br(&cond, &body_label, &end);

        self.start_block(&body_label);
        self.loops.push((end.clone(), head.clone()));
        self.block(func, body);
        self.loops.pop();
        if !self.terminated {
            self.br(&head);
        }

        self.start_block(&end);
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
        let start_value = self.expr(func, start);
        let stop_value = self.expr(func, stop);
        let literal_step = match step.kind {
            hir::ExprKind::Int(v) => Some(v),
            _ => None,
        };
        let step_value = self.expr(func, step);

        match literal_step {
            Some(0) => {
                self.panic("range() step must not be zero");
                return;
            }
            None => {
                let zero = self.fresh();
                self.emit(&format!("{zero} = icmp eq i64 {step_value}, 0"));
                self.panic_if(&zero, "range() step must not be zero");
            }
            Some(_) => {}
        }

        let iv = {
            self.lbl += 1;
            let name = format!("%for.iv.{}", self.lbl);
            let _ = writeln!(self.allocas, "  {name} = alloca i64, align 8");
            name
        };
        self.emit(&format!("store i64 {start_value}, ptr {iv}, align 8"));

        let head = self.fresh_label("for.cond");
        let body_label = self.fresh_label("for.body");
        let next = self.fresh_label("for.step");
        let end = self.fresh_label("for.end");

        self.br(&head);
        self.start_block(&head);
        let i = self.fresh();
        self.emit(&format!("{i} = load i64, ptr {iv}, align 8"));
        // A literal step decides the direction of the test at compile time.
        let cond = match literal_step {
            Some(v) if v > 0 => {
                let c = self.fresh();
                self.emit(&format!("{c} = icmp slt i64 {i}, {stop_value}"));
                c
            }
            Some(_) => {
                let c = self.fresh();
                self.emit(&format!("{c} = icmp sgt i64 {i}, {stop_value}"));
                c
            }
            None => {
                let up = self.fresh();
                let lt = self.fresh();
                let gt = self.fresh();
                let c = self.fresh();
                self.emit(&format!("{up} = icmp sgt i64 {step_value}, 0"));
                self.emit(&format!("{lt} = icmp slt i64 {i}, {stop_value}"));
                self.emit(&format!("{gt} = icmp sgt i64 {i}, {stop_value}"));
                self.emit(&format!("{c} = select i1 {up}, i1 {lt}, i1 {gt}"));
                c
            }
        };
        self.cond_br(&cond, &body_label, &end);

        self.start_block(&body_label);
        let slot = self.slots[var.index()].clone();
        self.emit(&format!("store i64 {i}, ptr {slot}, align 8"));
        self.loops.push((end.clone(), next.clone()));
        self.block(func, body);
        self.loops.pop();
        if !self.terminated {
            self.br(&next);
        }

        self.start_block(&next);
        let cur = self.fresh();
        let inc = self.fresh();
        self.emit(&format!("{cur} = load i64, ptr {iv}, align 8"));
        self.emit(&format!("{inc} = add i64 {cur}, {step_value}"));
        self.emit(&format!("store i64 {inc}, ptr {iv}, align 8"));
        self.br(&head);

        self.start_block(&end);
    }

    // -- panics -----------------------------------------------------------

    /// Emits an unconditional panic; the current block ends here.
    fn panic(&mut self, message: &str) {
        let (global, len) = self.panic_message(message);
        self.decls
            .insert("declare void @ty_panic(ptr, i64) noreturn nounwind");
        self.emit(&format!("call void @ty_panic(ptr {global}, i64 {len})"));
        self.emit("unreachable");
        self.terminated = true;
    }

    /// Emits `if cond: panic(message)`.
    fn panic_if(&mut self, cond: &str, message: &str) {
        let fail = self.fresh_label("panic");
        let ok = self.fresh_label("cont");
        self.cond_br(cond, &fail, &ok);
        self.start_block(&fail);
        self.panic(message);
        self.start_block(&ok);
    }

    fn panic_message(&mut self, message: &str) -> (String, usize) {
        self.raw_bytes(message)
    }

    /// Interns a plain byte array, returning `(global, length)`.
    ///
    /// Used for panic messages and for the punctuation `print` writes around a
    /// container (`[`, `, `, `]`).
    pub(super) fn raw_bytes(&mut self, text: &str) -> (String, usize) {
        if let Some(entry) = self.panics.get(text) {
            return entry.clone();
        }
        let name = format!("@bytes.{}", self.next_global);
        self.next_global += 1;
        let bytes = text.as_bytes();
        let _ = writeln!(
            self.globals,
            "{name} = private unnamed_addr constant [{} x i8] c\"{}\", align 1",
            bytes.len(),
            escape(bytes)
        );
        let entry = (name, bytes.len());
        self.panics.insert(text.to_string(), entry.clone());
        entry
    }

    /// Interns a string literal as a `str` header followed by its bytes.
    ///
    /// The header is `{ byte_len, char_len }` (DESIGN §4.3: `len(s)` counts
    /// code points and is O(1)), so a literal costs no allocation and no scan.
    pub(super) fn string_literal(&mut self, text: &str) -> String {
        if let Some(name) = self.strings.get(text) {
            return name.clone();
        }
        let name = format!("@str.{}", self.next_global);
        self.next_global += 1;
        let bytes = text.as_bytes();
        let chars = text.chars().count();
        let _ = writeln!(
            self.globals,
            "{name} = private unnamed_addr constant {{ i64, i64, [{n} x i8] }} \
             {{ i64 {n}, i64 {chars}, [{n} x i8] c\"{}\" }}, align 8",
            escape(bytes),
            n = bytes.len()
        );
        self.strings.insert(text.to_string(), name.clone());
        name
    }
}

/// The name of the LLVM struct type holding instances of a class.
pub(crate) fn class_type_name(class: ClassId) -> String {
    format!("%C.{}", class.index())
}

/// Escapes bytes for an LLVM `c"..."` constant.
fn escape(bytes: &[u8]) -> String {
    let mut out = String::with_capacity(bytes.len());
    for &b in bytes {
        if b == b'"' || b == b'\\' || !(0x20..0x7f).contains(&b) {
            let _ = write!(out, "\\{b:02X}");
        } else {
            out.push(b as char);
        }
    }
    out
}

/// Formats a `f64` so that LLVM parses it back bit for bit.
///
/// Rust's shortest round-trip form is used when it is finite (LLVM parses
/// decimals with correct rounding), except that LLVM needs a `.` or a hex
/// form: `1e300` alone would be read as an integer.
pub(crate) fn float_literal(value: f64) -> String {
    if !value.is_finite() {
        return format!("0x{:016X}", value.to_bits());
    }
    let text = format!("{value:?}");
    if text.contains('.') {
        return text;
    }
    match text.split_once('e') {
        Some((mantissa, exponent)) => format!("{mantissa}.0e{exponent}"),
        None => format!("{text}.0"),
    }
}

mod data;
mod exprs;
