use diy_ime2_core::{Candidate, ChoiceOption, Packet, TextMeta, ValidationError};

fn main() -> Result<(), ValidationError> {
    // 「今日」と「きょう」を選択肢として保持し、その後ろに「です」を置く例。
    let packet = Packet::new(Candidate::Sequence {
        items: vec![
            Candidate::Choice {
                options: vec![
                    ChoiceOption {
                        value: Candidate::Text {
                            text: "今日".into(),
                            meta: Some(TextMeta {
                                reading: Some("きょう".into()),
                                source_span: Some([0, 3]),
                            }),
                        },
                        score: Some(85.0),
                    },
                    ChoiceOption {
                        value: Candidate::Text {
                            text: "きょう".into(),
                            meta: None,
                        },
                        score: None,
                    },
                ],
            },
            Candidate::Text {
                text: "です".into(),
                meta: None,
            },
        ],
    });

    // 参照元は「きょうです」。ここでは候補の選択や文字列への連結は行わない。
    packet.validate()?;
    println!("妥当性検査: OK\n{packet:#?}");
    Ok(())
}
