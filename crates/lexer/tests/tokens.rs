//! One focused test per token class: keywords, identifiers, operators and
//! literals.

mod common;

use common::{content_kinds, content_kinds_lossy, kinds, lex, lex_ok, str_value};
use typhoon_lexer::TokenKind as K;

// ------------------------------------------------------------------ keywords

/// Every keyword, one by one, as its own source file.
#[test]
fn every_keyword_is_its_own_token() {
    let keywords = [
        ("fn", K::Fn),
        ("class", K::Class),
        ("if", K::If),
        ("elif", K::Elif),
        ("else", K::Else),
        ("while", K::While),
        ("for", K::For),
        ("in", K::In),
        ("not", K::Not),
        ("and", K::And),
        ("or", K::Or),
        ("return", K::Return),
        ("break", K::Break),
        ("continue", K::Continue),
        ("pass", K::Pass),
        ("True", K::True),
        ("False", K::False),
        ("None", K::None),
        ("is", K::Is),
        ("import", K::Import),
        ("from", K::From),
        ("try", K::Try),
        ("except", K::Except),
        ("finally", K::Finally),
        ("raise", K::Raise),
        ("match", K::Match),
        ("case", K::Case),
        ("as", K::As),
    ];
    for (text, kind) in keywords {
        assert_eq!(
            content_kinds(text),
            vec![kind.clone()],
            "lexing keyword {text:?}"
        );
        let spanned = lex_ok(text);
        assert_eq!(spanned[0].span.start, 0);
        assert_eq!(spanned[0].span.end, text.len() as u32, "span of {text:?}");
    }
}

#[test]
fn keywords_only_match_whole_words() {
    for text in ["fnord", "iffy", "classy", "_fn", "None2", "istrue", "As"] {
        assert_eq!(
            content_kinds(text),
            vec![K::Ident(text.to_string())],
            "{text:?} must be an identifier"
        );
    }
}

// --------------------------------------------------------------- identifiers

#[test]
fn identifiers() {
    for text in [
        "x",
        "_",
        "_x",
        "x1",
        "snake_case",
        "CamelCase",
        "__dunder__",
        "a0_9",
    ] {
        assert_eq!(content_kinds(text), vec![K::Ident(text.to_string())]);
    }
}

#[test]
fn identifier_cannot_start_with_a_digit() {
    // `1x` is a bad literal, not an identifier: see errors.rs.
    assert_eq!(content_kinds_lossy("1x"), vec![K::Int(1)]);
}

#[test]
fn identifiers_are_separated_by_operators() {
    assert_eq!(
        content_kinds("a+b"),
        vec![K::Ident("a".into()), K::Plus, K::Ident("b".into())]
    );
}

// ----------------------------------------------------------------- operators

#[test]
fn every_operator_and_punctuation() {
    let ops = [
        ("+", K::Plus),
        ("-", K::Minus),
        ("*", K::Star),
        ("/", K::Slash),
        ("//", K::SlashSlash),
        ("%", K::Percent),
        ("**", K::StarStar),
        ("==", K::EqEq),
        ("!=", K::BangEq),
        ("<", K::Lt),
        ("<=", K::LtEq),
        (">", K::Gt),
        (">=", K::GtEq),
        ("=", K::Eq),
        (":", K::Colon),
        (",", K::Comma),
        (".", K::Dot),
        ("(", K::LParen),
        (")", K::RParen),
        ("[", K::LBracket),
        ("]", K::RBracket),
        ("{", K::LBrace),
        ("}", K::RBrace),
        ("->", K::Arrow),
        ("|", K::Pipe),
        ("&", K::Amp),
        ("^", K::Caret),
        ("~", K::Tilde),
        ("<<", K::LtLt),
        (">>", K::GtGt),
        ("+=", K::PlusEq),
        ("-=", K::MinusEq),
        ("*=", K::StarEq),
        ("/=", K::SlashEq),
        (";", K::Semi),
    ];
    for (text, kind) in ops {
        assert_eq!(
            content_kinds(text),
            vec![kind.clone()],
            "lexing operator {text:?}"
        );
        let tokens = lex_ok(text);
        assert_eq!(tokens[0].span.end - tokens[0].span.start, text.len() as u32);
    }
}

