//! Diagnostics infrastructure shared by every phase of the typhoon compiler.
//!
//! The crate provides three things:
//!
//! * [`SourceMap`] / [`FileId`] / [`Span`] — where source text lives and how
//!   the rest of the compiler points into it,
//! * [`Diagnostic`] / [`Diagnostics`] — what a compiler message is and how
//!   messages are collected (phases never bail out on the first error),
//! * [`render`] / [`render_all`] — rustc-style textual rendering.
//!
//! ```
//! use typhoon_diag::{Diagnostic, Diagnostics, SourceMap, Span, render};
//!
//! let mut sources = SourceMap::new();
//! let file = sources.add("main.ty", "fn main():\n    x = 1\n");
//!
//! let mut diags = Diagnostics::new();
//! diags.push(
//!     Diagnostic::error("unknown variable `y`")
//!         .with_label(Span::new(file, 15, 16), "not found in this scope"),
//! );
//!
//! assert!(diags.has_errors());
//! let text = render(&diags.iter().next().unwrap().clone(), &sources, false);
//! assert!(text.starts_with("error: unknown variable `y`"));
//! ```

#![warn(missing_docs)]

mod diagnostic;
mod render;
mod source_map;
mod span;

pub use diagnostic::{Diagnostic, Diagnostics, Label, Level};
pub use render::{render, render_all};
pub use source_map::SourceMap;
pub use span::{FileId, Span};
