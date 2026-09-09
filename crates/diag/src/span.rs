//! Source positions: [`FileId`] and [`Span`].

use std::fmt;
use std::ops::Range;

/// Identifies a single source file inside a [`SourceMap`](crate::SourceMap).
///
/// `FileId`s are handed out by [`SourceMap::add`](crate::SourceMap::add) and are
/// only meaningful together with the map that created them.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct FileId(pub u32);

impl FileId {
    /// The id used by [`Span::dummy`]; it is never handed out by a [`SourceMap`](crate::SourceMap).
    pub const DUMMY: FileId = FileId(u32::MAX);

    /// Returns the raw index of this file.
    pub fn index(self) -> usize {
        self.0 as usize
    }
}

impl fmt::Display for FileId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "file#{}", self.0)
    }
}

/// A half-open byte range `[start, end)` inside one source file.
///
/// Offsets are byte offsets into the file text, not character indices, so they
/// can be used directly to slice the source string.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct Span {
    /// The file this span points into.
    pub file: FileId,
    /// Byte offset of the first byte of the span.
    pub start: u32,
    /// Byte offset one past the last byte of the span.
    pub end: u32,
}

impl Span {
    /// Creates a span covering `start..end` in `file`.
    ///
    /// The arguments are swapped if `start > end` so that a span is always
    /// well-formed.
    pub fn new(file: FileId, start: u32, end: u32) -> Span {
        if start <= end {
            Span { file, start, end }
        } else {
            Span {
                file,
                start: end,
                end: start,
            }
        }
    }

    /// A span that points nowhere; used for synthesized nodes that have no
    /// source text of their own.
    pub fn dummy() -> Span {
        Span {
            file: FileId::DUMMY,
            start: 0,
            end: 0,
        }
    }

    /// Returns `true` if this is the [`Span::dummy`] span.
    pub fn is_dummy(self) -> bool {
        self.file == FileId::DUMMY
    }

    /// Length of the span in bytes.
    pub fn len(self) -> u32 {
        self.end - self.start
    }

    /// Returns `true` if the span covers no bytes at all.
    pub fn is_empty(self) -> bool {
        self.start == self.end
    }

    /// The smallest span covering both `self` and `other`.
    ///
    /// If exactly one of the two is [`Span::dummy`] the other one is returned;
    /// if the spans belong to different files `self` is returned unchanged
    /// (merging across files is meaningless).
    pub fn merge(self, other: Span) -> Span {
        if self.is_dummy() {
            return other;
        }
        if other.is_dummy() || self.file != other.file {
            return self;
        }
        Span {
            file: self.file,
            start: self.start.min(other.start),
            end: self.end.max(other.end),
        }
    }

    /// An empty span pointing at the first byte of this span.
    pub fn shrink_to_start(self) -> Span {
        Span {
            file: self.file,
            start: self.start,
            end: self.start,
        }
    }

    /// An empty span pointing just past the last byte of this span.
    pub fn shrink_to_end(self) -> Span {
        Span {
            file: self.file,
            start: self.end,
            end: self.end,
        }
    }

    /// The byte range of this span, for slicing the file text.
    pub fn range(self) -> Range<usize> {
        self.start as usize..self.end as usize
    }
}

impl fmt::Display for Span {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}..{}", self.start, self.end)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const F0: FileId = FileId(0);
    const F1: FileId = FileId(1);

    #[test]
    fn new_orders_bounds() {
        let s = Span::new(F0, 5, 2);
        assert_eq!((s.start, s.end), (2, 5));
    }

    #[test]
    fn len_and_empty() {
        assert_eq!(Span::new(F0, 3, 7).len(), 4);
        assert!(Span::new(F0, 3, 3).is_empty());
        assert!(!Span::new(F0, 3, 4).is_empty());
    }

    #[test]
    fn merge_covers_both() {
        let a = Span::new(F0, 2, 5);
        let b = Span::new(F0, 10, 12);
        assert_eq!(a.merge(b), Span::new(F0, 2, 12));
        assert_eq!(b.merge(a), Span::new(F0, 2, 12));
    }

    #[test]
    fn merge_nested() {
        let outer = Span::new(F0, 0, 20);
        let inner = Span::new(F0, 5, 6);
        assert_eq!(outer.merge(inner), outer);
    }

    #[test]
    fn merge_with_dummy_returns_other() {
        let a = Span::new(F0, 2, 5);
        assert_eq!(a.merge(Span::dummy()), a);
        assert_eq!(Span::dummy().merge(a), a);
        assert!(Span::dummy().merge(Span::dummy()).is_dummy());
    }

    #[test]
    fn merge_across_files_keeps_self() {
        let a = Span::new(F0, 2, 5);
        let b = Span::new(F1, 0, 1);
        assert_eq!(a.merge(b), a);
    }

    #[test]
    fn shrink() {
        let s = Span::new(F0, 4, 9);
        assert_eq!(s.shrink_to_start(), Span::new(F0, 4, 4));
        assert_eq!(s.shrink_to_end(), Span::new(F0, 9, 9));
    }

    #[test]
    fn range_slices_source() {
        let text = "hello world";
        let s = Span::new(F0, 6, 11);
        assert_eq!(&text[s.range()], "world");
    }

    #[test]
    fn dummy_is_dummy() {
        assert!(Span::dummy().is_dummy());
        assert!(!Span::new(F0, 0, 0).is_dummy());
    }

    #[test]
    fn display() {
        assert_eq!(Span::new(F0, 1, 4).to_string(), "1..4");
        assert_eq!(F1.to_string(), "file#1");
    }
}
