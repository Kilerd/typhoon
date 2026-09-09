//! One test per error case: the message, the recovery, and the fact that
//! lexing keeps going afterwards.

mod common;

use common::{content_kinds_lossy, error_span, lex, messages, str_value};
use typhoon_lexer::TokenKind as K;

/// Lexes `head` followed by another statement and asserts that the expected
/// message was reported and that the later line still produced its tokens.
#[track_caller]
fn assert_recovers(head: &str, needle: &str) {
    let src = format!("{head}\ny = 2\n");
    let (tokens, diags) = lex(&src);
    let messages: Vec<_> = diags.iter().map(|d| d.message.clone()).collect();
    assert!(
        messages.iter().any(|m| m.contains(needle)),
        "expected a message containing {needle:?}, got {messages:?}"
    );
    for diag in diags.iter() {
        assert!(diag.is_error(), "lexer diagnostics are errors");
        let span = diag
            .span()
            .unwrap_or_else(|| panic!("{:?} has no label", diag.message));
        assert!(
            span.end as usize <= src.len(),
            "{:?} points outside the file: {span}",
            diag.message
        );
        assert!(
            !diag.message.is_empty()
                && diag.message.chars().next().is_some_and(char::is_lowercase)
                && !diag.message.ends_with('.'),
            "{:?} does not follow the rustc message conventions",
            diag.message
        );
    }
    let kinds: Vec<_> = tokens.iter().map(|t| t.kind.clone()).collect();
    assert!(
        kinds.contains(&K::Ident("y".into())) && kinds.contains(&K::Int(2)),
        "lexing did not continue after the error: {kinds:?}"
    );
    assert_eq!(tokens.last().map(|t| t.kind.clone()), Some(K::Eof));
}

// ------------------------------------------------------- unknown characters

#[test]
fn unknown_character() {
    assert_recovers("x = @", "unknown character `@`");
    // The invalid character produces no token at all.
    assert_eq!(
        content_kinds_lossy("x = @ 1"),
        vec![K::Ident("x".into()), K::Eq, K::Int(1)]
    );
}

#[test]
fn unknown_character_non_ascii_identifier() {
    assert_recovers("café = 1", "unknown character `é`");
    assert_eq!(messages("é").len(), 1);
}

#[test]
fn unknown_character_dollar() {
    assert_recovers("$x", "unknown character `$`");
}

#[test]
fn a_lone_bang_suggests_not() {
    assert_recovers("x = !y", "unknown character `!`");
    let (_, diags) = lex("!");
    let help = diags
        .iter()
        .flat_map(|d| d.help.clone())
        .collect::<Vec<_>>()
        .join(" ");
    assert!(help.contains("use `not` for logical negation"), "{help}");
    assert!(help.contains("`!=` for inequality"), "{help}");
}

// ------------------------------------------------------------------ numbers

#[test]
fn empty_digits_after_a_base_prefix() {
    assert_recovers("x = 0x", "missing digits after the `0x` base prefix");
    assert_recovers("x = 0b", "missing digits after the `0b` base prefix");
    assert_recovers("x = 0o", "missing digits after the `0o` base prefix");
    assert_eq!(content_kinds_lossy("0x"), vec![K::Int(0)]);
}

#[test]
fn invalid_digit_for_the_base() {
    assert_recovers("x = 0b12", "invalid digit for a base 2 literal");
    assert_recovers("x = 0o8", "invalid digit for a base 8 literal");
    assert_recovers("x = 0xg", "invalid digit for a base 16 literal");
}

#[test]
fn integer_too_large() {
    assert_recovers(
        "x = 9223372036854775808",
        "integer literal is too large for `int`",
    );
    assert_eq!(content_kinds_lossy("99999999999999999999"), vec![K::Int(0)]);
    let (_, diags) = lex("9223372036854775808");
    let notes = diags
        .iter()
        .flat_map(|d| d.notes.clone())
        .collect::<Vec<_>>()
        .join(" ");
    assert!(notes.contains("64-bit signed integer"), "{notes}");
}

#[test]
fn hex_integer_too_large() {
    assert_recovers(
        "x = 0xffffffffffffffff",
        "integer literal is too large for `int`",
    );
}

#[test]
fn float_without_a_leading_digit() {
    assert_recovers(
        "x = .5",
        "floats must have a digit before the decimal point",
    );
    assert_eq!(content_kinds_lossy(".5"), vec![K::Float(0.5)]);
    let (_, diags) = lex(".5");
    let help = diags
        .iter()
        .flat_map(|d| d.help.clone())
        .collect::<Vec<_>>()
        .join(" ");
    assert!(help.contains("`0.5`"), "{help}");
}

#[test]
fn float_without_a_trailing_digit() {
    assert_recovers("x = 1.", "floats must have a digit after the decimal point");
    assert_eq!(content_kinds_lossy("1."), vec![K::Float(1.0)]);
    let (_, diags) = lex("1.");
    let help = diags
        .iter()
        .flat_map(|d| d.help.clone())
        .collect::<Vec<_>>()
        .join(" ");
    assert!(help.contains("`1.0`"), "{help}");
}