#[test]
fn semi_is_lexed_without_a_diagnostic() {
    // The parser, not the lexer, explains that typhoon has no `;`.
    assert_eq!(
        content_kinds("a;b"),
        vec![K::Ident("a".into()), K::Semi, K::Ident("b".into())]
    );
}

// ------------------------------------------------------------ maximal munch

#[test]
fn maximal_munch_greater_than() {
    assert_eq!(content_kinds(">> >= >"), vec![K::GtGt, K::GtEq, K::Gt]);
    assert_eq!(content_kinds(">>="), vec![K::GtGt, K::Eq]);
    assert_eq!(content_kinds(">>>"), vec![K::GtGt, K::Gt]);
    assert_eq!(
        content_kinds("a>b"),
        vec![K::Ident("a".into()), K::Gt, K::Ident("b".into())]
    );
}

#[test]
fn maximal_munch_less_than() {
    assert_eq!(content_kinds("<= << <"), vec![K::LtEq, K::LtLt, K::Lt]);
    assert_eq!(content_kinds("<<="), vec![K::LtLt, K::Eq]);
    assert_eq!(
        content_kinds("list<int>"),
        vec![
            K::Ident("list".into()),
            K::Lt,
            K::Ident("int".into()),
            K::Gt
        ]
    );
    assert_eq!(
        content_kinds("dict<str, list<int>>"),
        vec![
            K::Ident("dict".into()),
            K::Lt,
            K::Ident("str".into()),
            K::Comma,
            K::Ident("list".into()),
            K::Lt,
            K::Ident("int".into()),
            K::GtGt,
        ]
    );
}

#[test]
fn maximal_munch_minus() {
    assert_eq!(
        content_kinds("-> - -= --"),
        vec![K::Arrow, K::Minus, K::MinusEq, K::Minus, K::Minus]
    );
    assert_eq!(
        content_kinds("a-b"),
        vec![K::Ident("a".into()), K::Minus, K::Ident("b".into())]
    );
}

#[test]
fn maximal_munch_slash() {
    assert_eq!(
        content_kinds("// / /= ///"),
        vec![K::SlashSlash, K::Slash, K::SlashEq, K::SlashSlash, K::Slash]
    );
}

#[test]
fn maximal_munch_star() {
    assert_eq!(
        content_kinds("** * *= ***"),
        vec![K::StarStar, K::Star, K::StarEq, K::StarStar, K::Star]
    );
}

#[test]
fn maximal_munch_equals() {
    assert_eq!(
        content_kinds("== = === !="),
        vec![K::EqEq, K::Eq, K::EqEq, K::Eq, K::BangEq]
    );
}

#[test]
fn maximal_munch_plus() {
    assert_eq!(
        content_kinds("+ += ++"),
        vec![K::Plus, K::PlusEq, K::Plus, K::Plus]
    );
}

// ----------------------------------------------------------------- integers

#[test]
fn decimal_integers() {
    assert_eq!(content_kinds("0"), vec![K::Int(0)]);
    assert_eq!(content_kinds("42"), vec![K::Int(42)]);
    assert_eq!(content_kinds("007"), vec![K::Int(7)]);
    assert_eq!(content_kinds("9223372036854775807"), vec![K::Int(i64::MAX)]);
}

#[test]
fn decimal_integers_with_separators() {
    assert_eq!(content_kinds("1_000_000"), vec![K::Int(1_000_000)]);
    assert_eq!(content_kinds("1_0"), vec![K::Int(10)]);
}

#[test]
fn hex_integers() {
    assert_eq!(content_kinds("0x0"), vec![K::Int(0)]);
    assert_eq!(content_kinds("0xff"), vec![K::Int(255)]);
    assert_eq!(content_kinds("0XFF"), vec![K::Int(255)]);
    assert_eq!(content_kinds("0xdead_beef"), vec![K::Int(0xdead_beef)]);
    assert_eq!(content_kinds("0x7fffffffffffffff"), vec![K::Int(i64::MAX)]);
}

#[test]
fn binary_integers() {
    assert_eq!(content_kinds("0b0"), vec![K::Int(0)]);
    assert_eq!(content_kinds("0b1010"), vec![K::Int(10)]);
    assert_eq!(content_kinds("0B1111_0000"), vec![K::Int(0xf0)]);
}

