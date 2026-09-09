//! Snapshot tests for the rustc-style diagnostic renderer.

use typhoon_diag::{Diagnostic, FileId, SourceMap, Span, render, render_all};

const SRC: &str = "\
fn main():
    total = 0
    for i in range(10):
        total = total + i
    print(totl)
";

fn sources() -> (SourceMap, FileId) {
    let mut sm = SourceMap::new();
    let f = sm.add("main.ty", SRC);
    (sm, f)
}

/// Byte offset of the first occurrence of `needle` in the fixture.
fn at(needle: &str) -> (u32, u32) {
    let start = SRC.find(needle).expect("needle not in fixture");
    (start as u32, (start + needle.len()) as u32)
}

fn span(file: FileId, needle: &str) -> Span {
    let (s, e) = at(needle);
    Span::new(file, s, e)
}

#[test]
fn single_primary_label() {
    let (sm, f) = sources();
    let d = Diagnostic::error("cannot find value `totl` in this scope")
        .with_label(span(f, "totl"), "not found in this scope");
    insta::assert_snapshot!(render(&d, &sm, false));
}

#[test]
fn primary_and_secondary_on_different_lines() {
    let (sm, f) = sources();
    let d = Diagnostic::error("cannot find value `totl` in this scope")
        .with_label(span(f, "totl"), "not found in this scope")
        .with_secondary(span(f, "total = 0"), "a similar name is declared here");
    insta::assert_snapshot!(render(&d, &sm, false));
}

#[test]
fn with_note_and_help() {
    let (sm, f) = sources();
    let d = Diagnostic::error("cannot find value `totl` in this scope")
        .with_code("E0425")
        .with_label(span(f, "totl"), "not found in this scope")
        .with_secondary(span(f, "total = 0"), "a similar name is declared here")
        .with_note("names are resolved per function")
        .with_help("did you mean `total`?");
    insta::assert_snapshot!(render(&d, &sm, false));
}

#[test]
fn warning_level() {
    let (sm, f) = sources();
    let d = Diagnostic::warning("unused variable `i`").with_label(span(f, "i in"), "never read");
    insta::assert_snapshot!(render(&d, &sm, false));
}

#[test]
fn label_without_message() {
    let (sm, f) = sources();
    let d = Diagnostic::error("something is wrong here").with_label(span(f, "range(10)"), "");
    insta::assert_snapshot!(render(&d, &sm, false));
}

#[test]
fn diagnostic_without_labels() {
    let (sm, _) = sources();
    let d = Diagnostic::error("no `fn main()` found in `main.ty`")
        .with_help("every program needs an entry point");
    insta::assert_snapshot!(render(&d, &sm, false));
}

#[test]
fn multi_line_span() {
    let (sm, f) = sources();
    let d = Diagnostic::error("this loop never terminates").with_label(
        span(f, "for i in range(10):\n        total = total + i"),
        "loop body",
    );
    insta::assert_snapshot!(render(&d, &sm, false));
}

#[test]
fn labels_in_two_files() {
    let mut sm = SourceMap::new();
    let a = sm.add("a.ty", "fn helper() -> int:\n    return 1\n");
    let b = sm.add("b.ty", "fn main():\n    helper(1)\n");
    let d = Diagnostic::error("this function takes 0 arguments but 1 was supplied")
        .with_label(Span::new(b, 22, 23), "unexpected argument")
        .with_secondary(Span::new(a, 0, 18), "function defined here");
    insta::assert_snapshot!(render(&d, &sm, false));
}

#[test]
fn utf8_source_underlines_the_right_columns() {
    let mut sm = SourceMap::new();
    let text = "fn main():\n    s = \"héllo wörld\"\n";
    let f = sm.add("utf8.ty", text);
    let start = text.find("wörld").unwrap() as u32;
    let d = Diagnostic::error("unknown word")
        .with_label(Span::new(f, start, start + "wörld".len() as u32), "here");
    insta::assert_snapshot!(render(&d, &sm, false));
}

#[test]
fn empty_span_at_end_of_file() {
    let (sm, f) = sources();
    let end = SRC.len() as u32;
    let d = Diagnostic::error("unexpected end of file")
        .with_label(Span::new(f, end, end), "expected a statement");
    insta::assert_snapshot!(render(&d, &sm, false));
}

#[test]
fn out_of_range_span_is_clamped() {
    let (sm, f) = sources();
    let d = Diagnostic::error("bogus span").with_label(Span::new(f, 10_000, 20_000), "here");
    // Must not panic; the span is clamped to the end of the file.
    let out = render(&d, &sm, false);
    assert!(out.starts_with("error: bogus span"), "{out}");
}

#[test]
fn dummy_span_renders_only_the_title() {
    let (sm, _) = sources();
    let d = Diagnostic::error("synthesized").with_label(Span::dummy(), "nowhere");
    assert_eq!(render(&d, &sm, false), "error: synthesized");
}

#[test]
fn render_all_separates_diagnostics_with_a_blank_line() {
    let (sm, f) = sources();
    let first = Diagnostic::error("first problem").with_label(span(f, "total = 0"), "here");
    let second = Diagnostic::error("second problem").with_label(span(f, "totl"), "and here");
    insta::assert_snapshot!(render_all([&first, &second], &sm, false));
}

#[test]
fn render_all_of_nothing_is_empty() {
    let (sm, _) = sources();
    assert_eq!(render_all([], &sm, false), "");
}

#[test]
fn color_output_contains_ansi_escapes() {
    let (sm, f) = sources();
    let d = Diagnostic::error("boom").with_label(span(f, "totl"), "here");
    let colored = render(&d, &sm, true);
    assert!(
        colored.contains('\u{1b}'),
        "expected ANSI escapes in {colored:?}"
    );
    let plain = render(&d, &sm, false);
    assert!(!plain.contains('\u{1b}'));
}
