//! Shared helpers for the parser tests.
#![allow(dead_code)]

use typhoon_ast::{Expr, ExprKind, FStringPart, Module, TypeExpr, TypeExprKind};
use typhoon_diag::{Diagnostics, FileId, SourceMap, render_all};

pub const FILE: FileId = FileId(0);

/// Parses a module and asserts that no diagnostic was produced.
pub fn module(src: &str) -> Module {
    let mut diags = Diagnostics::new();
    let module = typhoon_parser::parse_module(FILE, src, &mut diags);
    assert!(
        !diags.has_errors(),
        "unexpected diagnostics:\n{}",
        rendered(src, &diags)
    );
    module
}

/// Parses a module and returns it together with its diagnostics.
pub fn module_with_diags(src: &str) -> (Module, Diagnostics) {
    let mut diags = Diagnostics::new();
    let module = typhoon_parser::parse_module(FILE, src, &mut diags);
    (module, diags)
}

/// The diagnostic messages (headlines only) of parsing `src`.
pub fn messages(src: &str) -> Vec<String> {
    let (_, diags) = module_with_diags(src);
    diags.iter().map(|d| d.message.clone()).collect()
}

/// The rendered (plain) diagnostics of parsing `src`, for snapshot tests.
pub fn rendered_errors(src: &str) -> String {
    let (_, diags) = module_with_diags(src);
    rendered(src, &diags)
}

pub fn rendered(src: &str, diags: &Diagnostics) -> String {
    let mut sources = SourceMap::new();
    let file = sources.add("test.ty", src);
    assert_eq!(file, FILE);
    render_all(diags.iter(), &sources, false)
}

/// Parses an expression, asserting there are no diagnostics, and renders it in
/// a compact s-expression form.
pub fn expr(src: &str) -> String {
    let mut diags = Diagnostics::new();
    let expr = typhoon_parser::parse_expr(FILE, src, &mut diags);
    assert!(
        !diags.has_errors(),
        "unexpected diagnostics for {src:?}:\n{}",
        rendered(src, &diags)
    );
    sexpr(&expr)
}

/// Parses an expression and returns `(s-expression, messages)`.
pub fn expr_with_diags(src: &str) -> (String, Vec<String>) {
    let mut diags = Diagnostics::new();
    let expr = typhoon_parser::parse_expr(FILE, src, &mut diags);
    (
        sexpr(&expr),
        diags.iter().map(|d| d.message.clone()).collect(),
    )
}

/// The full AST dump of an expression (spans included).
pub fn expr_dump(src: &str) -> String {
    let mut diags = Diagnostics::new();
    let expr = typhoon_parser::parse_expr(FILE, src, &mut diags);
    assert!(
        !diags.has_errors(),
        "unexpected diagnostics for {src:?}:\n{}",
        rendered(src, &diags)
    );
    typhoon_ast::dump_expr(&expr)
}

/// A compact, parenthesised rendering of an expression, so that precedence
/// tests read like a table.
pub fn sexpr(e: &Expr) -> String {
    match &e.kind {
        ExprKind::Int(v) => v.to_string(),
        ExprKind::Float(v) => format!("{v:?}"),
        ExprKind::Str(v) => format!("{v:?}"),
        ExprKind::Bool(true) => "True".to_string(),
        ExprKind::Bool(false) => "False".to_string(),
        ExprKind::None => "None".to_string(),
        ExprKind::Name(n) => n.name.clone(),
        ExprKind::Error => "<error>".to_string(),
        ExprKind::Unary { op, expr, .. } => format!("({op} {})", sexpr(expr)),
        ExprKind::Binary { op, lhs, rhs, .. } => {
            format!("({op} {} {})", sexpr(lhs), sexpr(rhs))
        }
        ExprKind::BoolOp { op, lhs, rhs, .. } => {
            format!("({op} {} {})", sexpr(lhs), sexpr(rhs))
        }
        ExprKind::Compare { left, tail } => {
            let mut out = format!("(cmp {}", sexpr(left));
            for t in tail {
                out.push_str(&format!(" {} {}", t.op, sexpr(&t.rhs)));
            }
            out.push(')');
            out
        }
        ExprKind::Attribute { base, attr } => format!("(. {} {})", sexpr(base), attr.name),
        ExprKind::Index { base, index } => format!("(index {} {})", sexpr(base), sexpr(index)),
        ExprKind::Call {
            callee,
            type_args,
            args,
        } => {
            let mut out = format!("(call {}", sexpr(callee));
            if let Some(type_args) = type_args {
                let list: Vec<String> = type_args.iter().map(stype).collect();
                out.push_str(&format!("<{}>", list.join(",")));
            }
            for arg in args {
                match &arg.name {
                    Some(name) => out.push_str(&format!(" {}={}", name.name, sexpr(&arg.value))),
                    None => out.push_str(&format!(" {}", sexpr(&arg.value))),
                }
            }
            out.push(')');
            out
        }
        ExprKind::List(items) => format!("(list{})", items_str(items)),
        ExprKind::Set(items) => format!("(set{})", items_str(items)),
        ExprKind::Tuple(items) => format!("(tuple{})", items_str(items)),
        ExprKind::Dict(entries) => {
            let mut out = String::from("(dict");
            for (k, v) in entries {
                out.push_str(&format!(" {}:{}", sexpr(k), sexpr(v)));
            }
            out.push(')');
            out
        }
        ExprKind::FString(parts) => {
            let mut out = String::from("(fstr");
            for part in parts {
                match part {
                    FStringPart::Literal { value, .. } => out.push_str(&format!(" {value:?}")),
                    FStringPart::Expr { expr, spec, .. } => {
                        out.push_str(&format!(" {{{}", sexpr(expr)));
                        if let Some(spec) = spec {
                            out.push_str(&format!(":{spec}"));
                        }
                        out.push('}');
                    }
                }
            }
            out.push(')');
            out
        }
    }
}

fn items_str(items: &[Expr]) -> String {
    items.iter().map(|i| format!(" {}", sexpr(i))).collect()
}

/// A compact rendering of a type expression.
pub fn stype(t: &TypeExpr) -> String {
    match &t.kind {
        TypeExprKind::Named { name, args } => {
            if args.is_empty() {
                name.name.clone()
            } else {
                let list: Vec<String> = args.iter().map(stype).collect();
                format!("{}<{}>", name.name, list.join(","))
            }
        }
        TypeExprKind::Union(members) => {
            let list: Vec<String> = members.iter().map(stype).collect();
            list.join("|")
        }
        TypeExprKind::None => "None".to_string(),
        TypeExprKind::Error => "<error>".to_string(),
    }
}

/// Wraps `body` in `fn main():` and returns the statements of the body.
pub fn stmts(body: &str) -> Vec<typhoon_ast::Stmt> {
    let src = wrap_in_main(body);
    let module = module(&src);
    match &module.items[0] {
        typhoon_ast::Item::Fn(decl) => decl.body.stmts.clone(),
        other => panic!("expected a function, got {other:?}"),
    }
}

/// Wraps `body` in `fn main():`, indenting every non-empty line by 4 spaces.
pub fn wrap_in_main(body: &str) -> String {
    let mut src = String::from("fn main():\n");
    for line in body.lines() {
        if line.trim().is_empty() {
            src.push('\n');
        } else {
            src.push_str("    ");
            src.push_str(line);
            src.push('\n');
        }
    }
    src
}

/// The dump of the module produced by wrapping `body` in `fn main():`.
pub fn stmts_dump(body: &str) -> String {
    let src = wrap_in_main(body);
    typhoon_ast::dump(&module(&src))
}
