//! ノード作者が明示的に利用する、候補木の走査・再構築ライブラリ。
use crate::{Candidate, ChoiceOption, TextMeta};

/// 各 Text を変換結果に置き換え、Sequence / Choice の形・順序・スコアを保持します。
/// コールバックが返した新しい候補は再走査しません。入力の破壊的変更はありません。
pub fn map_text(
    root: &Candidate,
    mut transform: impl FnMut(&str, Option<&TextMeta>) -> Candidate,
) -> Candidate {
    enum Frame<'a> {
        Visit(&'a Candidate),
        Sequence(usize),
        Choice(&'a [ChoiceOption]),
    }
    let mut frames = vec![Frame::Visit(root)];
    let mut values = Vec::new();
    while let Some(frame) = frames.pop() {
        match frame {
            Frame::Visit(Candidate::Text { text, meta }) => {
                values.push(transform(text, meta.as_ref()))
            }
            Frame::Visit(Candidate::Sequence { items }) => {
                frames.push(Frame::Sequence(items.len()));
                frames.extend(items.iter().rev().map(Frame::Visit));
            }
            Frame::Visit(Candidate::Choice { options }) => {
                frames.push(Frame::Choice(options));
                frames.extend(
                    options
                        .iter()
                        .rev()
                        .map(|option| Frame::Visit(&option.value)),
                );
            }
            Frame::Sequence(count) => {
                let items = values.split_off(values.len() - count);
                values.push(Candidate::Sequence { items });
            }
            Frame::Choice(original) => {
                let children = values.split_off(values.len() - original.len());
                let options = children
                    .into_iter()
                    .zip(original)
                    .map(|(value, option)| ChoiceOption {
                        value,
                        score: option.score,
                    })
                    .collect();
                values.push(Candidate::Choice { options });
            }
        }
    }
    values
        .pop()
        .expect("a candidate always produces one result")
}
