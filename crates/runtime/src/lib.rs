//! DIY_IME2 の文章変換用データ型。
//!
//! 文章候補データ、妥当性検査、JSON 入出力を提供します。

pub mod error_input;
pub mod execution_control;
pub mod graph;
mod json;
pub mod node;
pub mod runtime;
pub mod session;
pub mod tree;
mod validation;

pub use json::{MAX_JSON_CANDIDATE_DEPTH, PacketJsonError};
use serde::{Deserialize, Serialize};
pub use validation::ValidationError;

/// 現行の文章変換用 Packet の形式バージョン。
pub const PACKET_VERSION: u32 = 1;

/// 文章変換ポートで受け渡す候補木。
///
/// 構築・変更時の妥当性検査は行いません。受け渡し前に [`Self::validate`] で検査できます。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Packet {
    pub version: u32,
    pub root: Candidate,
}

impl Packet {
    /// 指定した候補木を、現行バージョンの Packet に包みます。
    ///
    /// ```
    /// use diy_ime2_runtime::{Candidate, Packet};
    ///
    /// let packet = Packet::new(Candidate::Text {
    ///     text: "こんにちは".into(),
    ///     meta: None,
    /// });
    ///
    /// assert_eq!(packet.version, 1);
    /// ```
    pub fn new(root: Candidate) -> Self {
        Self {
            version: PACKET_VERSION,
            root,
        }
    }
}

/// 文章候補。各種類を自由に入れ子にできます。
///
/// この型は文章用です。将来のイベント・状態などの型は別途定義します。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "lowercase", deny_unknown_fields)]
pub enum Candidate {
    /// 文字列。空文字列も表現できます。
    Text {
        text: String,
        #[serde(
            default,
            skip_serializing_if = "Option::is_none",
            deserialize_with = "json::optional_non_null"
        )]
        meta: Option<TextMeta>,
    },
    /// 順序付きの連結。空配列も表現できます。
    Sequence { items: Vec<Candidate> },
    /// 選択肢。空配列は「候補なし」を表し、エラーや NoOutput とは区別します。
    Choice { options: Vec<ChoiceOption> },
}

/// Text に必要な場合だけ付ける、読みと参照元の位置。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct TextMeta {
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        deserialize_with = "json::optional_non_null"
    )]
    pub reading: Option<String>,
    /// Unicode コードポイント単位の半開区間 [start, end)。バイト位置ではありません。
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        deserialize_with = "json::optional_non_null"
    )]
    pub source_span: Option<[usize; 2]>,
}

/// 同一 Choice 内の選択肢と、その任意のスコア。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ChoiceOption {
    pub value: Candidate,
    /// 仕様上は有限数で、大きいほど優先。未設定も有効です。
    /// 有限性は [`Packet::validate`] で検査します。選択・並べ替えは行いません。
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        deserialize_with = "json::optional_non_null"
    )]
    pub score: Option<f64>,
}

/// Hostが明示するローカルファイル資源。
pub mod resources;

// Legacy demo implementations are test fixtures only, never production dependencies.
#[cfg(test)]
#[path = "../tests/support/demo.rs"]
mod builtins;
