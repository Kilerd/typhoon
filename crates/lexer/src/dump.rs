//! Deterministic textual rendering of a token stream, for snapshot tests.

use std::fmt::Write as _;

use typhoon_diag::Span;

use crate::token::{RawFStringPart, Token, TokenKind};

/// The variant name of a token kind, e.g. `Ident`, `Int`, `Newline`.
fn variant_name(kind: &TokenKind) -> String {
    let debug = format!("{kind:?}");
    match debug.split('(').next() {
        Some(name) => name.to_string(),
        None => debug,
    }
}

/// The `<Kind> <detail?>` part of a dump line.
fn head(kind: &TokenKind) -> String {
    let name = variant_name(kind);
    match kind {
        TokenKind::Ident(name_text) => format!("{name} `{name_text}`"),
        TokenKind::Int(value) => format!("{name} {value}"),
        TokenKind::Float(value) => format!("{name} {value:?}"),
        TokenKind::Str(value) => format!("{name} {value:?}"),
        _ => name,
    }
}

/// Renders a span as `@start..end`.
fn at(span: Span) -> String {
    format!("@{}..{}", span.start, span.end)
}

/// Renders a token stream as one token per line, for `insta` snapshots and for
/// debugging.
///
/// The format is `<Kind> <detail?> @<start>..<end>`; the parts of an f-string
/// follow on indented continuation lines. String and float payloads are printed
/// with `{:?}` so that they round-trip. There is no trailing newline.
///
/// ```
/// use typhoon_diag::{Diagnostics, FileId};
/// use typhoon_lexer::{dump_tokens, tokenize};
///
/// let mut diags = Diagnostics::new();
/// let tokens = tokenize(FileId(0), "x = 42", &mut diags);
/// assert_eq!(
///     dump_tokens(&tokens),
///     "Ident `x` @0..1\nEq @2..3\nInt 42 @4..6\nNewline @6..6\nEof @6..6"
/// );
/// ```
pub fn dump_tokens(tokens: &[Token]) -> String {
    let mut out = String::new();
    for (index, token) in tokens.iter().enumerate() {
        if index > 0 {
            out.push('\n');
        }
        let _ = write!(out, "{} {}", head(&token.kind), at(token.span));
        if let TokenKind::FString(parts) = &token.kind {
            for part in parts {
                out.push('\n');
                match part {
                    RawFStringPart::Literal { value, span } => {
                        let _ = write!(out, "    Literal {value:?} {}", at(*span));
                    }
                    RawFStringPart::Expr {
                        text,
                        span,
                        spec,
                        spec_span,
                        full_span,
                    } => {
                        let _ = write!(out, "    Expr {text:?} {}", at(*span));
                        if let Some(spec) = spec {
                            let _ = write!(out, " spec {spec:?}");
                            if let Some(spec_span) = spec_span {
                                let _ = write!(out, " {}", at(*spec_span));
                            }
                        }
                        let _ = write!(out, " full {}", at(*full_span));
                    }
                }
            }
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use typhoon_diag::{FileId, Span};

    use super::*;

    const F: FileId = FileId(0);

    fn span(start: u32, end: u32) -> Span {
        Span::new(F, start, end)
    }

    #[test]
    fn variant_names() {
        assert_eq!(variant_name(&TokenKind::Fn), "Fn");
        assert_eq!(variant_name(&TokenKind::Ident("x".into())), "Ident");
        assert_eq!(variant_name(&TokenKind::Int(1)), "Int");
        assert_eq!(variant_name(&TokenKind::Str("a(b".into())), "Str");
        assert_eq!(variant_name(&TokenKind::FString(vec![])), "FString");
        assert_eq!(variant_name(&TokenKind::GtGt), "GtGt");
        assert_eq!(variant_name(&TokenKind::Eof), "Eof");
    }

    #[test]
    fn heads_carry_the_payload() {
        assert_eq!(head(&TokenKind::Fn), "Fn");
        assert_eq!(head(&TokenKind::Ident("main".into())), "Ident `main`");
        assert_eq!(head(&TokenKind::Int(-3)), "Int -3");
        assert_eq!(head(&TokenKind::Float(1.5)), "Float 1.5");
        assert_eq!(head(&TokenKind::Float(2.0)), "Float 2.0");
        assert_eq!(head(&TokenKind::Str("a\nb".into())), "Str \"a\\nb\"");
    }

    #[test]
    fn empty_input_is_an_empty_dump() {
        assert_eq!(dump_tokens(&[]), "");
    }

    #[test]
    fn one_token_per_line_without_a_trailing_newline() {
        let tokens = vec![
            Token::new(TokenKind::Fn, span(0, 2)),
            Token::new(TokenKind::Newline, span(2, 3)),
            Token::new(TokenKind::Eof, span(3, 3)),
        ];
        assert_eq!(dump_tokens(&tokens), "Fn @0..2\nNewline @2..3\nEof @3..3");
    }

    #[test]
    fn f_string_parts_are_indented_continuation_lines() {
        let tokens = vec![Token::new(
            TokenKind::FString(vec![
                RawFStringPart::Literal {
                    value: "i=".into(),
                    span: span(2, 4),
                },
                RawFStringPart::Expr {
                    text: "i".into(),
                    span: span(5, 6),
                    spec: None,
                    spec_span: None,
                    full_span: span(4, 7),
                },
                RawFStringPart::Expr {
                    text: "x".into(),
                    span: span(8, 9),
                    spec: Some(".2f".into()),
                    spec_span: Some(span(10, 13)),
                    full_span: span(7, 14),
                },
            ]),
            span(0, 15),
        )];
        assert_eq!(
            dump_tokens(&tokens),
            "FString @0..15\n    \
             Literal \"i=\" @2..4\n    \
             Expr \"i\" @5..6 full @4..7\n    \
             Expr \"x\" @8..9 spec \".2f\" @10..13 full @7..14"
        );
    }

    #[test]
    fn an_f_string_without_parts_takes_one_line() {
        let tokens = vec![Token::new(TokenKind::FString(vec![]), span(0, 3))];
        assert_eq!(dump_tokens(&tokens), "FString @0..3");
    }
}