#[test]
fn octal_integers() {
    assert_eq!(content_kinds("0o0"), vec![K::Int(0)]);
    assert_eq!(content_kinds("0o755"), vec![K::Int(0o755)]);
    assert_eq!(content_kinds("0O7_7"), vec![K::Int(0o77)]);
}

#[test]
fn integer_spans_cover_the_whole_literal() {
    let tokens = lex_ok("0xdead_beef");
    assert_eq!((tokens[0].span.start, tokens[0].span.end), (0, 11));
}

// ------------------------------------------------------------------- floats

#[test]
fn simple_floats() {
    assert_eq!(content_kinds("1.5"), vec![K::Float(1.5)]);
    assert_eq!(content_kinds("0.0"), vec![K::Float(0.0)]);
    assert_eq!(content_kinds("12.3456"), vec![K::Float(12.3456)]);
}

#[test]
fn floats_with_exponents() {
    assert_eq!(content_kinds("1e10"), vec![K::Float(1e10)]);
    assert_eq!(content_kinds("1E10"), vec![K::Float(1e10)]);
    assert_eq!(content_kinds("1.5e-3"), vec![K::Float(1.5e-3)]);
    assert_eq!(content_kinds("1.5E+3"), vec![K::Float(1.5e3)]);
    assert_eq!(content_kinds("2e0"), vec![K::Float(2.0)]);
}

#[test]
fn floats_with_separators() {
    assert_eq!(content_kinds("1_000.5"), vec![K::Float(1000.5)]);
    assert_eq!(content_kinds("1.000_1"), vec![K::Float(1.0001)]);
    assert_eq!(content_kinds("1e1_0"), vec![K::Float(1e10)]);
}

#[test]
fn a_dot_after_an_identifier_char_is_attribute_access() {
    // `1.foo` is `1`, `.`, `foo` and not a malformed float.
    assert_eq!(
        content_kinds("1.foo"),
        vec![K::Int(1), K::Dot, K::Ident("foo".into())]
    );
}

#[test]
fn float_then_operator() {
    assert_eq!(
        content_kinds("1.5+2.5"),
        vec![K::Float(1.5), K::Plus, K::Float(2.5)]
    );
}

// ------------------------------------------------------------------ strings

#[test]
fn double_and_single_quoted_strings_are_the_same_token() {
    assert_eq!(content_kinds("\"hi\""), vec![K::Str("hi".into())]);
    assert_eq!(content_kinds("'hi'"), vec![K::Str("hi".into())]);
}

#[test]
fn empty_string() {
    assert_eq!(content_kinds("\"\""), vec![K::Str(String::new())]);
}

#[test]
fn string_can_contain_the_other_quote() {
    assert_eq!(content_kinds("\"it's\""), vec![K::Str("it's".into())]);
    assert_eq!(
        content_kinds("'say \"hi\"'"),
        vec![K::Str("say \"hi\"".into())]
    );
}

