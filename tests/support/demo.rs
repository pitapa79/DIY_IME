//! Synthetic transport/control fixtures only; never registered by production composition.
//! 初期 E2E ノードと旧デモの互換実装。標準ノードの最終 API ではありません。
use std::sync::Arc;

use serde_json::{Value, json};

use diy_ime2_core::{
    Candidate, Packet,
    node::{
        CoreError, InputPort, Inputs, Node, NodeDefinition, OutputValue, Outputs, Port, Registry,
        RootConstraint, RunContext,
    },
    tree::map_text,
};

pub(super) fn config_keys<'a>(
    config: &'a Value,
    keys: &[&str],
) -> Result<&'a serde_json::Map<String, Value>, CoreError> {
    let object = config
        .as_object()
        .ok_or_else(|| CoreError::new("invalid-config", "expected an object"))?;
    if object.len() != keys.len() || keys.iter().any(|key| !object.contains_key(*key)) {
        return Err(CoreError::new(
            "invalid-config",
            format!("expected exactly these settings: {keys:?}"),
        ));
    }
    Ok(object)
}

pub(super) fn output(value: OutputValue) -> Outputs {
    [("out".into(), value)].into()
}
fn unary(kind: &str, root: RootConstraint) -> NodeDefinition {
    NodeDefinition::new(
        kind,
        1,
        vec![InputPort::required(Port::packet("in"))],
        vec![Port::packet("out").with_root(root)],
    )
}

pub struct PacketSource;
impl Node for PacketSource {
    fn definition(&self) -> NodeDefinition {
        let mut definition =
            NodeDefinition::new("demo.packet", 1, vec![], vec![Port::packet("out")]);
        definition.default_config =
            json!({"packet":{"version":1,"root":{"type":"text","text":""}}});
        definition.config_schema = json!({"type":"object","required":["packet"],"additionalProperties":false,"properties":{"packet":{"type":"object"}}});
        definition
    }
    fn validate_config(&self, config: &Value) -> Result<(), CoreError> {
        let object = config_keys(config, &["packet"])?;
        Packet::from_json(&object["packet"].to_string())
            .map_err(|error| CoreError::new("invalid-config", error.to_string()))?;
        Ok(())
    }
    fn execute(&self, _: RunContext<'_>, _: &Inputs, config: &Value) -> Result<Outputs, CoreError> {
        let packet = Packet::from_json(&config["packet"].to_string())
            .map_err(|error| CoreError::new("invalid-packet", error.to_string()))?;
        Ok(output(OutputValue::Data(Arc::new(packet))))
    }
}

pub struct Identity;
impl Node for Identity {
    fn definition(&self) -> NodeDefinition {
        unary("demo.identity", RootConstraint::Any)
    }
    fn execute(&self, _: RunContext<'_>, inputs: &Inputs, _: &Value) -> Result<Outputs, CoreError> {
        Ok(output(inputs["in"].clone()))
    }
}

pub struct Uppercase;
impl Node for Uppercase {
    fn definition(&self) -> NodeDefinition {
        unary("demo.uppercase", RootConstraint::Any)
    }
    fn execute(&self, _: RunContext<'_>, inputs: &Inputs, _: &Value) -> Result<Outputs, CoreError> {
        let OutputValue::Data(packet) = &inputs["in"] else {
            return Ok(output(OutputValue::NoOutput));
        };
        let root = map_text(&packet.root, |text, meta| Candidate::Text {
            text: text.to_uppercase(),
            meta: meta.cloned(),
        });
        Ok(output(OutputValue::Data(Arc::new(Packet::new(root)))))
    }
}

