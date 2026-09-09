//! Helpers shared by the lexer integration tests.

#![allow(dead_code)]

use typhoon_diag::{Diagnostics, FileId, Span};
use typhoon_lexer::{Token, TokenKind, dump_tokens, tokenize};

/// The file id every test lexes into.
pub const F: FileId = FileId(0);

/// Lexes `src`, returning the tokens and the diagnostics.
pub fn lex(src: &str) -> (Vec<Token>, Diagnostics) {
    let mut diags = Diagnostics::new();
    let tokens = tokenize(F, src, &mut diags);
    (tokens, diags)
}

/// Lexes `src` and asserts that it produced no diagnostic at all.
pub fn lex_ok(src: &str) -> Vec<Token> {
    let (tokens, diags) = lex(src);
    assert!(
        diags.is_empty(),
        "expected no diagnostics for {src:?}, got {:?}",
        messages_of(&diags)
    );
    tokens
}

/// The kinds of every token, layout tokens included.
pub fn kinds(src: &str) -> Vec<TokenKind> {
    lex_ok(src).into_iter().map(|t| t.kind).collect()
}

/// The kinds of every token, ignoring diagnostics.
pub fn kinds_lossy(src: &str) -> Vec<TokenKind> {
    lex(src).0.into_iter().map(|t| t.kind).collect()
}

/// Whether a token kind is a layout token.
pub fn is_layout(kind: &TokenKind) -> bool {
    matches!(
        kind,
        TokenKind::Newline | TokenKind::Indent | TokenKind::Dedent | TokenKind::Eof
    )
}

/// The kinds of the non-layout tokens of a source that lexes cleanly.
pub fn content_kinds(src: &str) -> Vec<TokenKind> {
    kinds(src).into_iter().filter(|k| !is_layout(k)).collect()
}

/// The kinds of the non-layout tokens, ignoring diagnostics.
pub fn content_kinds_lossy(src: &str) -> Vec<TokenKind> {
    kinds_lossy(src)
        .into_iter()
        .filter(|k| !is_layout(k))
        .collect()
}

/// The messages of the collected diagnostics.
pub fn messages_of(diags: &Diagnostics) -> Vec<String> {
    diags.iter().map(|d| d.message.clone()).collect()
}

/// The diagnostic messages produced by lexing `src`.
pub fn messages(src: &str) -> Vec<String> {
    messages_of(&lex(src).1)
}

/// The single diagnostic produced by lexing `src`; panics if there is not
/// exactly one.
pub fn only_error(src: &str) -> typhoon_diag::Diagnostic {
    let (_, diags) = lex(src);
    assert_eq!(
        diags.len(),
        1,
        "expected exactly one diagnostic for {src:?}: {:?}",
        messages_of(&diags)
    );
    diags.iter().next().expect("just checked").clone()
}

/// Asserts that lexing `src` reports exactly one error whose message contains
/// `needle`, and returns the primary span of that error.
pub fn error_span(src: &str, needle: &str) -> Span {
    let diag = only_error(src);
    assert!(
        diag.message.contains(needle),
        "expected message containing {needle:?}, got {:?}",
        diag.message
    );
    diag.span().expect("every lexer diagnostic has a label")
}

/// The dump of a source, ignoring diagnostics.
pub fn dump(src: &str) -> String {
    dump_tokens(&lex(src).0)
}

/// The string payload of the `n`-th `Str` token.
pub fn str_value(src: &str, n: usize) -> String {
    lex(src)
        .0
        .into_iter()
        .filter_map(|t| match t.kind {
            TokenKind::Str(s) => Some(s),
            _ => None,
        })
        .nth(n)
        .unwrap_or_else(|| panic!("no string literal #{n} in {src:?}"))
}
