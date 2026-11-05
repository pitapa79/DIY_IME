use std::{error::Error, fmt};

use crate::{Candidate, PACKET_VERSION, Packet, TextMeta};

/// 文章候補データが共通仕様を満たさない理由。
#[derive(Debug, Clone, PartialEq)]
pub enum ValidationError {
    UnsupportedVersion { found: u32 },
    NonFiniteScore { value: f64 },
    ReversedSourceSpan { start: usize, end: usize },
}

impl fmt::Display for ValidationError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::UnsupportedVersion { found } => {
                write!(
                    f,
                    "unsupported Packet version: {found} (expected {PACKET_VERSION})"
                )
            }
            Self::NonFiniteScore { value } => write!(f, "score must be finite: {value}"),
            Self::ReversedSourceSpan { start, end } => {
                write!(f, "source_span start must not exceed end: [{start}, {end})")
            }
        }
    }
}

impl Error for ValidationError {}

impl Packet {
    /// バージョン、すべてのスコア、参照区間の順序を検査します。
    ///
    /// エラーを検出すると、その1件を返します。データの変更や候補の選択は行いません。
    /// 参照元文字列は Packet に含まれないため、区間が参照元内に収まるかは検査しません。
    /// 負の区間位置は `usize` 型で表現できません。
    ///
    /// 再帰呼び出しを使わずに走査し、検査のための固定の深さ制限は設けません。
    /// 所有する子だけを保持する現行の型では、安全な Rust で循環を構築できません。
    pub fn validate(&self) -> Result<(), ValidationError> {
        if self.version != PACKET_VERSION {
            return Err(ValidationError::UnsupportedVersion {
                found: self.version,
            });
        }

        let mut pending = vec![&self.root];
        while let Some(candidate) = pending.pop() {
            match candidate {
                Candidate::Text {
                    meta:
                        Some(TextMeta {
                            source_span: Some([start, end]),
                            ..
                        }),
                    ..
                } if start > end => {
                    return Err(ValidationError::ReversedSourceSpan {
                        start: *start,
                        end: *end,
                    });
                }
                Candidate::Text { .. } => {}
                Candidate::Sequence { items } => pending.extend(items.iter().rev()),
                Candidate::Choice { options } => {
                    for option in options {
                        if let Some(value) = option.score
                            && !value.is_finite()
                        {
                            return Err(ValidationError::NonFiniteScore { value });
                        }
                    }
                    pending.extend(options.iter().rev().map(|option| &option.value));
                }
            }
        }
        Ok(())
    }
}