#[test]
fn escape_newline() {
    assert_eq!(str_value(r#""a\nb""#, 0), "a\nb");
}

#[test]
fn escape_tab() {
    assert_eq!(str_value(r#""a\tb""#, 0), "a\tb");
}

#[test]
fn escape_carriage_return() {
    assert_eq!(str_value(r#""a\rb""#, 0), "a\rb");
}

#[test]
fn escape_backslash() {
    assert_eq!(str_value(r#""a\\b""#, 0), "a\\b");
}

#[test]
fn escape_double_quote() {
    assert_eq!(str_value(r#""a\"b""#, 0), "a\"b");
}

#[test]
fn escape_single_quote() {
    assert_eq!(str_value(r#""a\'b""#, 0), "a'b");
    assert_eq!(str_value(r"'a\'b'", 0), "a'b");
}

#[test]
fn escape_nul() {
    assert_eq!(str_value(r#""a\0b""#, 0), "a\0b");
}

#[test]
fn escape_hex() {
    assert_eq!(str_value(r#""\x41\x7a""#, 0), "Az");
    // `\xNN` above 0x7f is the scalar value with that code point.
    assert_eq!(str_value(r#""\xe9""#, 0), "\u{e9}");
}

#[test]
fn escape_unicode() {
    assert_eq!(str_value(r#""\u{41}""#, 0), "A");
    assert_eq!(str_value(r#""\u{1f600}""#, 0), "\u{1f600}");
    assert_eq!(str_value(r#""\u{0}""#, 0), "\0");
}

#[test]
fn strings_keep_non_ascii_text() {
    assert_eq!(
        content_kinds("\"héllo → 世界\""),
        vec![K::Str("héllo → 世界".into())]
    );
}

#[test]
fn string_span_covers_the_quotes() {
    let tokens = lex_ok(r#""hi""#);
    assert_eq!((tokens[0].span.start, tokens[0].span.end), (0, 4));
}

#[test]
fn a_hash_inside_a_string_is_not_a_comment() {
    assert_eq!(
        content_kinds("\"# not a comment\""),
        vec![K::Str("# not a comment".into())]
    );
}

// ----------------------------------------------------------------- comments

#[test]
fn comment_to_end_of_line() {
    assert_eq!(kinds("# nothing here\n"), vec![K::Eof]);
}

#[test]
fn comment_after_code() {
    assert_eq!(
        content_kinds("x = 1  # set x\n"),
        vec![K::Ident("x".into()), K::Eq, K::Int(1)]
    );
}

#[test]
fn comment_without_trailing_newline() {
    assert_eq!(
        kinds("x # done"),
        vec![K::Ident("x".into()), K::Newline, K::Eof]
    );
}

#[test]
fn hash_inside_brackets_still_comments() {
    assert_eq!(
        content_kinds("f(  # why\n    1)"),
        vec![K::Ident("f".into()), K::LParen, K::Int(1), K::RParen]
    );
}

// ------------------------------------------------------- i64::MIN literals

/// `i64::MIN` has no positive counterpart, so a `-` directly in front of the
/// magnitude `9223372036854775808` becomes part of the literal.
#[test]
fn negative_int_min_is_one_literal() {
    assert_eq!(
        content_kinds("x = -9223372036854775808"),
        vec![K::Ident("x".to_string()), K::Eq, K::Int(i64::MIN),]
    );
    // The same magnitude in hex, and with `_` separators.
    assert_eq!(content_kinds("-0x8000000000000000"), vec![K::Int(i64::MIN)]);
    assert_eq!(
        content_kinds("-9_223_372_036_854_775_808"),
        vec![K::Int(i64::MIN)]
    );
}

/// The span of a folded literal covers the sign as well.
#[test]
fn negative_int_min_span_covers_the_sign() {
    let tokens = lex_ok("-9223372036854775808");
    let token = tokens
        .iter()
        .find(|t| matches!(t.kind, K::Int(_)))
        .expect("an int token");
    assert_eq!(token.span.start, 0);
    assert_eq!(token.span.end, 20);
}

/// Only that one magnitude folds, so every other literal keeps its shape and
/// `-2 ** 2` still parses as `-(2 ** 2)`.
#[test]
fn other_negative_literals_do_not_fold() {
    assert_eq!(content_kinds("-5"), vec![K::Minus, K::Int(5)]);
    assert_eq!(content_kinds("- 5"), vec![K::Minus, K::Int(5)]);
    assert_eq!(
        content_kinds("-2 ** 2"),
        vec![K::Minus, K::Int(2), K::StarStar, K::Int(2)]
    );
    assert_eq!(content_kinds("-1.5"), vec![K::Minus, K::Float(1.5)]);
}

/// A `-` that follows an operand is subtraction, so the magnitude is still
/// out of range there.
#[test]
fn int_min_only_folds_after_a_prefix_minus() {
    for src in [
        "a - 9223372036854775808",
        "1 - 9223372036854775808",
        ") - 9223372036854775808",
    ] {
        let (_, diags) = lex(src);
        let messages = diags.iter().map(|d| d.message.clone()).collect::<Vec<_>>();
        assert!(
            messages
                .iter()
                .any(|m| m.contains("integer literal is too large")),
            "{src}: {messages:?}"
        );
    }
    // After an operator, a keyword or an opening bracket it does fold.
    for src in [
        "x = -9223372036854775808",
        "f(-9223372036854775808)",
        "[-9223372036854775808]",
        "return -9223372036854775808",
        "1 + -9223372036854775808",
    ] {
        let tokens = lex_ok(src);
        assert!(
            tokens.iter().any(|t| t.kind == K::Int(i64::MIN)),
            "{src}: {:?}",
            tokens.iter().map(|t| t.kind.clone()).collect::<Vec<_>>()
        );
    }
}
