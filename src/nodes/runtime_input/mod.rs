//! 実行時の文章スナップショットをグラフへ渡すsource。
use crate::{
    Candidate, Packet,
    node::{
        CoreError, Inputs, Node, NodeDefinition, OutputValue, Outputs, Port, RootConstraint,
        RunContext,
    },
};
use serde_json::Value;
use std::sync::Arc;

pub struct RuntimeTextInput;
impl Node for RuntimeTextInput {
    fn definition(&self) -> NodeDefinition {
        NodeDefinition::new(
            "input.text",
            1,
            vec![],
            vec![
                Port::packet("out")
                    .with_language("ja-Latn")
                    .with_root(RootConstraint::Text),
            ],
        )
    }
    fn execute(
        &self,
        context: RunContext<'_>,
        _: &Inputs,
        config: &Value,
    ) -> Result<Outputs, CoreError> {
        self.validate_config(config)?;
        let input = context.input.ok_or_else(|| {
            CoreError::new(
                "runtime-input-missing",
                "run requires a runtime input snapshot",
            )
        })?;
        input.validate()?;
        Ok([(
            "out".into(),
            OutputValue::Data(Arc::new(Packet::new(Candidate::Text {
                text: input.text.clone(),
                meta: None,
            }))),
        )]
        .into())
    }
}
