use std::ops::{Range, RangeBounds};

/// A selection in the text, represented by start and end byte indices.
#[derive(Debug, Copy, Clone, PartialEq, Eq, Default)]
pub struct Selection {
    pub start: usize,
    pub end: usize,
}

impl Selection {
    pub fn new(start: usize, end: usize) -> Self {
        Self { start, end }
    }

    pub fn len(&self) -> usize {
        self.end.saturating_sub(self.start)
    }

    pub fn is_empty(&self) -> bool {
        self.start == self.end
    }

    /// Clears the selection, setting start and end to 0.
    pub fn clear(&mut self) {
        self.start = 0;
        self.end = 0;
    }

    /// Checks if the given offset is within the selection range.
    pub fn contains(&self, offset: usize) -> bool {
        offset >= self.start && offset < self.end
    }

    /// zeDB patch (multi-cursor): map this selection's byte offsets
    /// through an edit that replaced `old_len` bytes starting at
    /// `start` with `new_len` bytes. Offsets before the edit are
    /// unchanged, offsets after shift by the length delta, and an
    /// offset inside the replaced span clamps to the end of the
    /// inserted text. This is the single primitive every multi-edit
    /// remaps selections through (Helix's change-mapping idea).
    pub fn mapped_through_edit(self, start: usize, old_len: usize, new_len: usize) -> Selection {
        let old_end = start + old_len;
        let map = |pos: usize| -> usize {
            if pos <= start {
                pos
            } else if pos >= old_end {
                pos - old_len + new_len
            } else {
                start + new_len
            }
        };
        Selection::new(map(self.start), map(self.end))
    }
}

impl From<Range<usize>> for Selection {
    fn from(value: Range<usize>) -> Self {
        Self::new(value.start, value.end)
    }
}
impl From<Selection> for Range<usize> {
    fn from(value: Selection) -> Self {
        value.start..value.end
    }
}
impl RangeBounds<usize> for Selection {
    fn start_bound(&self) -> std::ops::Bound<&usize> {
        std::ops::Bound::Included(&self.start)
    }

    fn end_bound(&self) -> std::ops::Bound<&usize> {
        std::ops::Bound::Excluded(&self.end)
    }
}

/// zeDB patch (multi-cursor): the identifier-style word (alphanumeric
/// or underscore) surrounding `cursor` in `text`, or None when the
/// cursor is not on such a character.
pub fn word_range_at(text: &str, cursor: usize) -> Option<Range<usize>> {
    let bytes = text.as_bytes();
    let cursor = cursor.min(bytes.len());
    let is_word = |b: u8| b.is_ascii_alphanumeric() || b == b'_';
    let mut start = cursor;
    while start > 0 && is_word(bytes[start - 1]) {
        start -= 1;
    }
    let mut end = cursor;
    while end < bytes.len() && is_word(bytes[end]) {
        end += 1;
    }
    if start == end {
        None
    } else {
        Some(start..end)
    }
}

/// zeDB patch (multi-cursor): given non-overlapping edit ranges
/// sorted ascending by start, each replaced with `new_len` bytes, the
/// resulting caret offset for each edit, accounting for the
/// cumulative length shift of the edits to its left. The analytic
/// counterpart to applying the edits right-to-left.
pub fn multi_edit_carets(ranges: &[Range<usize>], new_len: usize) -> Vec<usize> {
    let mut carets = Vec::with_capacity(ranges.len());
    let mut delta: isize = 0;
    for range in ranges {
        let start = (range.start as isize + delta).max(0) as usize;
        carets.push(start + new_len);
        delta += new_len as isize - (range.end - range.start) as isize;
    }
    carets
}

#[cfg(test)]
mod tests {
    use super::{multi_edit_carets, Selection};
    use crate::input::Position;

    #[test]
    fn test_line_column_from_to() {
        assert_eq!(
            Position::new(1, 2),
            Position {
                line: 1,
                character: 2
            }
        );
    }

    #[test]
    fn maps_selection_through_edit() {
        // Insert 2 bytes at offset 5 (replace 0 bytes with 2).
        let after = Selection::new(10, 14).mapped_through_edit(5, 0, 2);
        assert_eq!((after.start, after.end), (12, 16));
        // Edit entirely after the selection: unchanged.
        let before = Selection::new(1, 3).mapped_through_edit(5, 0, 2);
        assert_eq!((before.start, before.end), (1, 3));
        // Delete 3 bytes at 5 (replace 3 with 0): later offsets shift back.
        let deleted = Selection::new(10, 12).mapped_through_edit(5, 3, 0);
        assert_eq!((deleted.start, deleted.end), (7, 9));
        // An offset inside the replaced span clamps to end of insert.
        let inside = Selection::new(6, 7).mapped_through_edit(5, 3, 4);
        assert_eq!((inside.start, inside.end), (9, 9));
    }

    #[test]
    fn multi_edit_carets_account_for_left_shift() {
        // Type "x" (len 1) over two 6-byte words at 7 and 20.
        assert_eq!(multi_edit_carets(&[7..13, 20..26], 1), vec![8, 16]);
        // Delete (len 0) two single chars at 3 and 9.
        assert_eq!(multi_edit_carets(&[3..4, 9..10], 0), vec![3, 8]);
        // Insert at carets (empty ranges): each shifts the next.
        assert_eq!(multi_edit_carets(&[2..2, 5..5], 3), vec![5, 11]);
    }
}
