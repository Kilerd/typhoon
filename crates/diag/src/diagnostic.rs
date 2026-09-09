//! [`Diagnostic`], its builder API and the [`Diagnostics`] collector.

use crate::span::Span;

/// Severity of a [`Diagnostic`].
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum Level {
    /// Compilation cannot succeed.
    Error,
    /// Something suspicious, compilation continues.
    Warning,
}

impl Level {
    /// The lowercase name used when rendering (`"error"` / `"warning"`).
    pub fn as_str(self) -> &'static str {
        match self {
            Level::Error => "error",
            Level::Warning => "warning",
        }
    }
}

/// A span with an explanatory message, underlined in the rendered output.
///
/// Exactly one label of a diagnostic is normally `primary`; it is underlined
/// with `^^^` and determines the `--> file:line:col` header. Secondary labels
/// are underlined with `---`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Label {
    /// Where the label points.
    pub span: Span,
    /// The message shown next to the underline. May be empty.
    pub message: String,
    /// Whether this is the primary label.
    pub primary: bool,
}

impl Label {
    /// Creates a primary label.
    pub fn primary(span: Span, message: impl Into<String>) -> Label {
        Label {
            span,
            message: message.into(),
            primary: true,
        }
    }

    /// Creates a secondary label.
    pub fn secondary(span: Span, message: impl Into<String>) -> Label {
        Label {
            span,
            message: message.into(),
            primary: false,
        }
    }
}

/// A single compiler message: a level, a headline, labelled spans and footers.
///
/// Build one with [`Diagnostic::error`] / [`Diagnostic::warning`] and the
/// `with_*` methods:
///
/// ```
/// # use typhoon_diag::{Diagnostic, FileId, Span};
/// # let span = Span::new(FileId(0), 0, 3);
/// let d = Diagnostic::error("expected `:`, found `{`")
///     .with_code("E0001")
///     .with_label(span, "expected `:` here")
///     .with_help("typhoon uses indentation blocks, not braces");
/// assert_eq!(d.span(), Some(span));
/// ```
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Diagnostic {
    /// Error or warning.
    pub level: Level,
    /// Optional machine readable code, e.g. `"E0001"`.
    pub code: Option<String>,
    /// The headline, printed after `error: `.
    pub message: String,
    /// Labelled spans; at most one is primary.
    pub labels: Vec<Label>,
    /// `= note: ...` footers.
    pub notes: Vec<String>,
    /// `= help: ...` footers.
    pub help: Vec<String>,
}

impl Diagnostic {
    /// Creates a diagnostic with the given level and headline.
    pub fn new(level: Level, message: impl Into<String>) -> Diagnostic {
        Diagnostic {
            level,
            code: None,
            message: message.into(),
            labels: Vec::new(),
            notes: Vec::new(),
            help: Vec::new(),
        }
    }

    /// Creates an error diagnostic.
    pub fn error(message: impl Into<String>) -> Diagnostic {
        Diagnostic::new(Level::Error, message)
    }

    /// Creates a warning diagnostic.
    pub fn warning(message: impl Into<String>) -> Diagnostic {
        Diagnostic::new(Level::Warning, message)
    }

    /// Sets the diagnostic code.
    #[must_use]
    pub fn with_code(mut self, code: impl Into<String>) -> Diagnostic {
        self.code = Some(code.into());
        self
    }

    /// Adds the primary label (`^^^`).
    #[must_use]
    pub fn with_label(mut self, span: Span, message: impl Into<String>) -> Diagnostic {
        self.labels.push(Label::primary(span, message));
        self
    }

    /// Adds a secondary label (`---`).
    #[must_use]
    pub fn with_secondary(mut self, span: Span, message: impl Into<String>) -> Diagnostic {
        self.labels.push(Label::secondary(span, message));
        self
    }

    /// Adds a `= note: ...` footer.
    #[must_use]
    pub fn with_note(mut self, note: impl Into<String>) -> Diagnostic {
        self.notes.push(note.into());
        self
    }

    /// Adds a `= help: ...` footer.
    #[must_use]
    pub fn with_help(mut self, help: impl Into<String>) -> Diagnostic {
        self.help.push(help.into());
        self
    }

    /// The span of the primary label, or of the first label if none is marked
    /// primary, or `None` for a diagnostic without labels.
    pub fn span(&self) -> Option<Span> {
        self.labels
            .iter()
            .find(|l| l.primary)
            .or_else(|| self.labels.first())
            .map(|l| l.span)
    }

    /// Returns `true` if this diagnostic is an error.
    pub fn is_error(&self) -> bool {
        self.level == Level::Error
    }
}

/// A collector for diagnostics produced during one compilation.
///
/// Every phase takes `&mut Diagnostics` and pushes into it instead of
/// returning `Result`, which is what makes multi-error reporting and parser
/// error recovery possible.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Diagnostics {
    items: Vec<Diagnostic>,
}

impl Diagnostics {
    /// Creates an empty collector.
    pub fn new() -> Diagnostics {
        Diagnostics { items: Vec::new() }
    }

    /// Appends a diagnostic.
    pub fn push(&mut self, diag: Diagnostic) {
        self.items.push(diag);
    }

    /// Appends every diagnostic of `other`.
    pub fn extend(&mut self, other: impl IntoIterator<Item = Diagnostic>) {
        self.items.extend(other);
    }

