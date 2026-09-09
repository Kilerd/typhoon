//! Layout: indentation, newlines, implicit continuation inside brackets.

mod common;

use common::{content_kinds, dump, kinds, kinds_lossy, lex, lex_ok, messages};
use typhoon_lexer::TokenKind as K;

/// `Indent`/`Dedent`/`Eof` spans must be empty, `Newline` spans must cover the
/// line break.
#[test]
fn layout_token_spans() {
    let tokens = lex_ok("if a:\n    b\n");
    let dump = dump("if a:\n    b\n");
    assert_eq!(
        dump,
        "If @0..2\n\
         Ident `a` @3..4\n\
         Colon @4..5\n\
         Newline @5..6\n\
         Indent @10..10\n\
         Ident `b` @10..11\n\
         Newline @11..12\n\
         Dedent @12..12\n\
         Eof @12..12"
    );
    for token in &tokens {
        if matches!(token.kind, K::Indent | K::Dedent | K::Eof) {
            assert!(token.span.is_empty(), "{token:?} must have an empty span");
        }
    }
}

#[test]
fn nested_blocks() {
    assert_eq!(
        kinds("fn f():\n    if x:\n        pass\n"),
        vec![
            K::Fn,
            K::Ident("f".into()),
            K::LParen,
            K::RParen,
            K::Colon,
            K::Newline,
            K::Indent,
            K::If,
            K::Ident("x".into()),
            K::Colon,
            K::Newline,
            K::Indent,
            K::Pass,
            K::Newline,
            K::Dedent,
            K::Dedent,
            K::Eof,
        ]
    );
}

#[test]
fn two_level_dedent_on_one_line() {
    assert_eq!(
        kinds("if a:\n    if b:\n        x\ny\n"),
        vec![
            K::If,
            K::Ident("a".into()),
            K::Colon,
            K::Newline,
            K::Indent,
            K::If,
            K::Ident("b".into()),
            K::Colon,
            K::Newline,
            K::Indent,
            K::Ident("x".into()),
            K::Newline,
            K::Dedent,
            K::Dedent,
            K::Ident("y".into()),
            K::Newline,
            K::Eof,
        ]
    );
}

#[test]
fn indent_and_dedent_are_always_balanced() {
    for src in [
        "if a:\n    x\n",
        "if a:\n    x",
        "if a:\n    if b:\n        if c:\n            x\n",
        "if a:\n        x\n    y\n",
        "if a:\n\tx\n",
        "if a:\n  x\n     y\n z\n",
    ] {
        let kinds = kinds_lossy(src);
        let indents = kinds.iter().filter(|k| **k == K::Indent).count();
        let dedents = kinds.iter().filter(|k| **k == K::Dedent).count();
        assert_eq!(indents, dedents, "unbalanced layout for {src:?}: {kinds:?}");
    }
}

#[test]
fn dedent_to_an_invalid_level_is_reported_and_recovered() {
    let src = "if a:\n        x\n    y\n";
    let (tokens, diags) = lex(src);
    assert_eq!(diags.len(), 1);
    let diag = diags.iter().next().expect("one diagnostic");
    assert!(
        diag.message
            .contains("unindent does not match any outer indentation level"),
        "{:?}",
        diag.message
    );
    // The label points at the first token of the offending line.
    let span = diag.span().expect("primary label");
    assert_eq!(&src[span.range()], "y");
    // Lexing continues and the block structure stays balanced.
    let kinds: Vec<_> = tokens.into_iter().map(|t| t.kind).collect();
    assert_eq!(
        kinds,
        vec![
            K::If,
            K::Ident("a".into()),
            K::Colon,
            K::Newline,
            K::Indent,
            K::Ident("x".into()),
            K::Newline,
            K::Dedent,
            K::Ident("y".into()),
            K::Newline,
            K::Eof,
        ]
    );
}

#[test]
fn bracket_continuation_ignores_line_breaks_and_indentation() {
    assert_eq!(
        kinds("xs = [\n        1,\n  2,\n]\n"),
        vec![
            K::Ident("xs".into()),
            K::Eq,
            K::LBracket,
            K::Int(1),
            K::Comma,
            K::Int(2),
            K::Comma,
            K::RBracket,
            K::Newline,
            K::Eof,
        ]
    );
}

#[test]
fn bracket_continuation_nests() {
    let src = "f(\n  g(\n    1,\n  ),\n)\n";
    let kinds = kinds(src);
    assert_eq!(kinds.iter().filter(|k| **k == K::Newline).count(), 1);
    assert!(!kinds.contains(&K::Indent));
    assert!(!kinds.contains(&K::Dedent));
}

#[test]
fn a_tab_inside_brackets_is_not_an_indentation_error() {
    assert!(messages("f(\n\t1,\n)\n").is_empty());
}

