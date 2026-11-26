//! UTF-16 positions within a single, unchanged string snapshot.

use std::ops::Range;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BoundaryError {
    MalformedUtf16,
    InvalidRange,
}

/// Validates the whole snapshot and maps a half-open UTF-16 selection to bytes.
/// Returns an error for a reversed, out-of-range, or split-surrogate selection.
pub fn utf8_selection(
    snapshot: &[u16],
    selection: Range<usize>,
) -> Result<Range<usize>, BoundaryError> {
    if selection.start > selection.end || selection.end > snapshot.len() {
        return Err(BoundaryError::InvalidRange);
    }

    let mut utf8 = 0;
    let mut utf16 = 0;
    let mut start = None;
    let mut end = None;
    for scalar in char::decode_utf16(snapshot.iter().copied()) {
        if utf16 == selection.start {
            start = Some(utf8);
        }
        if utf16 == selection.end {
            end = Some(utf8);
        }
        let scalar = scalar.map_err(|_| BoundaryError::MalformedUtf16)?;
        utf8 += scalar.len_utf8();
        utf16 += scalar.len_utf16();
    }
    if utf16 == selection.start {
        start = Some(utf8);
    }
    if utf16 == selection.end {
        end = Some(utf8);
    }
    match (start, end) {
        (Some(start), Some(end)) => Ok(start..end),
        _ => Err(BoundaryError::InvalidRange),
    }
}