    /// Returns `true` if at least one [`Level::Error`] diagnostic was pushed.
    pub fn has_errors(&self) -> bool {
        self.items.iter().any(Diagnostic::is_error)
    }

    /// Number of collected diagnostics.
    pub fn len(&self) -> usize {
        self.items.len()
    }

    /// Returns `true` if nothing was collected.
    pub fn is_empty(&self) -> bool {
        self.items.is_empty()
    }

    /// Iterates over the collected diagnostics in insertion order.
    pub fn iter(&self) -> std::slice::Iter<'_, Diagnostic> {
        self.items.iter()
    }

    /// The collected diagnostics as a slice.
    pub fn as_slice(&self) -> &[Diagnostic] {
        &self.items
    }

    /// Consumes the collector and returns the diagnostics.
    pub fn into_vec(self) -> Vec<Diagnostic> {
        self.items
    }
}

impl<'a> IntoIterator for &'a Diagnostics {
    type Item = &'a Diagnostic;
    type IntoIter = std::slice::Iter<'a, Diagnostic>;

    fn into_iter(self) -> Self::IntoIter {
        self.items.iter()
    }
}

impl IntoIterator for Diagnostics {
    type Item = Diagnostic;
    type IntoIter = std::vec::IntoIter<Diagnostic>;

    fn into_iter(self) -> Self::IntoIter {
        self.items.into_iter()
    }
}

impl FromIterator<Diagnostic> for Diagnostics {
    fn from_iter<T: IntoIterator<Item = Diagnostic>>(iter: T) -> Diagnostics {
        Diagnostics {
            items: iter.into_iter().collect(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::span::FileId;

    const F: FileId = FileId(0);

    fn span(start: u32, end: u32) -> Span {
        Span::new(F, start, end)
    }

    #[test]
    fn level_names() {
        assert_eq!(Level::Error.as_str(), "error");
        assert_eq!(Level::Warning.as_str(), "warning");
    }

    #[test]
    fn builder_fills_every_field() {
        let d = Diagnostic::error("boom")
            .with_code("E0001")
            .with_label(span(0, 1), "here")
            .with_secondary(span(4, 5), "and here")
            .with_note("a note")
            .with_note("another note")
            .with_help("a help");
        assert_eq!(d.level, Level::Error);
        assert_eq!(d.code.as_deref(), Some("E0001"));
        assert_eq!(d.message, "boom");
        assert_eq!(d.labels.len(), 2);
        assert!(d.labels[0].primary);
        assert!(!d.labels[1].primary);
        assert_eq!(d.labels[1].message, "and here");
        assert_eq!(
            d.notes,
            vec!["a note".to_string(), "another note".to_string()]
        );
        assert_eq!(d.help, vec!["a help".to_string()]);
        assert!(d.is_error());
    }

    #[test]
    fn warning_is_not_an_error() {
        let d = Diagnostic::warning("careful");
        assert_eq!(d.level, Level::Warning);
        assert!(!d.is_error());
    }

    #[test]
    fn span_prefers_the_primary_label() {
        let d = Diagnostic::error("boom")
            .with_secondary(span(10, 12), "secondary first")
            .with_label(span(0, 1), "primary");
        assert_eq!(d.span(), Some(span(0, 1)));
    }

    #[test]
    fn span_falls_back_to_first_label() {
        let d = Diagnostic::error("boom").with_secondary(span(10, 12), "only secondary");
        assert_eq!(d.span(), Some(span(10, 12)));
    }

    #[test]
    fn span_is_none_without_labels() {
        assert_eq!(Diagnostic::error("boom").span(), None);
    }

    #[test]
    fn label_constructors() {
        assert!(Label::primary(span(0, 1), "p").primary);
        assert!(!Label::secondary(span(0, 1), "s").primary);
    }

    #[test]
    fn collector_basics() {
        let mut diags = Diagnostics::new();
        assert!(diags.is_empty());
        assert_eq!(diags.len(), 0);
        assert!(!diags.has_errors());

        diags.push(Diagnostic::warning("w"));
        assert!(!diags.has_errors());
        assert_eq!(diags.len(), 1);
        assert!(!diags.is_empty());

        diags.push(Diagnostic::error("e"));
        assert!(diags.has_errors());
        assert_eq!(diags.len(), 2);
        assert_eq!(
            diags.iter().map(|d| d.message.as_str()).collect::<Vec<_>>(),
            ["w", "e"]
        );
    }

    #[test]
    fn collector_extend_and_into_vec() {
        let mut a = Diagnostics::new();
        a.push(Diagnostic::error("one"));
        let mut b = Diagnostics::new();
        b.push(Diagnostic::error("two"));
        a.extend(b);
        let v = a.into_vec();
        assert_eq!(v.len(), 2);
        assert_eq!(v[1].message, "two");
    }

    #[test]
    fn collector_iteration_traits() {
        let diags: Diagnostics = [Diagnostic::error("a"), Diagnostic::warning("b")]
            .into_iter()
            .collect();
        assert_eq!(diags.as_slice().len(), 2);
        let by_ref: Vec<&Diagnostic> = (&diags).into_iter().collect();
        assert_eq!(by_ref.len(), 2);
        let owned: Vec<Diagnostic> = diags.into_iter().collect();
        assert_eq!(owned[1].message, "b");
    }
}
