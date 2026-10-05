//! Byte-oriented editing of a document.
//!
//! Output bytes always derive from input bytes. An edit replaces one byte range; everything
//! outside every range is copied through untouched. There is no serialiser, so there is nothing
//! that can quietly renormalise a declaration, reorder attributes, or collapse an empty element.

use super::scan::Span;
use std::fmt;

/// One replacement: the range to remove and the bytes to put in its place.
#[derive(Clone, Debug)]
pub struct Edit {
    pub span: Span,
    pub replacement: Vec<u8>,
}

impl Edit {
    pub fn new(span: Span, replacement: impl Into<Vec<u8>>) -> Self {
        Edit {
            span,
            replacement: replacement.into(),
        }
    }
}

#[derive(Debug)]
pub enum SpliceError {
    /// Two edits cover overlapping bytes, so the result would depend on their order.
    Overlap { first: Span, second: Span },
    /// An edit points outside the document.
    OutOfBounds { span: Span, len: usize },
    /// Two zero-length edits share a position, so their order would decide the result.
    CoincidentInsert { at: usize },
    /// The result would not fit in memory addressable by this platform.
    TooLarge,
}

impl fmt::Display for SpliceError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            SpliceError::Overlap { first, second } => write!(
                f,
                "overlapping edits: {}..{} and {}..{}",
                first.start, first.end, second.start, second.end
            ),
            SpliceError::OutOfBounds { span, len } => write!(
                f,
                "edit {}..{} is outside a document of {} bytes",
                span.start, span.end, len
            ),
            SpliceError::CoincidentInsert { at } => {
                write!(
                    f,
                    "two insertions at byte {at}; their order would decide the result"
                )
            }
            SpliceError::TooLarge => write!(f, "spliced document would be too large"),
        }
    }
}

impl std::error::Error for SpliceError {}

/// Apply edits to a document.
///
/// With no edits this returns the input unchanged, which is the identity property the corpus
/// test pins down. Edits may be supplied in any order; they are applied by position, and
/// overlapping edits are refused rather than silently resolved.
pub fn splice(src: &[u8], edits: &[Edit]) -> Result<Vec<u8>, SpliceError> {
    for edit in edits {
        if edit.span.end > src.len() || edit.span.start > edit.span.end {
            return Err(SpliceError::OutOfBounds {
                span: edit.span,
                len: src.len(),
            });
        }
    }

    let mut ordered: Vec<&Edit> = edits.iter().collect();
    ordered.sort_by_key(|e| (e.span.start, e.span.end));
    for pair in ordered.windows(2) {
        // Touching is fine (one ends where the next begins); covering the same byte is not.
        if pair[1].span.start < pair[0].span.end {
            return Err(SpliceError::Overlap {
                first: pair[0].span,
                second: pair[1].span,
            });
        }
        // Two insertions at the same point would apply in whatever order they were passed,
        // which contradicts the promise that argument order does not matter.
        if pair[0].span.is_empty()
            && pair[1].span.is_empty()
            && pair[0].span.start == pair[1].span.start
        {
            return Err(SpliceError::CoincidentInsert {
                at: pair[0].span.start,
            });
        }
    }

    let removed: usize = ordered.iter().map(|e| e.span.len()).sum();
    let added: usize = ordered.iter().map(|e| e.replacement.len()).sum();
    let final_len = src
        .len()
        .checked_sub(removed)
        .and_then(|kept| kept.checked_add(added))
        .ok_or(SpliceError::TooLarge)?;
    let mut out = Vec::with_capacity(final_len);
    let mut cursor = 0usize;
    for edit in ordered {
        out.extend_from_slice(&src[cursor..edit.span.start]);
        out.extend_from_slice(&edit.replacement);
        cursor = edit.span.end;
    }
    out.extend_from_slice(&src[cursor..]);
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn no_edits_is_the_identity() {
        let src = b"<a x=\"1\"><b/></a>";
        assert_eq!(splice(src, &[]).unwrap(), src.to_vec());
    }

    #[test]
    fn replacing_a_range_with_itself_is_the_identity() {
        let src = b"<a x=\"1\"><b/></a>";
        let edit = Edit::new(Span::new(3, 8), &src[3..8]);
        assert_eq!(splice(src, &[edit]).unwrap(), src.to_vec());
    }

    #[test]
    fn edits_apply_by_position_not_argument_order() {
        let src = b"one two three";
        let later = Edit::new(Span::new(8, 13), b"THREE".to_vec());
        let earlier = Edit::new(Span::new(0, 3), b"ONE".to_vec());
        let out = splice(src, &[later, earlier]).unwrap();
        assert_eq!(out, b"ONE two THREE".to_vec());
    }

    #[test]
    fn touching_edits_are_allowed() {
        let src = b"abcd";
        let edits = [
            Edit::new(Span::new(0, 2), b"X".to_vec()),
            Edit::new(Span::new(2, 4), b"Y".to_vec()),
        ];
        assert_eq!(splice(src, &edits).unwrap(), b"XY".to_vec());
    }

    #[test]
    fn overlapping_edits_are_refused() {
        let src = b"abcd";
        let edits = [
            Edit::new(Span::new(0, 3), b"X".to_vec()),
            Edit::new(Span::new(2, 4), b"Y".to_vec()),
        ];
        assert!(matches!(
            splice(src, &edits),
            Err(SpliceError::Overlap { .. })
        ));
    }

    #[test]
    fn two_insertions_at_one_point_are_refused() {
        let src = b"abcd";
        let edits = [
            Edit::new(Span::new(2, 2), b"X".to_vec()),
            Edit::new(Span::new(2, 2), b"Y".to_vec()),
        ];
        assert!(matches!(
            splice(src, &edits),
            Err(SpliceError::CoincidentInsert { .. })
        ));
    }

    #[test]
    fn a_single_insertion_is_applied() {
        let src = b"abcd";
        let edits = [Edit::new(Span::new(2, 2), b"--".to_vec())];
        assert_eq!(splice(src, &edits).unwrap(), b"ab--cd".to_vec());
    }

    #[test]
    fn an_edit_past_the_end_is_refused() {
        let src = b"abcd";
        let edits = [Edit::new(Span::new(2, 99), b"X".to_vec())];
        assert!(matches!(
            splice(src, &edits),
            Err(SpliceError::OutOfBounds { .. })
        ));
    }
}
