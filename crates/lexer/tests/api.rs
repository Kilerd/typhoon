//! The public API: `tokenize`, `tokenize_fragment`, `dump_tokens`, and the
//! guarantees the parser relies on.

mod common;

use common::{F, dump, lex};
use typhoon_diag::{Diagnostics, Span};
use typhoon_lexer::{
    RawFStringPart, Token, TokenKind as K, dump_tokens, tokenize, tokenize_fragment,
};

const SAMPLE: &str = "fn f(a: int) -> int:\n    # a comment\n    return a * 2 + 0x10\n";

#[test]
fn tokenize_is_tokenize_fragment_with_a_zero_offset() {
    let mut a = Diagnostics::new();
    let mut b = Diagnostics::new();
    let from_tokenize = tokenize(F, SAMPLE, &mut a);
    let from_fragment = tokenize_fragment(F, SAMPLE, 0, &mut b);
    assert_eq!(from_tokenize, from_fragment);
    assert_eq!(a, b);
}

#[test]
fn every_stream_ends_with_exactly_one_eof() {
    for src in ["", "\n", "x", SAMPLE, "if a:\n    b", "@", "\"oops"] {
        let (tokens, _) = lex(src);
        assert_eq!(
            tokens.last().map(|t| t.kind.clone()),
            Some(K::Eof),
            "{src:?}"
        );
        assert_eq!(
            tokens.iter().filter(|t| t.kind == K::Eof).count(),
            1,
            "{src:?}"
        );
        let eof = tokens.last().expect("non empty");
        assert_eq!(
            eof.span,
            Span::new(F, src.len() as u32, src.len() as u32),
            "{src:?}"
        );
    }
}

#[test]
fn token_spans_are_ordered_and_inside_the_file() {
    let (tokens, _) = lex(SAMPLE);
    let mut previous = 0;
    for token in &tokens {
        assert!(token.span.start >= previous, "{token:?} moves backwards");
        assert!(
            token.span.end as usize <= SAMPLE.len(),
            "{token:?} points past the end"
        );
        previous = token.span.start;
    }
}

#[test]
fn non_layout_token_spans_slice_the_source() {
    let (tokens, _) = lex(SAMPLE);
    for token in &tokens {
        if let Some(text) = token.kind.fixed_text() {
            assert_eq!(&SAMPLE[token.span.range()], text, "{token:?}");
        }
        if let K::Ident(name) = &token.kind {
            assert_eq!(&SAMPLE[token.span.range()], name, "{token:?}");
        }
    }
}

#[test]
fn fragment_shifts_token_spans() {
    let src = "a + 1";
    let base = 1000;
    let mut diags = Diagnostics::new();
    let plain = tokenize(F, src, &mut diags);
    let shifted = tokenize_fragment(F, src, base, &mut diags);
    assert_eq!(plain.len(), shifted.len());
    for (plain, shifted) in plain.iter().zip(&shifted) {
        assert_eq!(plain.kind, shifted.kind);
        assert_eq!(plain.span.start + base, shifted.span.start);
        assert_eq!(plain.span.end + base, shifted.span.end);
    }
}

#[test]
fn fragment_shifts_diagnostic_spans() {
    let src = "a @ 1";
    let base = 40;
    let mut plain = Diagnostics::new();
    let mut shifted = Diagnostics::new();
    tokenize(F, src, &mut plain);
    tokenize_fragment(F, src, base, &mut shifted);
    assert_eq!(plain.len(), 1);
    assert_eq!(shifted.len(), 1);
    let plain = plain.iter().next().expect("one").clone();
    let shifted = shifted.iter().next().expect("one").clone();
    assert_eq!(plain.message, shifted.message);
    for (plain, shifted) in plain.labels.iter().zip(&shifted.labels) {
        assert_eq!(plain.span.start + base, shifted.span.start);
        assert_eq!(plain.span.end + base, shifted.span.end);
    }
}

#[test]
fn fragment_shifts_f_string_part_spans() {
    let src = r#"f"a{b:c}""#;
    let base = 7;
    let mut diags = Diagnostics::new();
    let plain = tokenize(F, src, &mut diags);
    let shifted = tokenize_fragment(F, src, base, &mut diags);
    let parts = |tokens: Vec<Token>| {
        tokens
            .into_iter()
            .find_map(|t| match t.kind {
                K::FString(parts) => Some(parts),
                _ => None,
            })
            .expect("an f-string")
    };
    for (plain, shifted) in parts(plain).into_iter().zip(parts(shifted)) {
        match (plain, shifted) {
            (RawFStringPart::Literal { span: a, .. }, RawFStringPart::Literal { span: b, .. }) => {
                assert_eq!(a.start + base, b.start)
            }
            (
                RawFStringPart::Expr {
                    span: a,
                    spec_span: sa,
                    full_span: fa,
                    ..
                },
                RawFStringPart::Expr {
                    span: b,
                    spec_span: sb,
                    full_span: fb,
                    ..
                },
            ) => {
                assert_eq!(a.start + base, b.start);
                assert_eq!(a.end + base, b.end);
                assert_eq!(fa.start + base, fb.start);
                assert_eq!(
                    sa.expect("spec span").start + base,
                    sb.expect("spec span").start
                );
            }
            other => panic!("parts do not line up: {other:?}"),
        }
    }
}

