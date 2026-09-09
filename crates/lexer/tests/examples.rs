//! Every file in `examples/` must lex cleanly, and its token stream is
//! snapshotted so that a change to the lexer shows up as a reviewable diff.

use std::fs;

use typhoon_diag::{Diagnostics, FileId};
use typhoon_lexer::{RawFStringPart, TokenKind, dump_tokens, tokenize};

/// The `examples/` directory of the repository.
const EXAMPLES: &str = concat!(env!("CARGO_MANIFEST_DIR"), "/../../examples");

/// How many `.ty` files `examples/` is expected to contain; a guard against a
/// silently empty test.
const EXPECTED_COUNT: usize = 11;

/// Every example, as `(file stem, text)`, sorted by name.
fn examples() -> Vec<(String, String)> {
    let mut out = Vec::new();
    for entry in fs::read_dir(EXAMPLES).expect("examples directory is readable") {
        let path = entry.expect("directory entry").path();
        if path.extension().and_then(|e| e.to_str()) != Some("ty") {
            continue;
        }
        let stem = path
            .file_stem()
            .and_then(|s| s.to_str())
            .expect("example file name is utf-8")
            .to_string();
        let text = fs::read_to_string(&path).expect("example file is readable");
        out.push((stem, text));
    }
    out.sort();
    assert_eq!(
        out.len(),
        EXPECTED_COUNT,
        "unexpected number of examples: {out:?}"
    );
    out
}

#[test]
fn every_example_lexes_without_diagnostics() {
    for (stem, text) in examples() {
        let mut diags = Diagnostics::new();
        tokenize(FileId(0), &text, &mut diags);
        let messages: Vec<_> = diags.iter().map(|d| d.message.clone()).collect();
        assert!(
            messages.is_empty(),
            "examples/{stem}.ty produced diagnostics: {messages:?}"
        );
    }
}

#[test]
fn every_example_has_a_balanced_and_terminated_stream() {
    for (stem, text) in examples() {
        let mut diags = Diagnostics::new();
        let tokens = tokenize(FileId(0), &text, &mut diags);
        let indents = tokens
            .iter()
            .filter(|t| t.kind == TokenKind::Indent)
            .count();
        let dedents = tokens
            .iter()
            .filter(|t| t.kind == TokenKind::Dedent)
            .count();
        assert_eq!(
            indents, dedents,
            "examples/{stem}.ty has unbalanced layout tokens"
        );
        assert_eq!(
            tokens.last().map(|t| t.kind.clone()),
            Some(TokenKind::Eof),
            "examples/{stem}.ty does not end with `Eof`"
        );
        assert!(
            tokens.iter().any(|t| t.kind == TokenKind::Fn),
            "examples/{stem}.ty has no `fn`"
        );
        for token in &tokens {
            assert!(
                token.span.end as usize <= text.len(),
                "examples/{stem}.ty: {token:?} points past the end of the file"
            );
            if matches!(
                token.kind,
                TokenKind::Indent | TokenKind::Dedent | TokenKind::Eof
            ) {
                assert!(
                    token.span.is_empty(),
                    "examples/{stem}.ty: {token:?} is not empty"
                );
            }
        }
    }
}

#[test]
fn every_f_string_hole_of_every_example_slices_its_span() {
    let mut holes = 0;
    for (stem, text) in examples() {
        let mut diags = Diagnostics::new();
        for token in tokenize(FileId(0), &text, &mut diags) {
            let TokenKind::FString(parts) = token.kind else {
                continue;
            };
            for part in parts {
                match part {
                    RawFStringPart::Expr {
                        text: hole,
                        span,
                        spec,
                        spec_span,
                        full_span,
                    } => {
                        holes += 1;
                        assert_eq!(&text[span.range()], hole, "in examples/{stem}.ty");
                        assert!(text[full_span.range()].starts_with('{'));
                        assert!(text[full_span.range()].ends_with('}'));
                        if let (Some(spec), Some(spec_span)) = (spec, spec_span) {
                            assert_eq!(&text[spec_span.range()], spec, "in examples/{stem}.ty");
                        }
                    }
                    RawFStringPart::Literal { value, span } => {
                        assert!(
                            !value.is_empty(),
                            "empty literal part in examples/{stem}.ty"
                        );
                        assert!(!span.is_empty(), "empty literal span in examples/{stem}.ty");
                    }
                }
            }
        }
    }
    assert!(
        holes >= 10,
        "expected the examples to exercise f-string holes, found {holes}"
    );
}

#[test]
fn token_dumps() {
    for (stem, text) in examples() {
        let mut diags = Diagnostics::new();
        let tokens = tokenize(FileId(0), &text, &mut diags);
        insta::assert_snapshot!(stem.as_str(), dump_tokens(&tokens));
    }
}
