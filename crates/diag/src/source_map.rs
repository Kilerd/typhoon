//! The [`SourceMap`]: owns the text of every file the compiler has read.

use crate::span::{FileId, Span};

/// One file inside a [`SourceMap`].
#[derive(Debug, Clone)]
struct SourceFile {
    name: String,
    text: String,
    /// Byte offset of the first byte of every line (`line_starts[0] == 0`).
    line_starts: Vec<u32>,
}

impl SourceFile {
    fn new(name: String, text: String) -> SourceFile {
        let mut line_starts = vec![0u32];
        for (i, b) in text.bytes().enumerate() {
            if b == b'\n' {
                line_starts.push(i as u32 + 1);
            }
        }
        SourceFile {
            name,
            text,
            line_starts,
        }
    }
}

/// A collection of source files, keyed by [`FileId`].
///
/// The source map owns the text; spans produced by the lexer and parser refer
/// back into it and the renderer uses it to print snippets.
#[derive(Debug, Clone, Default)]
pub struct SourceMap {
    files: Vec<SourceFile>,
}

impl SourceMap {
    /// Creates an empty source map.
    pub fn new() -> SourceMap {
        SourceMap { files: Vec::new() }
    }

    /// Adds a file with the given display `name` and `text`, returning its id.
    pub fn add(&mut self, name: impl Into<String>, text: impl Into<String>) -> FileId {
        let id = FileId(self.files.len() as u32);
        self.files.push(SourceFile::new(name.into(), text.into()));
        id
    }

    /// Number of files in the map.
    pub fn len(&self) -> usize {
        self.files.len()
    }

    /// Returns `true` if no file has been added yet.
    pub fn is_empty(&self) -> bool {
        self.files.is_empty()
    }

    /// The display name of `file`.
    ///
    /// # Panics
    ///
    /// Panics if `file` was not created by this source map.
    pub fn name(&self, file: FileId) -> &str {
        &self.file(file).name
    }

    /// The full text of `file`.
    ///
    /// # Panics
    ///
    /// Panics if `file` was not created by this source map.
    pub fn text(&self, file: FileId) -> &str {
        &self.file(file).text
    }

    /// The display name of `file`, or `None` for an unknown id.
    pub fn try_name(&self, file: FileId) -> Option<&str> {
        self.files.get(file.index()).map(|f| f.name.as_str())
    }

    /// The text of `file`, or `None` for an unknown id.
    pub fn try_text(&self, file: FileId) -> Option<&str> {
        self.files.get(file.index()).map(|f| f.text.as_str())
    }

    /// The source text covered by `span`, or `None` if the span is out of
    /// bounds or does not land on character boundaries.
    pub fn span_text(&self, span: Span) -> Option<&str> {
        self.try_text(span.file)?.get(span.range())
    }

    /// Converts a byte `offset` inside `file` into a 1-based `(line, column)`
    /// pair. The column counts **characters**, not bytes, so a span after a
    /// multi-byte character still reports a sensible column.
    ///
    /// An offset past the end of the file is clamped to the end; an offset in
    /// the middle of a multi-byte character is snapped down to its start.
    ///
    /// # Panics
    ///
    /// Panics if `file` was not created by this source map.
    pub fn line_col(&self, file: FileId, offset: u32) -> (usize, usize) {
        let f = self.file(file);
        let offset = offset.min(f.text.len() as u32) as usize;
        // The index of the last line start that is <= offset.
        let line_idx = match f.line_starts.binary_search(&(offset as u32)) {
            Ok(i) => i,
            Err(i) => i - 1,
        };
        let line_start = f.line_starts[line_idx] as usize;
        // Column in characters. `offset` may not be a char boundary if the
        // caller passed a bogus value; snap down to the nearest one.
        let mut offset = offset;
        while offset > line_start && !f.text.is_char_boundary(offset) {
            offset -= 1;
        }
        let col = f.text[line_start..offset].chars().count() + 1;
        (line_idx + 1, col)
    }

    /// The 0-based byte offsets at which each line of `file` starts.
    ///
    /// # Panics
    ///
    /// Panics if `file` was not created by this source map.
    pub fn line_starts(&self, file: FileId) -> &[u32] {
        &self.file(file).line_starts
    }

    /// The text of a 1-based `line` of `file`, without the trailing newline.
    pub fn line_text(&self, file: FileId, line: usize) -> Option<&str> {
        let f = self.files.get(file.index())?;
        let start = *f.line_starts.get(line.checked_sub(1)?)? as usize;
        let end = f
            .line_starts
            .get(line)
            .map(|e| *e as usize)
            .unwrap_or(f.text.len());
        Some(
            f.text[start..end]
                .trim_end_matches('\n')
                .trim_end_matches('\r'),
        )
    }

