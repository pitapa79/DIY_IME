//! Windowsホスト接続前の境界検査用。製品の位置APIや編集ポリシーではない。
use std::{error::Error, fmt, ops::Range};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Unit {
    Utf8Bytes,
    CodePoints,
    Utf16Units,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct Position {
    utf8: usize,
    code_points: usize,
    utf16: usize,
}

impl Position {
    fn offset(self, unit: Unit) -> usize {
        match unit {
            Unit::Utf8Bytes => self.utf8,
            Unit::CodePoints => self.code_points,
            Unit::Utf16Units => self.utf16,
        }
    }
}

#[derive(Debug, PartialEq, Eq)]
enum BoundaryError {
    OutOfRange { unit: Unit, offset: usize },
    InsideCharacter { unit: Unit, offset: usize },
    ReversedRange,
}

impl fmt::Display for BoundaryError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "invalid text boundary: {self:?}")
    }
}
impl Error for BoundaryError {}

/// 同じ妥当なUTF-8原文を借用し、そのコードポイント境界だけを対応させる。
/// 出力文字列・別のrevision・文書内の絶対座標にこの表を使い回さない。
struct TextPositions<'text> {
    source: &'text str,
    boundaries: Vec<Position>,
}

impl<'text> TextPositions<'text> {
    fn new(source: &'text str) -> Self {
        let mut boundaries = vec![Position {
            utf8: 0,
            code_points: 0,
            utf16: 0,
        }];
        let mut utf16 = 0;
        for (index, (byte, character)) in source.char_indices().enumerate() {
            utf16 += character.len_utf16();
            boundaries.push(Position {
                utf8: byte + character.len_utf8(),
                code_points: index + 1,
                utf16,
            });
        }
        Self { source, boundaries }
    }

    fn boundary(&self, offset: usize, unit: Unit) -> Result<Position, BoundaryError> {
        // 空文字でも開始/終端の1境界を持つ。
        let end = self.boundaries.last().unwrap().offset(unit);
        if offset > end {
            return Err(BoundaryError::OutOfRange { unit, offset });
        }
        self.boundaries
            .binary_search_by_key(&offset, |position| position.offset(unit))
            .map(|index| self.boundaries[index])
            .map_err(|_| BoundaryError::InsideCharacter { unit, offset })
    }

    /// 半開区間。順序逆転・ペア途中・範囲外を丸めずに拒否する。
    fn utf8_range_from_utf16(&self, range: Range<usize>) -> Result<Range<usize>, BoundaryError> {
        if range.start > range.end {
            return Err(BoundaryError::ReversedRange);
        }
        Ok(self.boundary(range.start, Unit::Utf16Units)?.utf8
            ..self.boundary(range.end, Unit::Utf16Units)?.utf8)
    }
}

