//! Count normalized JSON without recursively walking native Packet/Value trees.
use super::OwnedInvocation;
use crate::{
    Candidate, Packet,
    error_input::{ErrorAwareInputs, ErrorAwareValue, ErrorSet, RecoveryRecord},
    graph::ExecutionPlan,
    node::{CoreError, RuntimeInput},
};
use serde::Serialize;
use serde_json::{Value, json};
use std::{
    collections::BTreeMap,
    io::{self, Write},
    sync::Arc,
};

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub(crate) enum UnitKey {
    Config(usize, usize),
    Input(usize),
    Packet(usize),
}
pub(crate) struct Ownership {
    pub metadata: u64,
    pub units: BTreeMap<UnitKey, u64>,
}
impl Ownership {
    pub fn invocation(invocation: &OwnedInvocation) -> Result<Self, CoreError> {
        Self::build(
            &invocation.plan,
            invocation.index,
            invocation.input.as_ref(),
            &invocation.inputs,
            &invocation.input_recoveries,
            invocation.input_errors.as_ref(),
        )
    }
    pub fn initial(
        plan: &Arc<ExecutionPlan>,
        index: usize,
        input: Option<&Arc<RuntimeInput>>,
    ) -> Result<Self, CoreError> {
        let inputs = plan.nodes[index]
            .node
            .definition
            .inputs
            .iter()
            .map(|port| (port.port.id.clone(), ErrorAwareValue::NoOutput))
            .collect();
        let recoveries = plan.nodes[index]
            .node
            .definition
            .inputs
            .iter()
            .map(|port| (port.port.id.clone(), vec![]))
            .collect();
        Self::build(plan, index, input, &inputs, &recoveries, None)
    }
    fn build(
        plan: &Arc<ExecutionPlan>,
        index: usize,
        input: Option<&Arc<RuntimeInput>>,
        inputs: &ErrorAwareInputs,
        recoveries: &BTreeMap<String, Vec<RecoveryRecord>>,
        errors: Option<&ErrorSet>,
    ) -> Result<Self, CoreError> {
        let mut units = BTreeMap::new();
        units.insert(
            UnitKey::Config(Arc::as_ptr(plan) as usize, index),
            value_bytes(&plan.nodes[index].config)?,
        );
        if let Some(input) = input {
            units.insert(
                UnitKey::Input(Arc::as_ptr(input) as usize),
                scalar_bytes(input.as_ref())?,
            );
        }
        let mut bindings = BTreeMap::new();
        for (port, value) in inputs {
            let binding = match value {
                ErrorAwareValue::Data(packet) => {
                    let key = UnitKey::Packet(Arc::as_ptr(packet) as usize);
                    if let std::collections::btree_map::Entry::Vacant(entry) = units.entry(key) {
                        entry.insert(packet_bytes(packet)?);
                    }
                    json!({"status":"data"})
                }
                ErrorAwareValue::NoOutput => json!({"status":"no-output"}),
                ErrorAwareValue::Error(errors) => json!({"status":"error","causes":errors}),
            };
            bindings.insert(port, binding);
        }
        let metadata = scalar_bytes(
            &json!({"inputs":bindings,"hasRuntimeInput":input.is_some(),"inputRecoveries":recoveries,"inputErrors":errors}),
        )?;
        Ok(Self { metadata, units })
    }
}
struct Counter(u64);
impl Counter {
    fn add(&mut self, n: u64) -> Result<(), CoreError> {
        self.0 = self
            .0
            .checked_add(n)
            .ok_or_else(|| CoreError::new("busy", "queued input byte count overflow"))?;
        Ok(())
    }
}
impl Write for Counter {
    fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
        self.0 = self
            .0
            .checked_add(bytes.len() as u64)
            .ok_or_else(|| io::Error::other("JSON byte count overflow"))?;
        Ok(bytes.len())
    }
    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}