    fn file(&self, file: FileId) -> &SourceFile {
        self.files
            .get(file.index())
            .unwrap_or_else(|| panic!("unknown FileId {file}"))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn add_returns_increasing_ids() {
        let mut sm = SourceMap::new();
        let a = sm.add("a.ty", "a");
        let b = sm.add("b.ty", "b");
        assert_eq!((a, b), (FileId(0), FileId(1)));
        assert_eq!(sm.len(), 2);
        assert!(!sm.is_empty());
        assert!(SourceMap::new().is_empty());
    }

    #[test]
    fn name_and_text_round_trip() {
        let mut sm = SourceMap::new();
        let f = sm.add("main.ty", "fn main():\n    pass\n");
        assert_eq!(sm.name(f), "main.ty");
        assert_eq!(sm.text(f), "fn main():\n    pass\n");
        assert_eq!(sm.try_name(FileId(7)), None);
        assert_eq!(sm.try_text(FileId(7)), None);
    }

    #[test]
    fn line_col_single_line() {
        let mut sm = SourceMap::new();
        let f = sm.add("a.ty", "abcdef");
        assert_eq!(sm.line_col(f, 0), (1, 1));
        assert_eq!(sm.line_col(f, 3), (1, 4));
        assert_eq!(sm.line_col(f, 6), (1, 7));
    }

    #[test]
    fn line_col_multi_line() {
        let mut sm = SourceMap::new();
        //              0123 4567 89
        let f = sm.add("a.ty", "abc\ndef\ngh");
        assert_eq!(sm.line_col(f, 0), (1, 1));
        assert_eq!(sm.line_col(f, 3), (1, 4)); // the '\n' itself
        assert_eq!(sm.line_col(f, 4), (2, 1));
        assert_eq!(sm.line_col(f, 6), (2, 3));
        assert_eq!(sm.line_col(f, 8), (3, 1));
        assert_eq!(sm.line_col(f, 9), (3, 2));
    }

    #[test]
    fn line_col_empty_lines() {
        let mut sm = SourceMap::new();
        let f = sm.add("a.ty", "\n\nx\n");
        assert_eq!(sm.line_col(f, 0), (1, 1));
        assert_eq!(sm.line_col(f, 1), (2, 1));
        assert_eq!(sm.line_col(f, 2), (3, 1));
        assert_eq!(sm.line_col(f, 3), (3, 2));
    }

    #[test]
    fn line_col_counts_characters_not_bytes() {
        let mut sm = SourceMap::new();
        // "héllo" -> h(1) é(2 bytes) l l o
        let f = sm.add("a.ty", "héllo\nwörld");
        assert_eq!(sm.line_col(f, 0), (1, 1));
        assert_eq!(sm.line_col(f, 1), (1, 2)); // start of 'é'
        assert_eq!(sm.line_col(f, 3), (1, 3)); // 'l', one char after 'é'
        assert_eq!(sm.line_col(f, 6), (1, 6)); // '\n'
        assert_eq!(sm.line_col(f, 7), (2, 1)); // 'w'
        assert_eq!(sm.line_col(f, 10), (2, 3)); // 'r' after two-byte 'ö'
        assert_eq!(sm.line_col(f, 9), (2, 2)); // inside 'ö', snapped to its start
    }

    #[test]
    fn line_col_with_astral_chars() {
        let mut sm = SourceMap::new();
        // 🌀 is 4 bytes.
        let f = sm.add("a.ty", "🌀x");
        assert_eq!(sm.line_col(f, 0), (1, 1));
        assert_eq!(sm.line_col(f, 4), (1, 2));
        assert_eq!(sm.line_col(f, 5), (1, 3));
    }

    #[test]
    fn line_col_clamps_past_end() {
        let mut sm = SourceMap::new();
        let f = sm.add("a.ty", "ab\ncd");
        assert_eq!(sm.line_col(f, 100), (2, 3));
    }

    #[test]
    fn line_col_empty_file() {
        let mut sm = SourceMap::new();
        let f = sm.add("a.ty", "");
        assert_eq!(sm.line_col(f, 0), (1, 1));
    }

    #[test]
    fn line_col_crlf_text() {
        let mut sm = SourceMap::new();
        let f = sm.add("a.ty", "ab\r\ncd");
        assert_eq!(sm.line_col(f, 2), (1, 3)); // '\r'
        assert_eq!(sm.line_col(f, 4), (2, 1)); // 'c'
    }

    #[test]
    fn line_starts_are_recorded() {
        let mut sm = SourceMap::new();
        let f = sm.add("a.ty", "abc\ndef\n");
        assert_eq!(sm.line_starts(f), &[0, 4, 8]);
    }

    #[test]
    fn line_text_strips_line_endings() {
        let mut sm = SourceMap::new();
        let f = sm.add("a.ty", "abc\r\ndef\nghi");
        assert_eq!(sm.line_text(f, 1), Some("abc"));
        assert_eq!(sm.line_text(f, 2), Some("def"));
        assert_eq!(sm.line_text(f, 3), Some("ghi"));
        assert_eq!(sm.line_text(f, 4), None);
        assert_eq!(sm.line_text(f, 0), None);
    }

    #[test]
    fn span_text_extracts_source() {
        let mut sm = SourceMap::new();
        let f = sm.add("a.ty", "fn main():");
        assert_eq!(sm.span_text(Span::new(f, 3, 7)), Some("main"));
        assert_eq!(sm.span_text(Span::new(f, 3, 100)), None);
        assert_eq!(sm.span_text(Span::dummy()), None);
    }

    #[test]
    #[should_panic(expected = "unknown FileId")]
    fn unknown_file_panics() {
        let sm = SourceMap::new();
        sm.text(FileId(3));
    }
}