/// 同一 Choice の最大スコアを選ぶ例。スコア省略は設定値、同点は先の候補を採用。
/// 空 Choice を含む選択経路は NoOutput、空 Sequence は空文字列になります。
pub struct SelectText;
impl Node for SelectText {
    fn definition(&self) -> NodeDefinition {
        let mut definition = unary("demo.select-text", RootConstraint::Text);
        definition.default_config = json!({"unscored":0.0});
        definition.config_schema = json!({"type":"object","required":["unscored"],"additionalProperties":false,"properties":{"unscored":{"type":"number"}}});
        definition
    }
    fn validate_config(&self, config: &Value) -> Result<(), CoreError> {
        let object = config_keys(config, &["unscored"])?;
        if object["unscored"].as_f64().is_some_and(f64::is_finite) {
            Ok(())
        } else {
            Err(CoreError::new(
                "invalid-config",
                "unscored must be a finite number",
            ))
        }
    }
    fn execute(
        &self,
        _: RunContext<'_>,
        inputs: &Inputs,
        config: &Value,
    ) -> Result<Outputs, CoreError> {
        let OutputValue::Data(packet) = &inputs["in"] else {
            return Ok(output(OutputValue::NoOutput));
        };
        let unscored = config["unscored"]
            .as_f64()
            .ok_or_else(|| CoreError::new("invalid-config", "missing unscored score"))?;
        let mut pending = vec![&packet.root];
        let mut text = String::new();
        while let Some(candidate) = pending.pop() {
            match candidate {
                Candidate::Text { text: part, .. } => text.push_str(part),
                Candidate::Sequence { items } => pending.extend(items.iter().rev()),
                Candidate::Choice { options } => {
                    let mut best = None;
                    for option in options {
                        if best.is_none_or(|current: &diy_ime2_core::ChoiceOption| {
                            option.score.unwrap_or(unscored) > current.score.unwrap_or(unscored)
                        }) {
                            best = Some(option);
                        }
                    }
                    let Some(best) = best else {
                        return Ok(output(OutputValue::NoOutput));
                    };
                    pending.push(&best.value);
                }
            }
        }
        // 複数 Text のメタデータを無条件には合成しない。
        Ok(output(OutputValue::Data(Arc::new(Packet::new(
            Candidate::Text { text, meta: None },
        )))))
    }
}

pub struct Branch;
impl Node for Branch {
    fn definition(&self) -> NodeDefinition {
        let mut definition = NodeDefinition::new(
            "demo.branch",
            1,
            vec![InputPort::required(Port::packet("in"))],
            vec![Port::packet("yes"), Port::packet("no")],
        );
        definition.default_config = json!({"take":"yes"});
        definition.config_schema = json!({"type":"object","required":["take"],"additionalProperties":false,"properties":{"take":{"enum":["yes","no"]}}});
        definition
    }
    fn validate_config(&self, config: &Value) -> Result<(), CoreError> {
        let object = config_keys(config, &["take"])?;
        if matches!(object["take"].as_str(), Some("yes" | "no")) {
            Ok(())
        } else {
            Err(CoreError::new("invalid-config", "take must be yes or no"))
        }
    }
    fn execute(
        &self,
        _: RunContext<'_>,
        inputs: &Inputs,
        config: &Value,
    ) -> Result<Outputs, CoreError> {
        Ok(["yes", "no"]
            .into_iter()
            .map(|port| {
                (
                    port.into(),
                    if config["take"] == port {
                        inputs["in"].clone()
                    } else {
                        OutputValue::NoOutput
                    },
                )
            })
            .collect())
    }
}

pub fn register_demo_nodes(registry: &mut Registry) -> Result<(), CoreError> {
    registry.register(PacketSource)?;
    registry.register(Identity)?;
    registry.register(Uppercase)?;
    registry.register(SelectText)?;
    registry.register(Branch)
}

pub fn register_builtin_nodes(registry: &mut Registry) -> Result<(), CoreError> {
    register_demo_nodes(registry)?;
    registry.register(diy_ime2_core::nodes::runtime_input::RuntimeTextInput)?;
    registry.register(diy_ime2_core::builtins::initial_e2e::FixedInput)?;
    registry.register(OutputTextSource)?;
    registry.register(diy_ime2_core::builtins::initial_e2e::DebugOutput)
}

/// Synthetic source for the canonical Japanese-language output adapter contract.
pub struct OutputTextSource;
impl Node for OutputTextSource {
    fn definition(&self) -> NodeDefinition {
        let mut definition = NodeDefinition::new(
            "test.output-source",
            1,
            vec![],
            vec![
                Port::packet("out")
                    .with_language("ja")
                    .with_root(RootConstraint::Text),
            ],
        );
        definition.default_config = json!({"text":""});
        definition
    }
    fn validate_config(&self, config: &Value) -> Result<(), CoreError> {
        let object = config_keys(config, &["text"])?;
        if !object["text"].is_string() {
            return Err(CoreError::new("invalid-config", "text required"));
        }
        Ok(())
    }
    fn execute(&self, _: RunContext<'_>, _: &Inputs, config: &Value) -> Result<Outputs, CoreError> {
        self.validate_config(config)?;
        Ok(output(OutputValue::Data(Arc::new(Packet::new(
            Candidate::Text {
                text: config["text"].as_str().unwrap().into(),
                meta: None,
            },
        )))))
    }
}
