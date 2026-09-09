//! Snapshots of synthetic sources: the whole token vocabulary, layout corner
//! cases, f-strings, and the rendered diagnostics of a file full of mistakes.

use typhoon_diag::{Diagnostics, SourceMap, render_all};
use typhoon_lexer::{dump_tokens, tokenize};

/// Lexes `src` as `main.ty` and returns the token dump followed by the
/// rendered diagnostics.
fn report(src: &str) -> String {
    let mut sources = SourceMap::new();
    let file = sources.add("main.ty", src);
    let mut diags = Diagnostics::new();
    let tokens = tokenize(file, src, &mut diags);
    let mut out = dump_tokens(&tokens);
    if !diags.is_empty() {
        out.push_str("\n\n--- diagnostics ---\n");
        out.push_str(&render_all(diags.iter(), &sources, false));
    }
    out
}

#[test]
fn every_keyword_and_operator() {
    let src = "\
fn class if elif else while for in not and or return break continue pass
True False None is import from try except finally raise match case as
+ - * / // % ** == != < <= > >= = : , . ( ) [ ] { } -> | & ^ ~ << >>
+= -= *= /= ;
";
    insta::assert_snapshot!(report(src));
}

#[test]
fn literals() {
    let src = "\
ints = 0 42 1_000_000 0x1f 0XFF 0b1010 0B1111_0000 0o755 0O7_7
floats = 1.5 1e10 1E10 1.5e-3 1_000.5 0.0 2.0
strings = \"double\" 'single' \"esc \\n \\t \\r \\\\ \\\" \\' \\0 \\x41 \\u{1f600}\"
";
    insta::assert_snapshot!(report(src));
}

#[test]
fn layout_corner_cases() {
    let src = "\
# leading comment

fn f():
    if a:
        x
    # comment inside the block
        y
    z

    xs = [
        1,
          2,
    ]
    return xs
";
    insta::assert_snapshot!(report(src));
}

#[test]
fn f_string_forms() {
    let src = "\
a = f\"plain\"
b = f'{x}'
c = f\"{x:.2f} {y:>{w}}\"
d = f\"{{literal}} {d['k']} {a + b}\"
e = f\"\"
";
    insta::assert_snapshot!(report(src));
}

#[test]
fn error_recovery() {
    let src = "\
fn main():
    a = 0x
    b = .5
    c = 1.
    d = 1e
    e = 123abc
    f = 9223372036854775808
    g = \"bad \\q escape\"
    h = \"unterminated
    i = f\"{}\"
    j = f\"lone } brace\"
    k = a ! b
	l = 1
        m = 2
";
    insta::assert_snapshot!(report(src));
}
