//! The typhoon parser: a hand-written recursive-descent parser with a
//! precedence-climbing expression parser.
//!
//! The parser calls the lexer itself, never bails out on the first error, and
//! reports every problem it finds into a [`Diagnostics`] sink; on a syntax
//! error it records one diagnostic and then synchronizes (to the end of the
//! line inside a block, to the next top-level item for a broken item header)
//! so that later errors in the same file are still reported.
//!
//! ```
//! use typhoon_diag::{Diagnostics, SourceMap};
//!
//! let mut sources = SourceMap::new();
//! let src = "fn main():\n    print(1 + 2)\n";
//! let file = sources.add("main.ty", src);
//!
//! let mut diags = Diagnostics::new();
//! let module = typhoon_parser::parse_module(file, src, &mut diags);
//!
//! assert!(!diags.has_errors());
//! assert_eq!(module.items.len(), 1);
//! ```

#![warn(missing_docs)]

mod exprs;
mod items;
mod parser;
mod stmts;
mod types;

use typhoon_ast::{Expr, Module};
use typhoon_diag::{Diagnostics, FileId, Span};

use crate::parser::Parser;

/// Parses a whole source file into a [`Module`].
///
/// `file` must be the [`FileId`] that `src` was registered under in the
/// [`SourceMap`](typhoon_diag::SourceMap), so that spans point at the right
/// file. Lexing happens inside; both lexer and parser diagnostics end up in
/// `diags`. A module is always returned, even for input full of errors: check
/// [`Diagnostics::has_errors`] before using it.
pub fn parse_module(file: FileId, src: &str, diags: &mut Diagnostics) -> Module {
    let tokens = typhoon_lexer::tokenize(file, src, diags);
    let mut parser = Parser::new(file, tokens, diags);
    let mut items = Vec::new();
    loop {
        parser.skip_newlines();
        if parser.at_eof() {
            break;
        }
        let before = parser.pos();
        if let Some(item) = parser.parse_item() {
            items.push(item);
        }
        // Guarantee progress even if an item parser consumed nothing.
        if parser.pos() == before {
            parser.bump();
        }
    }
    Module {
        items,
        span: Span::new(file, 0, src.len() as u32),
    }
}

/// Parses a single expression, for tests and tools.
///
/// Trailing tokens after the expression are reported as an error.
pub fn parse_expr(file: FileId, src: &str, diags: &mut Diagnostics) -> Expr {
    let tokens = typhoon_lexer::tokenize(file, src, diags);
    let mut parser = Parser::new(file, tokens, diags);
    let expr = parser.parse_expr();
    parser.skip_newlines();
    if !parser.at_eof() {
        parser.expected("end of input");
    }
    expr
}
