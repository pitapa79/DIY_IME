//! 開発キットの検証専用。製品ノードとしての採用・標準登録を意味しません。
use crate::{
    Candidate,
    node::{
        CoreError, InputPort, Inputs, Node, NodeDefinition, OutputValue, Outputs, Port,
        RootConstraint, RunContext,
    },
};
use serde_json::Value;

pub const KIND: &str = "kit.__NAME__";

#[derive(Debug, Clone, Copy)]
pub struct ExampleNode;

impl Node for ExampleNode {
    fn definition(&self) -> NodeDefinition {
        NodeDefinition::new(
            KIND,
            1,
            vec![InputPort::required(
                Port::packet("in")
                    .with_language("ja")
                    .with_root(RootConstraint::Text),
            )],
            vec![
                Port::packet("out")
                    .with_language("ja")
                    .with_root(RootConstraint::Text),
            ],
        )
    }

    // 既定の validate_config は {} のみ許可。独自設定は承認済み契約に合わせて実装する。
    fn execute(
        &self,
        _: RunContext<'_>,
        inputs: &Inputs,
        config: &Value,
    ) -> Result<Outputs, CoreError> {
        self.validate_config(config)?;
        let output = match inputs.get("in") {
            Some(OutputValue::Data(packet)) => {
                packet
                    .validate()
                    .map_err(|error| CoreError::new("invalid-packet", error.to_string()))?;
                if !matches!(packet.root, Candidate::Text { .. }) {
                    return Err(CoreError::new("invalid-root", "expected Text"));
                }
                // 雛形の唯一の処理: Text と meta を読み取り専用のまま渡す。
                OutputValue::Data(packet.clone())
            }
            Some(OutputValue::NoOutput) => OutputValue::NoOutput,
            None => return Err(CoreError::new("missing-input", "required input: in")),
        };
        Ok([("out".into(), output)].into())
    }
}