fn scalar_bytes(value: &impl Serialize) -> Result<u64, CoreError> {
    let mut counter = Counter(0);
    serde_json::to_writer(&mut counter, value)
        .map_err(|error| CoreError::new("invalid-queued-input", error.to_string()))?;
    Ok(counter.0)
}
pub(crate) fn value_bytes(value: &Value) -> Result<u64, CoreError> {
    let mut count = Counter(0);
    let mut stack = vec![value];
    while let Some(value) = stack.pop() {
        match value {
            Value::Array(values) => {
                count.add(2 + values.len().saturating_sub(1) as u64)?;
                stack.extend(values);
            }
            Value::Object(values) => {
                count.add(2 + values.len().saturating_sub(1) as u64)?;
                for (key, value) in values {
                    count.add(scalar_bytes(key)?)?;
                    count.add(1)?;
                    stack.push(value);
                }
            }
            _ => count.add(scalar_bytes(value)?)?,
        }
    }
    Ok(count.0)
}
pub(crate) fn packet_bytes(packet: &Packet) -> Result<u64, CoreError> {
    let mut count = Counter(0);
    count.add(scalar_bytes(&packet.version)?)?;
    count.add("{\"version\":,\"root\":}".len() as u64)?;
    let mut stack = vec![&packet.root];
    while let Some(candidate) = stack.pop() {
        match candidate {
            Candidate::Text { text, meta } => {
                count.add("{\"type\":\"text\",\"text\":}".len() as u64)?;
                count.add(scalar_bytes(text)?)?;
                if let Some(meta) = meta {
                    count.add(",\"meta\":".len() as u64)?;
                    count.add(scalar_bytes(meta)?)?;
                }
            }
            Candidate::Sequence { items } => {
                count.add(
                    "{\"type\":\"sequence\",\"items\":[]}".len() as u64
                        + items.len().saturating_sub(1) as u64,
                )?;
                stack.extend(items);
            }
            Candidate::Choice { options } => {
                count.add(
                    "{\"type\":\"choice\",\"options\":[]}".len() as u64
                        + options.len().saturating_sub(1) as u64,
                )?;
                for option in options {
                    count.add("{\"value\":}".len() as u64)?;
                    if let Some(score) = option.score {
                        count.add(",\"score\":".len() as u64)?;
                        count.add(scalar_bytes(&score)?)?;
                    }
                    stack.push(&option.value);
                }
            }
        }
    }
    Ok(count.0)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn byte_counter_overflow_does_not_wrap_or_partially_change_the_count() {
        let mut count = Counter(u64::MAX - 1);
        assert!(count.write(b"xx").is_err());
        assert_eq!(count.0, u64::MAX - 1);
        assert_eq!(count.write(b"x").unwrap(), 1);
        assert_eq!(count.0, u64::MAX);
        assert_eq!(count.add(1).unwrap_err().code, "busy");
        assert_eq!(count.0, u64::MAX);
    }
    #[test]
    fn iterative_packet_and_value_counts_match_normalized_serde_json() {
        for source in [
            r#"{"version":1,"root":{"type":"text","text":"😀\n\"","meta":{"reading":"かな","sourceSpan":[1,3]}}}"#,
            r#"{"version":1,"root":{"type":"sequence","items":[]}}"#,
            r#"{"version":1,"root":{"type":"choice","options":[{"value":{"type":"text","text":"x"},"score":-0.5},{"value":{"type":"sequence","items":[{"type":"text","text":""}]}}]}}"#,
        ] {
            let packet = Packet::from_json(source).unwrap();
            assert_eq!(
                packet_bytes(&packet).unwrap(),
                serde_json::to_vec(&packet).unwrap().len() as u64
            );
            let value: Value = serde_json::from_str(source).unwrap();
            assert_eq!(
                value_bytes(&value).unwrap(),
                serde_json::to_vec(&value).unwrap().len() as u64
            );
        }
    }
    #[test]
    fn counting_native_deep_data_does_not_impose_the_transport_depth_limit() {
        let mut root = Candidate::Text {
            text: "x".into(),
            meta: None,
        };
        for _ in 0..300 {
            root = Candidate::Sequence { items: vec![root] };
        }
        let packet = Packet::new(root);
        assert!(packet.to_json().is_err());
        assert_eq!(
            packet_bytes(&packet).unwrap(),
            serde_json::to_vec(&packet).unwrap().len() as u64
        );
        let mut config = Value::Bool(true);
        for _ in 0..300 {
            config = Value::Array(vec![config]);
        }
        assert_eq!(value_bytes(&config).unwrap(), 604);
    }
}
