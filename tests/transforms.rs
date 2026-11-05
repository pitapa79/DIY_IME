#[allow(dead_code)]
#[path = "support/demo.rs"]
mod test_nodes;
use diy_ime2_core::{
    Candidate, ChoiceOption, Packet,
    graph::{ExecutionPlan, Graph},
    node::Registry,
    runtime::{Engine, ResolvedValue, RunReport},
    tree::map_text,
};
use serde_json::{Value, json};
use test_nodes::register_demo_nodes;

#[test]
fn map_text_preserves_structure_order_scores_and_metadata() {
    let original = json!({"version":1,"root":{"type":"sequence","items":[
        {"type":"choice","options":[
            {"score":-1.0,"value":{"type":"text","text":"a","meta":{"reading":"えー","sourceSpan":[0,1]}}},
            {"value":{"type":"sequence","items":[{"type":"text","text":"b"},{"type":"text","text":"c"}]}}
        ]},
        {"type":"choice","options":[]},
        {"type":"sequence","items":[]}
    ]}});
    let packet = Packet::from_json(&original.to_string()).unwrap();
    let mapped = Packet::new(map_text(&packet.root, |text, meta| Candidate::Text {
        text: text.to_uppercase(),
        meta: meta.cloned(),
    }));
    let mut expected = original.clone();
    expected["root"]["items"][0]["options"][0]["value"]["text"] = json!("A");
    expected["root"]["items"][0]["options"][1]["value"]["items"][0]["text"] = json!("B");
    expected["root"]["items"][0]["options"][1]["value"]["items"][1]["text"] = json!("C");
    assert_eq!(
        serde_json::from_str::<Value>(&mapped.to_json().unwrap()).unwrap(),
        expected
    );
    assert_eq!(
        serde_json::from_str::<Value>(&packet.to_json().unwrap()).unwrap(),
        original
    );
}

#[test]
fn map_text_can_replace_text_with_a_choice_without_reprocessing_it() {
    let mut calls = 0;
    let output = map_text(
        &Candidate::Text {
            text: "a".into(),
            meta: None,
        },
        |text, _| {
            calls += 1;
            Candidate::Choice {
                options: vec![ChoiceOption {
                    value: Candidate::Text {
                        text: text.into(),
                        meta: None,
                    },
                    score: None,
                }],
            }
        },
    );
    assert_eq!(calls, 1);
    assert!(matches!(output, Candidate::Choice { .. }));
}

#[test]
fn map_text_handles_deep_trees_without_recursive_traversal() {
    fn discard(root: Candidate) {
        let mut pending = vec![root];
        while let Some(candidate) = pending.pop() {
            match candidate {
                Candidate::Text { .. } => {}
                Candidate::Sequence { items } => pending.extend(items),
                Candidate::Choice { options } => {
                    pending.extend(options.into_iter().map(|option| option.value))
                }
            }
        }
    }
    let mut root = Candidate::Text {
        text: "a".into(),
        meta: None,
    };
    for index in 0..10_000 {
        root = if index % 2 == 0 {
            Candidate::Sequence { items: vec![root] }
        } else {
            Candidate::Choice {
                options: vec![ChoiceOption {
                    value: root,
                    score: None,
                }],
            }
        };
    }
    let mapped = map_text(&root, |_, _| Candidate::Text {
        text: "b".into(),
        meta: None,
    });
    let mut current = &mapped;
    loop {
        match current {
            Candidate::Text { text, .. } => {
                assert_eq!(text, "b");
                break;
            }
            Candidate::Sequence { items } => current = &items[0],
            Candidate::Choice { options } => current = &options[0].value,
        }
    }
    discard(root);
    discard(mapped);
}

fn select(root: Value, unscored: f64) -> RunReport {
    let graph = Graph::from_json(&json!({"version":1,"nodes":[
        {"id":"input","kind":"demo.packet","version":1,"config":{"packet":{"version":1,"root":root}}},
        {"id":"select","kind":"demo.select-text","version":1,"config":{"unscored":unscored}}
    ],"edges":[{"from":{"node":"input","port":"out"},"to":{"node":"select","port":"in"}}]}).to_string()).unwrap();
    let mut registry = Registry::new();
    register_demo_nodes(&mut registry).unwrap();
    Engine::new(ExecutionPlan::compile(&graph, &registry).unwrap())
        .run()
        .unwrap()
}

fn result(report: &RunReport) -> &str {
    let Candidate::Text { text, .. } = &report.packet("select", "out").unwrap().root else {
        panic!("expected Text")
    };
    text
}

#[test]
fn selection_uses_local_scores_and_preserves_sequence_order() {
    let report = select(
        json!({"type":"sequence","items":[
            {"type":"choice","options":[{"score":1,"value":{"type":"text","text":"A"}},{"score":2,"value":{"type":"text","text":"B"}}]},
            {"type":"choice","options":[{"score":-4,"value":{"type":"text","text":"X"}},{"score":-2,"value":{"type":"text","text":"Y"}}]}
        ]}),
        0.0,
    );
    assert_eq!(result(&report), "BY");
}

#[test]
fn ties_choose_first_and_unscored_policy_is_configurable() {
    let tied = select(
        json!({"type":"choice","options":[
            {"score":1,"value":{"type":"text","text":"first"}}, {"score":1,"value":{"type":"text","text":"second"}}
        ]}),
        0.0,
    );
    assert_eq!(result(&tied), "first");
    let root = json!({"type":"choice","options":[
        {"score":-1,"value":{"type":"text","text":"scored"}}, {"value":{"type":"text","text":"unscored"}}
    ]});
    assert_eq!(result(&select(root.clone(), 0.0)), "unscored");
    assert_eq!(result(&select(root, -10.0)), "scored");
}

#[test]
fn empty_choice_and_empty_sequence_remain_distinct() {
    let no_candidates = select(json!({"type":"choice","options":[]}), 0.0);
    assert!(no_candidates.succeeded());
    assert!(matches!(
        no_candidates.nodes["select"].outputs["out"],
        ResolvedValue::NoOutput
    ));
    let empty_sequence = select(json!({"type":"sequence","items":[]}), 0.0);
    assert_eq!(result(&empty_sequence), "");
}
