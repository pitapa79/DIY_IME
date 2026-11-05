use diy_ime2_core::{Candidate, ChoiceOption, Packet, TextMeta, ValidationError};

fn text(value: &str) -> Candidate {
    Candidate::Text {
        text: value.into(),
        meta: None,
    }
}

#[test]
fn rejects_unsupported_versions() {
    for version in [0, 2, u32::MAX] {
        let packet = Packet {
            version,
            root: text("あ"),
        };
        assert_eq!(
            packet.validate(),
            Err(ValidationError::UnsupportedVersion { found: version })
        );
    }
}

#[test]
fn accepts_empty_text_sequence_and_choice() {
    for root in [
        text(""),
        Candidate::Sequence { items: vec![] },
        Candidate::Choice { options: vec![] },
    ] {
        assert_eq!(Packet::new(root).validate(), Ok(()));
    }
}

#[test]
fn accepts_finite_and_omitted_scores_without_normalizing() {
    let packet = Packet::new(Candidate::Choice {
        options: [Some(f64::MIN), None, Some(85.0), Some(f64::MAX)]
            .into_iter()
            .map(|score| ChoiceOption {
                value: text("候補"),
                score,
            })
            .collect(),
    });
    let original = packet.clone();
    assert_eq!(packet.validate(), Ok(()));
    assert_eq!(packet, original);
}

#[test]
fn rejects_non_finite_scores_inside_nested_candidates() {
    for value in [f64::NAN, f64::INFINITY, f64::NEG_INFINITY] {
        let packet = Packet::new(Candidate::Sequence {
            items: vec![
                text("有効な前置き"),
                Candidate::Choice {
                    options: vec![
                        ChoiceOption {
                            value: text("正常"),
                            score: Some(1.0),
                        },
                        ChoiceOption {
                            value: text("不正"),
                            score: Some(value),
                        },
                    ],
                },
            ],
        });
        assert!(matches!(
            packet.validate(),
            Err(ValidationError::NonFiniteScore { .. })
        ));
    }
}

#[test]
fn rejects_reversed_source_span_inside_choice_and_sequence() {
    let packet = Packet::new(Candidate::Choice {
        options: vec![ChoiceOption {
            value: Candidate::Sequence {
                items: vec![Candidate::Text {
                    text: "今日".into(),
                    meta: Some(TextMeta {
                        reading: Some("きょう".into()),
                        source_span: Some([3, 0]),
                    }),
                }],
            },
            score: None,
        }],
    });
    assert_eq!(
        packet.validate(),
        Err(ValidationError::ReversedSourceSpan { start: 3, end: 0 })
    );
}

#[test]
fn accepts_optional_metadata_and_spans_independent_of_output_length() {
    for source_span in [None, Some([0, 0]), Some([0, 4]), Some([4, 4])] {
        let packet = Packet::new(Candidate::Text {
            // 参照元「よしのや」の4コードポイントは、変換後「𠮷」の長さとは別。
            text: "𠮷".into(),
            meta: Some(TextMeta {
                reading: None,
                source_span,
            }),
        });
        assert_eq!(packet.validate(), Ok(()));
    }
}

#[test]
fn validates_deeply_nested_candidates_without_recursive_traversal() {
    let mut root = text("深い候補");
    for _ in 0..10_000 {
        root = Candidate::Sequence { items: vec![root] };
    }
    let packet = Packet::new(root);
    let result = packet.validate();

    // テストデータの解放も一層ずつ行い、標準の再帰的な drop と検査を切り分ける。
    let mut root = packet.root;
    while let Candidate::Sequence { mut items } = root {
        root = items.pop().unwrap();
    }
    assert_eq!(result, Ok(()));
}