/// The way the parser is meant to use `tokenize_fragment`: re-lex the raw text
/// of a hole and get spans that point back into the original file.
#[test]
fn re_lexing_a_hole_points_back_into_the_file() {
    let src = "print(f\"{ x + 1 :.2f}\")\n";
    let (tokens, diags) = lex(src);
    assert!(
        diags.is_empty(),
        "{:?}",
        diags.iter().map(|d| &d.message).collect::<Vec<_>>()
    );
    let parts = tokens
        .into_iter()
        .find_map(|t| match t.kind {
            K::FString(parts) => Some(parts),
            _ => None,
        })
        .expect("an f-string");
    let RawFStringPart::Expr { text, span, .. } = &parts[0] else {
        panic!("expected a hole")
    };
    assert_eq!(&src[span.range()], text);

    let mut hole_diags = Diagnostics::new();
    let hole = tokenize_fragment(F, text, span.start, &mut hole_diags);
    assert!(hole_diags.is_empty());
    assert_eq!(hole[0].kind, K::Ident("x".into()));
    assert_eq!(&src[hole[0].span.range()], "x");
    assert_eq!(hole[1].kind, K::Plus);
    assert_eq!(&src[hole[1].span.range()], "+");
    assert_eq!(hole[2].kind, K::Int(1));
    assert_eq!(&src[hole[2].span.range()], "1");
}

// ---------------------------------------------------------------- dump format

#[test]
fn dump_of_an_empty_stream_is_empty() {
    assert_eq!(dump_tokens(&[]), "");
}

#[test]
fn dump_has_no_trailing_newline() {
    let dumped = dump(SAMPLE);
    assert!(!dumped.ends_with('\n'));
    assert!(
        dumped.ends_with(&format!("Eof @{0}..{0}", SAMPLE.len())),
        "{dumped}"
    );
}

#[test]
fn dump_uses_debug_for_string_and_float_payloads() {
    assert_eq!(
        dump(r#""a\nb""#),
        "Str \"a\\nb\" @0..6\nNewline @6..6\nEof @6..6"
    );
    assert_eq!(dump("1.5"), "Float 1.5 @0..3\nNewline @3..3\nEof @3..3");
    assert_eq!(
        dump("1e10"),
        "Float 10000000000.0 @0..4\nNewline @4..4\nEof @4..4"
    );
    assert_eq!(dump("2.0"), "Float 2.0 @0..3\nNewline @3..3\nEof @3..3");
}

#[test]
fn dump_of_the_readme_example() {
    assert_eq!(
        dump("fn main():\n    print(42)\n"),
        "Fn @0..2\n\
         Ident `main` @3..7\n\
         LParen @7..8\n\
         RParen @8..9\n\
         Colon @9..10\n\
         Newline @10..11\n\
         Indent @15..15\n\
         Ident `print` @15..20\n\
         LParen @20..21\n\
         Int 42 @21..23\n\
         RParen @23..24\n\
         Newline @24..25\n\
         Dedent @25..25\n\
         Eof @25..25"
    );
}

// ------------------------------------------------------------- never panics

#[test]
fn nasty_inputs_terminate_without_panicking() {
    let nasty = [
        "",
        "\0",
        "\\",
        "\\\\",
        "{",
        "}",
        "f\"{",
        "f\"{{",
        "f'{a",
        "\"",
        "'",
        "f\"",
        "0x",
        "1e",
        ".",
        "..",
        "1.",
        "é",
        "\u{1f600}",
        "\"\u{1f600}",
        "f\"{\u{1f600}}\"",
        "x = \"\u{e9}\u{4e16}\"",
        "\r",
        "\r\r\r",
        "\n\r",
        "\t",
        " \t \t x",
        "((((((((((",
        "))))))))))",
        "#",
        "#\n#\n",
        "f\"{'}\"",
        "f\"{a:{b}\"",
        "0b2",
        "1_",
        "1__2",
        "1e+",
        "\u{feff}x",
    ];
    for src in nasty {
        let (tokens, _) = lex(src);
        assert_eq!(
            tokens.last().map(|t| t.kind.clone()),
            Some(K::Eof),
            "{src:?}"
        );
        // The dump must not panic either.
        let _ = dump_tokens(&tokens);
    }
}

#[test]
fn a_large_file_terminates() {
    let mut src = String::new();
    for i in 0..2_000 {
        src.push_str(&format!(
            "fn f{i}(a: int) -> int:\n    return a + {i} * 0x1f\n\n"
        ));
    }
    let (tokens, diags) = lex(&src);
    assert!(diags.is_empty());
    assert!(tokens.len() > 20_000);
}

#[test]
fn deeply_nested_f_string_holes_terminate() {
    let src = format!("f\"{}x{}\"", "{".repeat(64), "}".repeat(64));
    let (tokens, _) = lex(&src);
    assert_eq!(tokens.last().map(|t| t.kind.clone()), Some(K::Eof));
}