#[test]
fn exponent_without_digits() {
    assert_recovers("x = 1e", "expected at least one digit in the exponent");
    assert_recovers("x = 1e+", "expected at least one digit in the exponent");
    assert_recovers("x = 1.5e-", "expected at least one digit in the exponent");
    assert_eq!(content_kinds_lossy("1e"), vec![K::Float(1.0)]);
    assert_eq!(content_kinds_lossy("2.5e+"), vec![K::Float(2.5)]);
}

#[test]
fn number_glued_to_an_identifier() {
    assert_recovers("x = 123abc", "invalid suffix `abc` on a numeric literal");
    assert_eq!(content_kinds_lossy("123abc"), vec![K::Int(123)]);
    assert_eq!(content_kinds_lossy("1.5f"), vec![K::Float(1.5)]);
    // The suffix is swallowed by the literal instead of becoming an identifier.
    assert_eq!(messages("123abc").len(), 1);
}

// ------------------------------------------------------------------ strings

#[test]
fn unknown_escape() {
    assert_recovers(r#"x = "a\qb""#, r"unknown character escape `\q`");
    assert_eq!(str_value(r#""a\qb""#, 0), "aqb");
    // The label covers exactly the two characters of the escape.
    let span = error_span(r#""a\qb""#, "unknown character escape");
    assert_eq!(&r#""a\qb""#[span.range()], r"\q");
}

#[test]
fn bad_hex_escape() {
    assert_recovers(r#"x = "\xg1""#, r"invalid `\x` escape");
    assert_recovers(r#"x = "\x4""#, r"invalid `\x` escape");
    assert_eq!(str_value(r#""a\x""#, 0), "a");
}

#[test]
fn bad_unicode_escape_without_braces() {
    assert_recovers(r#"x = "\u41""#, r"invalid `\u` escape");
    assert_eq!(str_value(r#""\u41""#, 0), "41");
}

#[test]
fn unterminated_unicode_escape() {
    assert_recovers(r#"x = "\u{41""#, r"unterminated `\u` escape");
}

#[test]
fn empty_unicode_escape() {
    assert_recovers(r#"x = "\u{}""#, r"empty `\u` escape");
}

#[test]
fn overlong_unicode_escape() {
    assert_recovers(r#"x = "\u{1234567}""#, r"overlong `\u` escape");
}

#[test]
fn unicode_escape_that_is_not_a_scalar_value() {
    assert_recovers(r#"x = "\u{d800}""#, "invalid unicode scalar value");
    assert_recovers(r#"x = "\u{110000}""#, "invalid unicode scalar value");
    assert_eq!(str_value(r#""a\u{d800}b""#, 0), "ab");
}

#[test]
fn unterminated_string_at_end_of_line() {
    assert_recovers(r#"x = "oops"#, "unterminated string literal");
    // Recovery stops the string at the line break and the next line is normal.
    let src = "x = \"oops\ny = 2\n";
    let (tokens, _) = lex(src);
    let kinds: Vec<_> = tokens.into_iter().map(|t| t.kind).collect();
    assert_eq!(
        kinds,
        vec![
            K::Ident("x".into()),
            K::Eq,
            K::Str("oops".into()),
            K::Newline,
            K::Ident("y".into()),
            K::Eq,
            K::Int(2),
            K::Newline,
            K::Eof,
        ]
    );
}

#[test]
fn unterminated_string_primary_label_is_the_opening_quote() {
    let src = "x = 'oops\n";
    let span = error_span(src, "unterminated string literal");
    assert_eq!(&src[span.range()], "'");
    assert_eq!(span.start, 4);
}

#[test]
fn unterminated_string_at_end_of_file() {
    let (tokens, diags) = lex("x = \"oops");
    assert_eq!(diags.len(), 1);
    let kinds: Vec<_> = tokens.into_iter().map(|t| t.kind).collect();
    assert_eq!(
        kinds,
        vec![
            K::Ident("x".into()),
            K::Eq,
            K::Str("oops".into()),
            K::Newline,
            K::Eof
        ]
    );
}

#[test]
fn a_backslash_at_the_end_of_a_line_does_not_continue_the_string() {
    let src = "x = \"oops\\\ny = 2\n";
    let (tokens, diags) = lex(src);
    assert!(
        diags
            .iter()
            .any(|d| d.message.contains("unterminated string literal")),
        "{:?}",
        diags.iter().map(|d| d.message.clone()).collect::<Vec<_>>()
    );
    let kinds: Vec<_> = tokens.into_iter().map(|t| t.kind).collect();
    assert!(kinds.contains(&K::Ident("y".into())), "{kinds:?}");
}

#[test]
fn a_lone_backslash_terminates_and_does_not_hang() {
    let (_, diags) = lex("\\");
    assert_eq!(diags.len(), 1);
    assert!(
        diags
            .iter()
            .next()
            .expect("one")
            .message
            .contains("unknown character")
    );
}