fn main() -> Result<(), Box<dyn Error>> {
    let positions = TextPositions::new("A𠮷か\u{3099}🦀");
    println!("source: {}", positions.source);
    println!("UTF-8 bytes | code points | UTF-16 units");
    for position in &positions.boundaries {
        // 3方向が同じ境界を指すことを、表示前に照合する。
        for unit in [Unit::Utf8Bytes, Unit::CodePoints, Unit::Utf16Units] {
            assert_eq!(positions.boundary(position.offset(unit), unit)?, *position);
        }
        println!(
            "{:>11} | {:>11} | {:>11}",
            position.utf8, position.code_points, position.utf16
        );
    }
    let range = positions.utf8_range_from_utf16(1..3)?;
    println!("UTF-16 [1,3): {}", &positions.source[range]);
    assert!(positions.boundary(2, Unit::Utf16Units).is_err());
    println!("UTF-16 offset 2: rejected (inside a surrogate pair)");
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn known_mixed_text_has_exact_three_way_boundaries() {
        let positions = TextPositions::new("A𠮷か\u{3099}🦀");
        let expected = [
            (0, 0, 0),
            (1, 1, 1),
            (5, 2, 3),
            (8, 3, 4),
            (11, 4, 5),
            (15, 5, 7),
        ];
        for (utf8, code_points, utf16) in expected {
            let expected = Position {
                utf8,
                code_points,
                utf16,
            };
            for unit in [Unit::Utf8Bytes, Unit::CodePoints, Unit::Utf16Units] {
                assert_eq!(
                    positions.boundary(expected.offset(unit), unit),
                    Ok(expected)
                );
            }
        }
    }

    #[test]
    fn utf8_and_surrogate_interiors_are_rejected_without_rounding() {
        let positions = TextPositions::new("A𠮷か\u{3099}🦀");
        for offset in [2, 6] {
            assert_eq!(
                positions.boundary(offset, Unit::Utf16Units),
                Err(BoundaryError::InsideCharacter {
                    unit: Unit::Utf16Units,
                    offset
                })
            );
        }
        for offset in [2, 3, 4, 6, 7, 9, 10, 12, 13, 14] {
            assert_eq!(
                positions.boundary(offset, Unit::Utf8Bytes),
                Err(BoundaryError::InsideCharacter {
                    unit: Unit::Utf8Bytes,
                    offset
                })
            );
        }
    }

    #[test]
    fn empty_text_and_ending_boundaries_are_valid_but_overflow_is_not() {
        for source in ["", "a", "𠮷"] {
            let positions = TextPositions::new(source);
            let end = Position {
                utf8: source.len(),
                code_points: source.chars().count(),
                utf16: source.encode_utf16().count(),
            };
            for unit in [Unit::Utf8Bytes, Unit::CodePoints, Unit::Utf16Units] {
                assert_eq!(positions.boundary(0, unit).unwrap().utf8, 0);
                assert_eq!(positions.boundary(end.offset(unit), unit), Ok(end));
                for offset in [end.offset(unit) + 1, usize::MAX] {
                    assert_eq!(
                        positions.boundary(offset, unit),
                        Err(BoundaryError::OutOfRange { unit, offset })
                    );
                }
            }
        }
    }

    #[test]
    fn half_open_ranges_preserve_exact_source_and_reject_invalid_endpoints() {
        let positions = TextPositions::new("A𠮷か\u{3099}🦀");
        for (range, expected) in [
            (1..3, "𠮷"),
            (3..5, "か\u{3099}"),
            (7..7, ""),
            (0..7, positions.source),
        ] {
            let bytes = positions.utf8_range_from_utf16(range).unwrap();
            assert_eq!(&positions.source[bytes], expected);
        }
        assert_eq!(
            positions.utf8_range_from_utf16(Range { start: 3, end: 1 }),
            Err(BoundaryError::ReversedRange)
        );
        for range in [1..2, 2..3, 5..6, 7..8] {
            assert!(positions.utf8_range_from_utf16(range).is_err());
        }
    }

    #[test]
    fn scalar_edges_and_controls_round_trip_using_independent_prefix_counts() {
        let mut source = String::from("\0\r\n\tか\u{3099}");
        for scalar in [
            0x7f, 0x80, 0x7ff, 0x800, 0xd7ff, 0xe000, 0xffff, 0x10000, 0x1f980, 0x10ffff,
        ] {
            source.push(char::from_u32(scalar).unwrap());
        }
        let positions = TextPositions::new(&source);
        for byte in 0..=source.len() {
            if source.is_char_boundary(byte) {
                let prefix = &source[..byte];
                let from_byte = positions.boundary(byte, Unit::Utf8Bytes).unwrap();
                assert_eq!(from_byte.code_points, prefix.chars().count());
                assert_eq!(from_byte.utf16, prefix.encode_utf16().count());
                assert_eq!(
                    positions
                        .boundary(from_byte.utf16, Unit::Utf16Units)
                        .unwrap()
                        .utf8,
                    byte
                );
            } else {
                assert!(positions.boundary(byte, Unit::Utf8Bytes).is_err());
            }
        }
        for offset in 0..=source.encode_utf16().count() {
            if let Ok(position) = positions.boundary(offset, Unit::Utf16Units) {
                assert_eq!(source[..position.utf8].encode_utf16().count(), offset);
            }
        }
    }

    #[test]
    fn combining_boundary_is_a_scalar_boundary_without_claiming_a_grapheme_boundary() {
        let positions = TextPositions::new("か\u{3099}");
        assert_eq!(positions.boundary(1, Unit::Utf16Units).unwrap().utf8, 3);
        assert_eq!(positions.boundaries.len(), 3);
    }

    #[test]
    fn different_snapshots_have_independent_mappings() {
        let before = TextPositions::new("ab");
        let after = TextPositions::new("𠮷b");
        assert_eq!(before.boundary(1, Unit::Utf16Units).unwrap().utf8, 1);
        assert!(after.boundary(1, Unit::Utf16Units).is_err());
        assert_eq!(after.boundary(3, Unit::Utf16Units).unwrap().utf8, 5);
        assert_eq!(before.source, "ab");
        assert_eq!(after.source, "𠮷b");
    }
}
