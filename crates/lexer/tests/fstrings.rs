//! f-strings: literal parts, holes, format specs and their errors.

mod common;

use common::{content_kinds, content_kinds_lossy, dump, lex, lex_ok, messages};
use typhoon_lexer::{RawFStringPart, TokenKind as K};

/// The parts of the first f-string in `src`.
fn parts(src: &str) -> Vec<RawFStringPart> {
    lex(src)
        .0
        .into_iter()
        .find_map(|t| match t.kind {
            K::FString(parts) => Some(parts),
            _ => None,
        })
        .unwrap_or_else(|| panic!("no f-string in {src:?}"))
}

/// Asserts the invariant every hole must satisfy: `text` is the source slice
/// of `span`, and the same for the format spec.
#[track_caller]
fn assert_holes_slice_their_span(src: &str) {
    for token in lex(src).0 {
        let K::FString(parts) = token.kind else {
            continue;
        };
        for part in parts {
            if let RawFStringPart::Expr {
                text,
                span,
                spec,
                spec_span,
                full_span,
            } = part
            {
                assert_eq!(&src[span.range()], text, "hole text of {src:?}");
                assert!(full_span.start <= span.start && span.end <= full_span.end);
                assert_eq!(&src[full_span.range()][..1], "{");
                match (spec, spec_span) {
                    (Some(spec), Some(spec_span)) => {
                        assert_eq!(&src[spec_span.range()], spec, "spec of {src:?}");
                    }
                    (None, None) => {}
                    other => panic!("spec and spec_span disagree: {other:?}"),
                }
            }
        }
    }
}

// -------------------------------------------------------------------- basics

