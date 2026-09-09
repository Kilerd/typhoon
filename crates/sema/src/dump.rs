//! A compact textual dump of the typed IR, used by tests to pin down what the
//! checker produced.

use std::fmt::Write;

use crate::hir::{
    Block, Expr, ExprKind, FStringPart, FloatOp, FormatSpec, Function, IntOp, MinMax, Program, Stmt,
};

/// Renders a whole program as indented text.
///
/// ```
/// use typhoon_diag::{Diagnostics, SourceMap};
///
/// let src = "fn main():\n    x = 1\n";
/// let mut sources = SourceMap::new();
/// let file = sources.add("main.ty", src);
/// let mut diags = Diagnostics::new();
/// let module = typhoon_parser::parse_module(file, src, &mut diags);
/// let program = typhoon_sema::check_module(&module, file, &mut diags).unwrap();
/// assert!(typhoon_sema::dump_program(&program).contains("x: int"));
/// ```
pub fn dump_program(program: &Program) -> String {
    let mut out = String::new();
    for func in &program.functions {
        dump_function(&mut out, program, func);
    }
    out
}

fn dump_function(out: &mut String, program: &Program, func: &Function) {
    let params: Vec<String> = func
        .params
        .iter()
        .map(|id| {
            let local = &func.locals[id.index()];
            format!("{}: {}", local.name, program.types.name(local.ty))
        })
        .collect();
    let _ = writeln!(
        out,
        "fn {}({}) -> {}",
        func.name,
        params.join(", "),
        program.types.name(func.ret)
    );
    for local in func.locals.iter().filter(|l| !l.is_param) {
        let _ = writeln!(
            out,
            "  local {}: {}",
            local.name,
            program.types.name(local.ty)
        );
    }
    dump_block(out, program, func, &func.body, 1);
}

fn dump_block(out: &mut String, program: &Program, func: &Function, block: &Block, depth: usize) {
    for stmt in &block.stmts {
        dump_stmt(out, program, func, stmt, depth);
    }
}

fn dump_stmt(out: &mut String, program: &Program, func: &Function, stmt: &Stmt, depth: usize) {
    let pad = "  ".repeat(depth);
    match stmt {
        Stmt::Assign { local, value } => {
            let _ = writeln!(
                out,
                "{pad}{} = {}",
                func.locals[local.index()].name,
                dump_expr(program, func, value)
            );
        }
        Stmt::Expr(expr) => {
            let _ = writeln!(out, "{pad}{}", dump_expr(program, func, expr));
        }
        Stmt::If { cond, then, else_ } => {
            let _ = writeln!(out, "{pad}if {}:", dump_expr(program, func, cond));
            dump_block(out, program, func, then, depth + 1);
            if let Some(else_) = else_ {
                let _ = writeln!(out, "{pad}else:");
                dump_block(out, program, func, else_, depth + 1);
            }
        }
        Stmt::While { cond, body } => {
            let _ = writeln!(out, "{pad}while {}:", dump_expr(program, func, cond));
            dump_block(out, program, func, body, depth + 1);
        }
        Stmt::ForRange {
            var,
            start,
            stop,
            step,
            body,
        } => {
            let _ = writeln!(
                out,
                "{pad}for {} in range({}, {}, {}):",
                func.locals[var.index()].name,
                dump_expr(program, func, start),
                dump_expr(program, func, stop),
                dump_expr(program, func, step)
            );
            dump_block(out, program, func, body, depth + 1);
        }
        Stmt::Return(None) => {
            let _ = writeln!(out, "{pad}return");
        }
        Stmt::Return(Some(value)) => {
            let _ = writeln!(out, "{pad}return {}", dump_expr(program, func, value));
        }
        Stmt::Break => {
            let _ = writeln!(out, "{pad}break");
        }
        Stmt::Continue => {
            let _ = writeln!(out, "{pad}continue");
        }
    }
}