#[test]
fn block_resumes_after_a_bracketed_continuation() {
    assert_eq!(
        kinds("if a:\n    f(1,\n      2)\n    g()\n"),
        vec![
            K::If,
            K::Ident("a".into()),
            K::Colon,
            K::Newline,
            K::Indent,
            K::Ident("f".into()),
            K::LParen,
            K::Int(1),
            K::Comma,
            K::Int(2),
            K::RParen,
            K::Newline,
            K::Ident("g".into()),
            K::LParen,
            K::RParen,
            K::Newline,
            K::Dedent,
            K::Eof,
        ]
    );
}

#[test]
fn a_stray_closing_bracket_does_not_break_the_layout() {
    // `)` with nothing open must not make the depth counter wrap around.
    assert_eq!(
        kinds(")\nx\n"),
        vec![
            K::RParen,
            K::Newline,
            K::Ident("x".into()),
            K::Newline,
            K::Eof
        ]
    );
}

#[test]
fn comment_only_lines_inside_a_block_change_nothing() {
    assert_eq!(
        kinds("if a:\n    x\n    # note\n    y\n"),
        vec![
            K::If,
            K::Ident("a".into()),
            K::Colon,
            K::Newline,
            K::Indent,
            K::Ident("x".into()),
            K::Newline,
            K::Ident("y".into()),
            K::Newline,
            K::Dedent,
            K::Eof,
        ]
    );
}

#[test]
fn a_comment_at_column_zero_does_not_dedent() {
    assert_eq!(
        kinds("if a:\n    x\n# note\n    y\n"),
        vec![
            K::If,
            K::Ident("a".into()),
            K::Colon,
            K::Newline,
            K::Indent,
            K::Ident("x".into()),
            K::Newline,
            K::Ident("y".into()),
            K::Newline,
            K::Dedent,
            K::Eof,
        ]
    );
}

#[test]
fn a_deeper_comment_inside_a_block_does_not_indent() {
    assert_eq!(
        kinds("if a:\n    x\n        # deep\n    y\n"),
        vec![
            K::If,
            K::Ident("a".into()),
            K::Colon,
            K::Newline,
            K::Indent,
            K::Ident("x".into()),
            K::Newline,
            K::Ident("y".into()),
            K::Newline,
            K::Dedent,
            K::Eof,
        ]
    );
}

#[test]
fn a_deeper_indent_after_a_comment_line_still_indents() {
    assert_eq!(
        kinds("if a:\n# note\n    x\n"),
        vec![
            K::If,
            K::Ident("a".into()),
            K::Colon,
            K::Newline,
            K::Indent,
            K::Ident("x".into()),
            K::Newline,
            K::Dedent,
            K::Eof,
        ]
    );
}

#[test]
fn blank_lines_produce_nothing() {
    assert_eq!(kinds("\n\n\n"), vec![K::Eof]);
    assert_eq!(kinds("   \n\t\n"), vec![K::Eof]);
    assert_eq!(kinds(""), vec![K::Eof]);
}

#[test]
fn a_file_may_start_with_blank_lines() {
    let tokens = lex_ok("\n\n\nx\n");
    assert_eq!(tokens[0].kind, K::Ident("x".into()));
    assert_eq!(tokens[0].span.start, 3);
}

#[test]
fn blank_lines_inside_a_block_do_not_close_it() {
    assert_eq!(
        kinds("if a:\n    x\n\n    y\n"),
        vec![
            K::If,
            K::Ident("a".into()),
            K::Colon,
            K::Newline,
            K::Indent,
            K::Ident("x".into()),
            K::Newline,
            K::Ident("y".into()),
            K::Newline,
            K::Dedent,
            K::Eof,
        ]
    );
}

#[test]
fn trailing_whitespace_is_ignored() {
    let src = "x = 1   \ny = 2\n";
    let tokens = lex_ok(src);
    let newline = tokens
        .iter()
        .find(|t| t.kind == K::Newline)
        .expect("a newline");
    assert_eq!(&src[newline.span.range()], "\n");
    assert_eq!(newline.span.start, 8);
}

#[test]
fn a_line_with_only_whitespace_after_a_block_does_not_dedent() {
    assert_eq!(
        kinds("if a:\n    x\n   \n    y\n"),
        vec![
            K::If,
            K::Ident("a".into()),
            K::Colon,
            K::Newline,
            K::Indent,
            K::Ident("x".into()),
            K::Newline,
            K::Ident("y".into()),
            K::Newline,
            K::Dedent,
            K::Eof,
        ]
    );
}

#[test]
fn file_without_a_trailing_newline() {
    assert_eq!(
        dump("x = 1"),
        "Ident `x` @0..1\nEq @2..3\nInt 1 @4..5\nNewline @5..5\nEof @5..5"
    );
}