#[test]
fn f_string_without_holes() {
    assert_eq!(
        parts(r#"f"hi""#),
        vec![RawFStringPart::Literal {
            value: "hi".into(),
            span: typhoon_diag::Span::new(common::F, 2, 4),
        }]
    );
}

#[test]
fn empty_f_string_has_no_parts() {
    assert_eq!(parts(r#"f"""#), vec![]);
    assert!(messages(r#"f"""#).is_empty());
}

#[test]
fn single_quoted_f_string() {
    assert_eq!(
        content_kinds("f'hi'"),
        vec![K::FString(vec![RawFStringPart::Literal {
            value: "hi".into(),
            span: typhoon_diag::Span::new(common::F, 2, 4),
        }])]
    );
}

#[test]
fn f_string_token_span_covers_the_prefix_and_both_quotes() {
    let tokens = lex_ok(r#"x = f"hi""#);
    let fstring = tokens
        .iter()
        .find(|t| matches!(t.kind, K::FString(_)))
        .expect("f-string");
    assert_eq!((fstring.span.start, fstring.span.end), (4, 9));
}

#[test]
fn only_a_lowercase_f_directly_before_the_quote_is_a_prefix() {
    assert_eq!(
        content_kinds("f = 1"),
        vec![K::Ident("f".into()), K::Eq, K::Int(1)]
    );
    assert_eq!(
        content_kinds(r#"f "x""#),
        vec![K::Ident("f".into()), K::Str("x".into())]
    );
    assert_eq!(
        content_kinds(r#"F"x""#),
        vec![K::Ident("F".into()), K::Str("x".into())]
    );
    assert_eq!(
        content_kinds(r#"r"x""#),
        vec![K::Ident("r".into()), K::Str("x".into())]
    );
    assert_eq!(
        content_kinds(r#"rf"x""#),
        vec![K::Ident("rf".into()), K::Str("x".into())]
    );
}

#[test]
fn f_string_inside_a_call() {
    assert_eq!(content_kinds_lossy(r#"print(f"a{b}c")"#).len(), 4);
    assert_holes_slice_their_span(r#"print(f"a{b}c")"#);
}

// --------------------------------------------------------------------- holes

#[test]
fn a_single_hole() {
    let src = r#"f"{x}""#;
    assert_eq!(
        parts(src),
        vec![RawFStringPart::Expr {
            text: "x".into(),
            span: typhoon_diag::Span::new(common::F, 3, 4),
            spec: None,
            spec_span: None,
            full_span: typhoon_diag::Span::new(common::F, 2, 5),
        }]
    );
    assert_holes_slice_their_span(src);
}

#[test]
fn literal_hole_literal() {
    let src = r#"f"a{x}b""#;
    let parts = parts(src);
    assert_eq!(parts.len(), 3);
    assert!(matches!(&parts[0], RawFStringPart::Literal { value, .. } if value == "a"));
    assert!(matches!(&parts[1], RawFStringPart::Expr { text, .. } if text == "x"));
    assert!(matches!(&parts[2], RawFStringPart::Literal { value, .. } if value == "b"));
    assert_holes_slice_their_span(src);
}

#[test]
fn several_holes() {
    let src = r#"f"{a}{b} {c}""#;
    let holes: Vec<_> = parts(src)
        .into_iter()
        .filter_map(|p| match p {
            RawFStringPart::Expr { text, .. } => Some(text),
            RawFStringPart::Literal { .. } => None,
        })
        .collect();
    assert_eq!(holes, vec!["a", "b", "c"]);
    assert_holes_slice_their_span(src);
}

#[test]
fn a_hole_can_hold_a_whole_expression() {
    for src in [
        r#"f"{a + b * 2}""#,
        r#"f"{f(x, y)}""#,
        r#"f"{xs[0]}""#,
        r#"f"{p.dist()}""#,
        r#"f"{ x }""#,
        r#"f"{a if b else c}""#,
    ] {
        assert!(
            messages(src).is_empty(),
            "{src:?} should lex cleanly: {:?}",
            messages(src)
        );
        assert_holes_slice_their_span(src);
    }
}

#[test]
fn hole_text_is_the_exact_source_slice() {
    let src = r#"x = f"{ a + b }""#;
    let parts = parts(src);
    let RawFStringPart::Expr { text, span, .. } = &parts[0] else {
        panic!("expected a hole")
    };
    // The span is trimmed together with the text, so the invariant holds.
    assert_eq!(text, "a + b");
    assert_eq!(&src[span.range()], "a + b");
    assert_eq!(span.start, 8);
}

// ---------------------------------------------------------------- format spec

#[test]
fn hole_with_a_format_spec() {
    let src = r#"f"{x:.2f}""#;
    let parts = parts(src);
    let RawFStringPart::Expr {
        text,
        spec,
        spec_span,
        full_span,
        ..
    } = &parts[0]
    else {
        panic!("expected a hole")
    };
    assert_eq!(text, "x");
    assert_eq!(spec.as_deref(), Some(".2f"));
    assert_eq!(&src[spec_span.expect("spec span").range()], ".2f");
    assert_eq!(&src[full_span.range()], "{x:.2f}");
    assert_holes_slice_their_span(src);
}

#[test]
fn only_the_first_colon_at_depth_zero_starts_the_spec() {
    let src = r#"f"{x:>:5}""#;
    let parts = parts(src);
    let RawFStringPart::Expr { text, spec, .. } = &parts[0] else {
        panic!("expected a hole")
    };
    assert_eq!(text, "x");
    assert_eq!(spec.as_deref(), Some(">:5"));
}

#[test]
fn a_colon_inside_brackets_is_not_a_spec() {
    let src = r#"f"{d[1:2]}""#;
    let parts = parts(src);
    let RawFStringPart::Expr { text, spec, .. } = &parts[0] else {
        panic!("expected a hole")
    };
    assert_eq!(text, "d[1:2]");
    assert_eq!(spec.as_deref(), None);
}

#[test]
fn a_colon_inside_a_dict_literal_is_not_a_spec() {
    let src = r#"f"{ {1: 2} }""#;
    let parts = parts(src);
    let RawFStringPart::Expr { text, spec, .. } = &parts[0] else {
        panic!("expected a hole")
    };
    assert_eq!(text, "{1: 2}");
    assert_eq!(spec.as_deref(), None);
    assert!(messages(src).is_empty());
}

#[test]
fn a_spec_may_contain_braces() {
    let src = r#"f"{x:>{w}}""#;
    let parts = parts(src);
    let RawFStringPart::Expr { text, spec, .. } = &parts[0] else {
        panic!("expected a hole")
    };
    assert_eq!(text, "x");
    assert_eq!(spec.as_deref(), Some(">{w}"));
    assert_holes_slice_their_span(src);
}

#[test]
fn an_empty_spec_is_kept() {
    let parts = parts(r#"f"{x:}""#);
    let RawFStringPart::Expr { spec, .. } = &parts[0] else {
        panic!("expected a hole")
    };
    assert_eq!(spec.as_deref(), Some(""));
}

// -------------------------------------------------------------- escaped braces

#[test]
fn doubled_braces_decode_to_single_ones() {
    let parts = parts(r#"f"{{}}""#);
    assert_eq!(parts.len(), 1);
    assert!(matches!(&parts[0], RawFStringPart::Literal { value, .. } if value == "{}"));
    assert!(messages(r#"f"{{}}""#).is_empty());
}

#[test]
fn doubled_braces_around_a_hole() {
    let src = r#"f"{{{x}}}""#;
    let values: Vec<String> = parts(src)
        .into_iter()
        .map(|p| match p {
            RawFStringPart::Literal { value, .. } => format!("lit {value}"),
            RawFStringPart::Expr { text, .. } => format!("hole {text}"),
        })
        .collect();
    assert_eq!(values, vec!["lit {", "hole x", "lit }"]);
    assert!(messages(src).is_empty());
}

#[test]
fn escapes_are_decoded_in_literal_parts() {
    let parts = parts(r#"f"a\nb\tc\u{21}""#);
    assert!(matches!(&parts[0], RawFStringPart::Literal { value, .. } if value == "a\nb\tc!"));
}

#[test]
fn an_escaped_quote_does_not_end_the_f_string() {
    let parts = parts(r#"f"a\"b""#);
    assert!(matches!(&parts[0], RawFStringPart::Literal { value, .. } if value == "a\"b"));
    assert!(messages(r#"f"a\"b""#).is_empty());
}

// --------------------------------------------------------- quotes inside holes

#[test]
fn the_other_quote_kind_inside_a_hole_is_fine() {
    let src = r#"f"{d['a']}""#;
    let parts = parts(src);
    let RawFStringPart::Expr { text, .. } = &parts[0] else {
        panic!("expected a hole")
    };
    assert_eq!(text, "d['a']");
    assert!(messages(src).is_empty());
    assert_holes_slice_their_span(src);
}

#[test]
fn a_brace_inside_a_nested_string_does_not_close_the_hole() {
    let src = r#"f"{d['}']}!""#;
    let parts = parts(src);
    let RawFStringPart::Expr { text, .. } = &parts[0] else {
        panic!("expected a hole")
    };
    assert_eq!(text, "d['}']");
    assert!(messages(src).is_empty());
    assert_holes_slice_their_span(src);
}

#[test]
fn a_colon_inside_a_nested_string_is_not_a_spec() {
    let src = r#"f"{d[':']}""#;
    let parts = parts(src);
    let RawFStringPart::Expr { text, spec, .. } = &parts[0] else {
        panic!("expected a hole")
    };
    assert_eq!(text, "d[':']");
    assert_eq!(spec, &None);
}

#[test]
fn nested_quotes_of_the_same_kind_are_rejected() {
    let src = "x = f\"{d[\"a\"]}\"\ny = 2\n";
    let (tokens, diags) = lex(src);
    let messages: Vec<_> = diags.iter().map(|d| d.message.clone()).collect();
    assert!(
        messages
            .iter()
            .any(|m| m.contains("nested quotes of the same kind are not supported")),
        "{messages:?}"
    );
    let help = diags
        .iter()
        .flat_map(|d| d.help.clone())
        .collect::<Vec<_>>()
        .join(" ");
    assert!(help.contains("different quote style"), "{help}");
    // Lexing keeps going: the next line is still tokenized.
    let kinds: Vec<_> = tokens.into_iter().map(|t| t.kind).collect();
    assert!(kinds.contains(&K::Ident("y".into())), "{kinds:?}");
}

// -------------------------------------------------------------------- errors

#[test]
fn a_single_closing_brace_is_an_error() {
    let src = "x = f\"a}b\"\ny = 2\n";
    let (tokens, diags) = lex(src);
    let messages: Vec<_> = diags.iter().map(|d| d.message.clone()).collect();
    assert_eq!(messages, vec!["single `}` is not allowed in an f-string"]);
    let help = diags
        .iter()
        .flat_map(|d| d.help.clone())
        .collect::<Vec<_>>()
        .join(" ");
    assert!(help.contains("`}}`"), "{help}");
    let kinds: Vec<_> = tokens.into_iter().map(|t| t.kind).collect();
    assert!(kinds.contains(&K::Ident("y".into())), "{kinds:?}");
}

#[test]
fn an_empty_hole_is_an_error() {
    for src in [r#"f"{}""#, r#"f"{ }""#, "f\"{\t}\""] {
        let messages = messages(src);
        assert!(
            messages
                .iter()
                .any(|m| m.contains("empty expression in f-string")),
            "{src:?}: {messages:?}"
        );
        // The hole is dropped, the rest of the string survives.
        assert!(
            parts(src)
                .iter()
                .all(|p| matches!(p, RawFStringPart::Literal { .. }))
        );
    }
}

#[test]
fn an_empty_hole_does_not_stop_the_line() {
    let (tokens, _) = lex("x = f\"{}\"\ny = 2\n");
    let kinds: Vec<_> = tokens.into_iter().map(|t| t.kind).collect();
    assert!(kinds.contains(&K::Ident("y".into())), "{kinds:?}");
}

#[test]
fn an_unterminated_hole_is_an_error() {
    for src in ["x = f\"{a\ny = 2\n", "x = f\"{a"] {
        let (tokens, diags) = lex(src);
        let messages: Vec<_> = diags.iter().map(|d| d.message.clone()).collect();
        assert!(
            messages
                .iter()
                .any(|m| m.contains("unterminated f-string hole")),
            "{src:?}: {messages:?}"
        );
        assert_eq!(tokens.last().map(|t| t.kind.clone()), Some(K::Eof));
    }
}

#[test]
fn an_unterminated_hole_points_at_the_opening_brace() {
    let src = "f\"{a\n";
    let (_, diags) = lex(src);
    let diag = diags.iter().next().expect("a diagnostic");
    let span = diag.span().expect("a label");
    assert_eq!(&src[span.range()], "{");
}

#[test]
fn an_unbalanced_brace_inside_a_hole_is_an_error() {
    let messages = messages("x = f\"{a{b}\n");
    assert!(
        messages
            .iter()
            .any(|m| m.contains("unterminated f-string hole")),
        "{messages:?}"
    );
}

#[test]
fn an_unbalanced_brace_closed_by_the_quote_reports_the_quote() {
    // `f"{a{b}"` runs out of braces exactly at the closing quote; the lexer
    // cannot tell that apart from a same-kind nested quote, and says so with a
    // secondary label on the `{` that is still open.
    let src = "f\"{a{b}\"\n";
    let (_, diags) = lex(src);
    let diag = diags.iter().next().expect("a diagnostic");
    assert!(
        diag.message.contains("nested quotes of the same kind"),
        "{:?}",
        diag.message
    );
    let secondary = diag
        .labels
        .iter()
        .find(|l| !l.primary)
        .expect("a secondary label");
    assert_eq!(&src[secondary.span.range()], "{");
}

#[test]
fn an_unterminated_f_string_is_an_error() {
    let src = "x = f\"abc\ny = 2\n";
    let (tokens, diags) = lex(src);
    let messages: Vec<_> = diags.iter().map(|d| d.message.clone()).collect();
    assert_eq!(messages, vec!["unterminated f-string literal"]);
    let span = diags.iter().next().expect("one").span().expect("a label");
    assert_eq!(&src[span.range()], "\"");
    let kinds: Vec<_> = tokens.into_iter().map(|t| t.kind).collect();
    assert!(kinds.contains(&K::Ident("y".into())), "{kinds:?}");
}

#[test]
fn an_unterminated_nested_string_inside_a_hole_is_an_error() {
    let messages = messages("f\"{d['a}\"\n");
    assert!(!messages.is_empty(), "expected a diagnostic");
}

// ---------------------------------------------------------------------- dump

#[test]
fn dump_renders_parts_on_continuation_lines() {
    assert_eq!(
        dump(r#"f"i={i} {x:.2f}""#),
        "FString @0..16\n    \
         Literal \"i=\" @2..4\n    \
         Expr \"i\" @5..6 full @4..7\n    \
         Literal \" \" @7..8\n    \
         Expr \"x\" @9..10 spec \".2f\" @11..14 full @8..15\n\
         Newline @16..16\n\
         Eof @16..16"
    );
}
