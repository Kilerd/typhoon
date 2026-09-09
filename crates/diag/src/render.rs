//! Rendering of [`Diagnostic`]s into rustc-style text using `annotate-snippets`.

use annotate_snippets::{
    Annotation, AnnotationKind, Group, Renderer, Snippet, level::Level as AsLevel,
};

use crate::diagnostic::{Diagnostic, Level};
use crate::source_map::SourceMap;
use crate::span::FileId;

/// Renders one diagnostic.
///
/// The output looks like rustc's:
///
/// ```text
/// error: expected `:`, found `{`
///  --> main.ty:1:14
///   |
/// 1 | fn main() {
///   |           ^ expected `:` here
///   |
///   = help: typhoon uses indentation blocks, not braces
/// ```
///
/// `color` selects between the ANSI-styled and the plain renderer. The
/// returned string has no trailing newline.
pub fn render(diag: &Diagnostic, sources: &SourceMap, color: bool) -> String {
    let renderer = if color {
        Renderer::styled()
    } else {
        Renderer::plain()
    };
    renderer.render(&build_groups(diag, sources))
}

/// Renders a sequence of diagnostics, separated by a blank line.
pub fn render_all<'a>(
    diags: impl IntoIterator<Item = &'a Diagnostic>,
    sources: &SourceMap,
    color: bool,
) -> String {
    let mut out = String::new();
    for diag in diags {
        if !out.is_empty() {
            out.push_str("\n\n");
        }
        out.push_str(&render(diag, sources, color));
    }
    out
}

fn build_groups<'a>(diag: &'a Diagnostic, sources: &'a SourceMap) -> Vec<Group<'a>> {
    let level = match diag.level {
        Level::Error => AsLevel::ERROR,
        Level::Warning => AsLevel::WARNING,
    };
    let mut title = level.primary_title(diag.message.as_str());
    if let Some(code) = &diag.code {
        title = title.id(code.as_str());
    }
    let mut group = Group::with_title(title);

    // Group the labels by file, with the file of the primary label first, and
    // the remaining files in order of first appearance.
    let mut files: Vec<FileId> = Vec::new();
    if let Some(span) = diag.span()
        && sources.try_text(span.file).is_some()
    {
        files.push(span.file);
    }
    for label in &diag.labels {
        if sources.try_text(label.span.file).is_some() && !files.contains(&label.span.file) {
            files.push(label.span.file);
        }
    }

    for file in files {
        let text = sources.text(file);
        let mut snippet: Snippet<'a, Annotation<'a>> = Snippet::source(text)
            .path(sources.name(file))
            .line_start(1)
            .fold(true);
        for label in diag.labels.iter().filter(|l| l.span.file == file) {
            let start = snap_down(text, label.span.start as usize);
            let end = snap_up(text, (label.span.end as usize).max(start));
            let kind = if label.primary {
                AnnotationKind::Primary
            } else {
                AnnotationKind::Context
            };
            let ann = kind.span(start..end);
            let ann = if label.message.is_empty() {
                ann
            } else {
                ann.label(label.message.as_str())
            };
            snippet = snippet.annotation(ann);
        }
        group = group.element(snippet);
    }

    for note in &diag.notes {
        group = group.element(AsLevel::NOTE.message(note.as_str()));
    }
    for help in &diag.help {
        group = group.element(AsLevel::HELP.message(help.as_str()));
    }

    vec![group]
}

/// Clamps `offset` into `text` and moves it down to the nearest char boundary.
fn snap_down(text: &str, offset: usize) -> usize {
    let mut o = offset.min(text.len());
    while o > 0 && !text.is_char_boundary(o) {
        o -= 1;
    }
    o
}

/// Clamps `offset` into `text` and moves it up to the nearest char boundary.
fn snap_up(text: &str, offset: usize) -> usize {
    let mut o = offset.min(text.len());
    while o < text.len() && !text.is_char_boundary(o) {
        o += 1;
    }
    o
}