#[test]
fn open_block_without_a_trailing_newline() {
    assert_eq!(
        dump("if a:\n    x"),
        "If @0..2\n\
         Ident `a` @3..4\n\
         Colon @4..5\n\
         Newline @5..6\n\
         Indent @10..10\n\
         Ident `x` @10..11\n\
         Newline @11..11\n\
         Dedent @11..11\n\
         Eof @11..11"
    );
}

#[test]
fn eof_order_is_newline_then_dedents_then_eof() {
    let kinds = kinds("if a:\n    if b:\n        x");
    let tail = &kinds[kinds.len() - 4..];
    assert_eq!(tail, [K::Newline, K::Dedent, K::Dedent, K::Eof]);
}

#[test]
fn crlf_line_breaks() {
    let src = "fn f():\r\n    pass\r\n";
    let tokens = lex_ok(src);
    let newline = tokens
        .iter()
        .find(|t| t.kind == K::Newline)
        .expect("a newline");
    assert_eq!(&src[newline.span.range()], "\r\n");
    assert_eq!(
        kinds(src),
        vec![
            K::Fn,
            K::Ident("f".into()),
            K::LParen,
            K::RParen,
            K::Colon,
            K::Newline,
            K::Indent,
            K::Pass,
            K::Newline,
            K::Dedent,
            K::Eof,
        ]
    );
}

#[test]
fn crlf_blank_lines_and_comments() {
    assert_eq!(
        kinds("\r\n# c\r\nx\r\n"),
        vec![K::Ident("x".into()), K::Newline, K::Eof]
    );
}

#[test]
fn a_lone_carriage_return_is_ordinary_whitespace() {
    assert_eq!(
        content_kinds("a\rb"),
        vec![K::Ident("a".into()), K::Ident("b".into())]
    );
    // At the start of a line a lone `\r` is skipped and does not indent.
    assert_eq!(
        kinds("a\n\rb\n"),
        vec![
            K::Ident("a".into()),
            K::Newline,
            K::Ident("b".into()),
            K::Newline,
            K::Eof
        ]
    );
}

#[test]
fn tabs_in_indentation_are_reported_once_per_line() {
    let src = "if a:\n\tx\n\ty\n";
    let (tokens, diags) = lex(src);
    let messages = diags.iter().map(|d| d.message.clone()).collect::<Vec<_>>();
    assert_eq!(
        messages.len(),
        2,
        "one diagnostic per offending line: {messages:?}"
    );
    for message in &messages {
        assert!(
            message.contains("tabs are not allowed in indentation"),
            "{message}"
        );
    }
    let help = diags.iter().next().expect("one").help.join(" ");
    assert!(help.contains("4 spaces"), "{help}");
    // A tab counts as one space, so both lines sit at column 1.
    let kinds: Vec<_> = tokens.into_iter().map(|t| t.kind).collect();
    assert_eq!(
        kinds,
        vec![
            K::If,
            K::Ident("a".into()),
            K::Colon,
            K::Newline,
            K::Indent,
            K::Ident("x".into()),
            K::Newline,
            K::Ident("y".into()),
            K::Newline,
            K::Dedent,
            K::Eof,
        ]
    );
}

#[test]
fn a_tab_on_a_blank_line_is_not_reported() {
    assert!(messages("   \n\t\n\t# comment\nx\n").is_empty());
}

#[test]
fn a_tab_after_the_first_token_is_not_an_indentation_error() {
    assert!(messages("x\t= 1\n").is_empty());
}

#[test]
fn a_line_of_only_invalid_characters_is_ignored_by_the_layout() {
    // `@` produces no token, so the line neither opens nor closes a block.
    let (tokens, diags) = lex("if a:\n    x\n@\n    y\n");
    assert_eq!(diags.len(), 1);
    let kinds: Vec<_> = tokens.into_iter().map(|t| t.kind).collect();
    assert_eq!(
        kinds,
        vec![
            K::If,
            K::Ident("a".into()),
            K::Colon,
            K::Newline,
            K::Indent,
            K::Ident("x".into()),
            K::Newline,
            K::Ident("y".into()),
            K::Newline,
            K::Dedent,
            K::Eof,
        ]
    );
}

#[test]
fn deeply_nested_blocks_terminate() {
    let mut src = String::new();
    for depth in 0..200 {
        src.push_str(&" ".repeat(depth * 2));
        src.push_str("if a:\n");
    }
    src.push_str(&" ".repeat(400));
    src.push_str("x\n");
    let kinds = kinds_lossy(&src);
    assert_eq!(kinds.iter().filter(|k| **k == K::Indent).count(), 200);
    assert_eq!(kinds.iter().filter(|k| **k == K::Dedent).count(), 200);
}

#[test]
fn deeply_nested_brackets_terminate() {
    let src = format!("x = {}{}\n", "(".repeat(500), ")".repeat(500));
    let kinds = kinds(&src);
    assert_eq!(kinds.iter().filter(|k| **k == K::LParen).count(), 500);
}
