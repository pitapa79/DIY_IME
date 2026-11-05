use std::{error::Error, fmt};

use serde::{Deserialize, Deserializer};

use crate::{Candidate, Packet, ValidationError};

/// 初期 JSON コーデックの資源上限。候補木自体の論理的な深さ制限ではありません。
/// ルートを深さ1と数えます。再帰的なシリアライズのスタック使用量を制限します。
pub const MAX_JSON_CANDIDATE_DEPTH: usize = 32;

#[derive(Debug)]
pub enum PacketJsonError {
    Json(serde_json::Error),
    Validation(ValidationError),
    DepthLimit,
}

impl fmt::Display for PacketJsonError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Json(error) => write!(f, "invalid Packet JSON: {error}"),
            Self::Validation(error) => error.fmt(f),
            Self::DepthLimit => {
                write!(f, "JSON candidate depth exceeds {MAX_JSON_CANDIDATE_DEPTH}")
            }
        }
    }
}

impl Error for PacketJsonError {
    fn source(&self) -> Option<&(dyn Error + 'static)> {
        match self {
            Self::Json(error) => Some(error),
            Self::Validation(error) => Some(error),
            Self::DepthLimit => None,
        }
    }
}

// フィールド省略は許可し、明示的な null は仕様の型と異なるので拒否します。
pub(crate) fn optional_non_null<'de, D, T>(deserializer: D) -> Result<Option<T>, D::Error>
where
    D: Deserializer<'de>,
    T: Deserialize<'de>,
{
    match Option::<T>::deserialize(deserializer)? {
        Some(value) => Ok(Some(value)),
        None => Err(serde::de::Error::custom(
            "explicit null is not allowed; omit the field",
        )),
    }
}

impl Packet {
    /// 仕様の JSON 形式を読み、妥当性・資源上限を検査します。
    /// 未知フィールド・明示的な null・型の不一致・末尾の余分な JSON は拒否します。
    /// 外部入力には未検証の `serde_json::from_str::<Packet>` よりこの API を使います。
    pub fn from_json(json: &str) -> Result<Self, PacketJsonError> {
        let packet: Self = serde_json::from_str(json).map_err(PacketJsonError::Json)?;
        packet.check_json_limits()?;
        packet.validate().map_err(PacketJsonError::Validation)?;
        Ok(packet)
    }

    /// 検査後、任意フィールドを省略した仕様どおりの JSON を返します。
    /// 非有限スコアが JSON の null に変わることを検査で防ぎます。
    pub fn to_json(&self) -> Result<String, PacketJsonError> {
        self.check_json_limits()?;
        self.validate().map_err(PacketJsonError::Validation)?;
        serde_json::to_string_pretty(self).map_err(PacketJsonError::Json)
    }

    fn check_json_limits(&self) -> Result<(), PacketJsonError> {
        let mut pending = vec![(&self.root, 1)];
        while let Some((candidate, depth)) = pending.pop() {
            if depth > MAX_JSON_CANDIDATE_DEPTH {
                return Err(PacketJsonError::DepthLimit);
            }
            match candidate {
                Candidate::Text { .. } => {}
                Candidate::Sequence { items } => {
                    pending.extend(items.iter().map(|item| (item, depth + 1)));
                }
                Candidate::Choice { options } => {
                    pending.extend(options.iter().map(|option| (&option.value, depth + 1)));
                }
            }
        }
        Ok(())
    }
}
