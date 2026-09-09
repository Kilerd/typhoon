//! typhoon-lexer: the hand-written, indentation-aware lexer (see DESIGN 6.1).
//!
//! [`tokenize`] turns source text into a `Vec<Token>` and pushes every problem
//! it finds into a [`Diagnostics`]; it never fails and never panics, so the
//! parser always gets a complete, well-formed token stream to work with.
//!
//! ```
//! use typhoon_diag::{Diagnostics, FileId};
//! use typhoon_lexer::{TokenKind, dump_tokens, tokenize};
//!
//! let mut diags = Diagnostics::new();
//! let tokens = tokenize(FileId(0), "fn main():\n    pass\n", &mut diags);
//! assert!(diags.is_empty());
//! assert_eq!(tokens[0].kind, TokenKind::Fn);
//! assert_eq!(
//!     dump_tokens(&tokens),
//!     "Fn @0..2\n\
//!      Ident `main` @3..7\n\
//!      LParen @7..8\n\
//!      RParen @8..9\n\
//!      Colon @9..10\n\
//!      Newline @10..11\n\
//!      Indent @15..15\n\
//!      Pass @15..19\n\
//!      Newline @19..20\n\
//!      Dedent @20..20\n\
//!      Eof @20..20"
//! );
//! ```
//!
//! # Shape of the token stream
//!
//! The layout rules are the part a parser author has to know by heart:
//!
//! * **Newline.** Every logical line that produced at least one token is
//!   terminated by exactly one [`TokenKind::Newline`], whose span covers the
//!   line break (`\n` or `\r\n`). If the file does not end with a line break,
//!   a `Newline` with an empty span at the end of the file is synthesized.
//! * **Indent / Dedent.** [`TokenKind::Indent`] and [`TokenKind::Dedent`] have
//!   empty spans positioned at the first token of the line they belong to.
//!   They are emitted *before* that token, i.e. after the previous `Newline`.
//!   `Indent` and `Dedent` are always balanced: at the end of the file one
//!   `Dedent` is emitted for every still-open block.
//! * **Nothing tokens.** Blank lines, comment-only lines (`# ...`) and lines
//!   that consist only of invalid characters produce no tokens at all and do
//!   not touch the indentation stack.
//! * **Implicit continuation.** Inside `(`, `[` or `{` a line break produces
//!   nothing: no `Newline`, no `Indent`, no `Dedent`.
//! * **End of file.** The very last tokens are always, in this order: the
//!   optional synthesized `Newline`, zero or more `Dedent`, exactly one
//!   [`TokenKind::Eof`]. All three have an empty span at the end of the file.
//!
//! Literal tokens carry decoded values: [`TokenKind::Int`] has `_` separators
//! and the `0x` / `0b` / `0o` prefix already resolved, [`TokenKind::Str`] has
//! its escapes decoded. Only f-strings keep raw text: the holes of a
//! [`TokenKind::FString`] are [`RawFStringPart::Expr`] values whose `text` is
//! *exactly* the source slice of their `span`, so the parser can re-lex them
//! with [`tokenize_fragment`] and still get spans that point into the file.
//!
//! # Error recovery
//!
//! Every diagnostic is followed by a best-effort recovery so that lexing
//! continues to the end of the file: an unterminated string ends at the line
//! break, a bad escape keeps its character, an out-of-range integer becomes
//! `0`, an unknown character is skipped without producing a token, and a
//! dedent to an unknown column adjusts the indentation stack instead of
//! desynchronizing the block structure.

#![warn(missing_docs)]

mod dump;
mod scanner;
mod token;

use typhoon_diag::{Diagnostics, FileId};

pub use dump::dump_tokens;
pub use token::{RawFStringPart, Token, TokenKind};

/// Tokenizes a whole source file.
///
/// `file` is only used to build the spans; the text itself comes from `src`.
/// Problems are pushed into `diags` and recovered from, so the returned stream
/// is always complete (it always ends with [`TokenKind::Eof`]) even for
/// nonsensical input.
pub fn tokenize(file: FileId, src: &str, diags: &mut Diagnostics) -> Vec<Token> {
    tokenize_fragment(file, src, 0, diags)
}

/// Tokenizes a fragment of a file, such as the expression inside an f-string
/// hole.
///
/// Identical to [`tokenize`], except that every span — of tokens *and* of
/// diagnostics — is shifted by `base_offset`, so that it points at the right
/// place in the enclosing file. `tokenize(file, src, diags)` is exactly
/// `tokenize_fragment(file, src, 0, diags)`.
///
/// ```
/// use typhoon_diag::{Diagnostics, FileId};
/// use typhoon_lexer::{TokenKind, tokenize_fragment};
///
/// // the hole of `print(f"{x + 1}")`, which starts at byte 9
/// let mut diags = Diagnostics::new();
/// let tokens = tokenize_fragment(FileId(0), "x + 1", 9, &mut diags);
/// assert_eq!(tokens[0].kind, TokenKind::Ident("x".into()));
/// assert_eq!(tokens[0].span.start, 9);
/// ```
pub fn tokenize_fragment(
    file: FileId,
    src: &str,
    base_offset: u32,
    diags: &mut Diagnostics,
) -> Vec<Token> {
    scanner::Scanner::new(file, src, base_offset, diags).run()
}