/// Renders one expression, annotated with its type.
fn dump_expr(program: &Program, func: &Function, expr: &Expr) -> String {
    let e = |inner: &Expr| dump_expr(program, func, inner);
    let body = match &expr.kind {
        ExprKind::Int(v) => v.to_string(),
        ExprKind::Float(v) => format!("{v:?}"),
        ExprKind::Bool(v) => if *v { "True" } else { "False" }.to_string(),
        ExprKind::Str(v) => format!("{v:?}"),
        ExprKind::Unit => "None".to_string(),
        ExprKind::Local(id) => func.locals[id.index()].name.clone(),
        ExprKind::IntNeg(v) => format!("-{}", e(v)),
        ExprKind::IntNot(v) => format!("~{}", e(v)),
        ExprKind::FloatNeg(v) => format!("-{}", e(v)),
        ExprKind::Not(v) => format!("not {}", e(v)),
        ExprKind::IntBin { op, lhs, rhs } => {
            format!("({} {} {})", e(lhs), int_op_str(*op), e(rhs))
        }
        ExprKind::IntSquare(v) => format!("square({})", e(v)),
        ExprKind::FloatBin { op, lhs, rhs } => {
            format!("({} {} {})", e(lhs), float_op_str(*op), e(rhs))
        }
        ExprKind::FloatSqrt(v) => format!("sqrt({})", e(v)),
        ExprKind::FloatSquare(v) => format!("square({})", e(v)),
        ExprKind::IntDiv { lhs, rhs } => format!("({} / {})", e(lhs), e(rhs)),
        ExprKind::IntCmp { op, lhs, rhs }
        | ExprKind::FloatCmp { op, lhs, rhs }
        | ExprKind::BoolCmp { op, lhs, rhs }
        | ExprKind::StrCmp { op, lhs, rhs } => {
            format!("({} {} {})", e(lhs), cmp_op_str(*op), e(rhs))
        }
        ExprKind::StrConcat { lhs, rhs } => format!("({} + {})", e(lhs), e(rhs)),
        ExprKind::And { lhs, rhs } => format!("({} and {})", e(lhs), e(rhs)),
        ExprKind::Or { lhs, rhs } => format!("({} or {})", e(lhs), e(rhs)),
        ExprKind::Call { func: id, args, .. } => {
            let args: Vec<String> = args.iter().map(&e).collect();
            format!("{}({})", program.function(*id).name, args.join(", "))
        }
        ExprKind::IntToFloat(v) => format!("float({})", e(v)),
        ExprKind::FloatToInt(v) => format!("int({})", e(v)),
        ExprKind::FloatFence(v) => format!("fence({})", e(v)),
        ExprKind::IntAbs(v) | ExprKind::FloatAbs(v) => format!("abs({})", e(v)),
        ExprKind::IntMinMax { op, lhs, rhs } | ExprKind::FloatMinMax { op, lhs, rhs } => {
            let name = match op {
                MinMax::Min => "min",
                MinMax::Max => "max",
            };
            format!("{name}({}, {})", e(lhs), e(rhs))
        }
        ExprKind::Print(args) => {
            let args: Vec<String> = args.iter().map(&e).collect();
            format!("print({})", args.join(", "))
        }
        ExprKind::FString(parts) => {
            let parts: Vec<String> = parts
                .iter()
                .map(|part| match part {
                    FStringPart::Literal(text) => format!("{text:?}"),
                    FStringPart::Value { expr, spec } => match spec {
                        FormatSpec::Display => format!("{{{}}}", e(expr)),
                        FormatSpec::Fixed(n) => format!("{{{}:.{n}f}}", e(expr)),
                    },
                })
                .collect();
            format!("f({})", parts.join(", "))
        }
    };
    format!("{body}:{}", program.types.name(expr.ty))
}

fn int_op_str(op: IntOp) -> &'static str {
    match op {
        IntOp::Add => "+",
        IntOp::Sub => "-",
        IntOp::Mul => "*",
        IntOp::FloorDiv => "//",
        IntOp::Mod => "%",
        IntOp::Pow => "**",
        IntOp::BitAnd => "&",
        IntOp::BitOr => "|",
        IntOp::BitXor => "^",
        IntOp::Shl => "<<",
        IntOp::Shr => ">>",
    }
}

fn float_op_str(op: FloatOp) -> &'static str {
    match op {
        FloatOp::Add => "+",
        FloatOp::Sub => "-",
        FloatOp::Mul => "*",
        FloatOp::Div => "/",
        FloatOp::FloorDiv => "//",
        FloatOp::Mod => "%",
        FloatOp::Pow => "**",
    }
}

fn cmp_op_str(op: crate::hir::CmpOp) -> &'static str {
    match op {
        crate::hir::CmpOp::Eq => "==",
        crate::hir::CmpOp::Ne => "!=",
        crate::hir::CmpOp::Lt => "<",
        crate::hir::CmpOp::Le => "<=",
        crate::hir::CmpOp::Gt => ">",
        crate::hir::CmpOp::Ge => ">=",
    }
}
