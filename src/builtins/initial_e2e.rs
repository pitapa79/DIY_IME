//! Canonical initial-E2E source/sink adapters; processing is provided by official Wasm.
use super::{config_keys, output};
use crate::{
    Candidate, Packet,
    node::{
        CoreError, InputPort, Inputs, Node, NodeDefinition, OutputValue, Outputs, Port,
        RootConstraint, RunContext,
    },
};
use serde_json::{Value, json};
use std::sync::Arc;
pub const ROMAJI: &str = "honjitunokousuikakurituha30%desu";
pub const HIRAGANA: &str = "ほんじつのこうすいかくりつは30%です";
pub const EXPECTED: &str = "本日の降水確率は30%です";

fn port(id: &str, language: &str, root: RootConstraint) -> Port {
    Port::packet(id).with_language(language).with_root(root)
}
fn unary(
    kind: &str,
    input: RootConstraint,
    result: RootConstraint,
    input_language: &str,
) -> NodeDefinition {
    NodeDefinition::new(
        kind,
        1,
        vec![InputPort::required(port("in", input_language, input))],
        vec![port("out", "ja", result)],
    )
}
fn packet(inputs: &Inputs) -> Result<Option<&Packet>, CoreError> {
    match inputs.get("in") {
        Some(OutputValue::Data(packet)) => {
            packet
                .validate()
                .map_err(|error| CoreError::new("invalid-packet", error.to_string()))?;
            Ok(Some(packet))
        }
        Some(OutputValue::NoOutput) => Ok(None),
        None => Err(CoreError::new("missing-input", "in is required")),
    }
}
fn text(inputs: &Inputs) -> Result<Option<&str>, CoreError> {
    match packet(inputs)? {
        Some(Packet {
            root: Candidate::Text { text, .. },
            ..
        }) => Ok(Some(text)),
        Some(_) => Err(CoreError::new(
            "invalid-root",
            "this node requires a Text root",
        )),
        None => Ok(None),
    }
}
fn data(root: Candidate) -> Outputs {
    output(OutputValue::Data(Arc::new(Packet::new(root))))
}

pub struct FixedInput;
impl Node for FixedInput {
    fn definition(&self) -> NodeDefinition {
        let mut definition = NodeDefinition::new(
            "e2e.input",
            1,
            vec![],
            vec![port("out", "ja-Latn", RootConstraint::Text)],
        );
        definition.default_config = json!({"mode":"fixed","text":ROMAJI});
        definition.config_schema = json!({"type":"object","required":["mode","text"],"additionalProperties":false,"properties":{"mode":{"enum":["fixed"]},"text":{"type":"string","maxLength":4096}}});
        definition
    }
    fn validate_config(&self, config: &Value) -> Result<(), CoreError> {
        let object = config_keys(config, &["mode", "text"])?;
        if object["mode"] != "fixed"
            || object["text"]
                .as_str()
                .is_none_or(|text| text.chars().count() > 4096)
        {
            return Err(CoreError::new(
                "invalid-config",
                "Input requires mode=fixed and a string of at most 4096 code points",
            ));
        }
        Ok(())
    }
    fn execute(&self, _: RunContext<'_>, _: &Inputs, config: &Value) -> Result<Outputs, CoreError> {
        self.validate_config(config)?;
        Ok(data(Candidate::Text {
            text: config["text"].as_str().unwrap().into(),
            meta: None,
        }))
    }
}

pub struct DebugOutput;
impl Node for DebugOutput {
    fn definition(&self) -> NodeDefinition {
        unary(
            "e2e.output",
            RootConstraint::Text,
            RootConstraint::Text,
            "ja",
        )
    }
    fn execute(
        &self,
        context: RunContext<'_>,
        inputs: &Inputs,
        _: &Value,
    ) -> Result<Outputs, CoreError> {
        let Some(value) = text(inputs)? else {
            return Ok(output(OutputValue::NoOutput));
        };
        // stdout は CLI / 常駐プロトコル専用。ログは stderr、画面用データは out へ返す。
        eprintln!(
            "[Output:{} plan={} run={}] {}",
            context.node_id, context.plan_version, context.run_id, value
        );
        Ok(output(inputs["in"].clone()))
    }
}
