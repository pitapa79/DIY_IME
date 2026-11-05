//! Target-scoped terminal projection. It does not dispatch or prioritize jobs.
use super::{ResolvedPort, RunDelivery, RunKey, TerminalReason, TerminalResult};
use crate::{
    error_input::{CauseId, ErrorSet, RecoveryRecord, merge_errors},
    graph::{Endpoint, ExecutionPlan},
    node::CoreError,
    runtime::{Diagnostic, NodeExecution, NodeStatus, ResolvedValue},
};
use serde_json::{Value, json};
use std::{
    collections::{BTreeMap, BTreeSet},
    sync::Arc,
};

/// A trusted host's collection of results issued for one accepted run.
pub struct ExecutionResults {
    pub(crate) key: RunKey,
    pub(crate) plan: Arc<ExecutionPlan>,
    pub(crate) targets: Vec<Endpoint>,
    scope: Vec<usize>,
    records: BTreeMap<String, NodeExecution>,
    discarded: BTreeSet<String>,
    terminal: Option<TerminalResult>,
}
impl ExecutionResults {
    pub(crate) fn new(key: RunKey, plan: Arc<ExecutionPlan>, targets: Vec<Endpoint>) -> Self {
        let scope = target_scope(&plan, &targets);
        Self {
            key,
            plan,
            targets,
            scope,
            records: BTreeMap::new(),
            discarded: BTreeSet::new(),
            terminal: None,
        }
    }
    pub fn key(&self) -> &RunKey {
        &self.key
    }
    pub(crate) fn execution(&self, node: &str) -> Option<&NodeExecution> {
        self.records.get(node)
    }
    /// Stable plan order, with shared ancestors occurring once.
    pub fn required_nodes(&self) -> impl Iterator<Item = &str> {
        self.scope.iter().map(|&i| self.plan.nodes[i].id.as_str())
    }
    fn require_open_node(&self, node: &str) -> Result<usize, CoreError> {
        if self.terminal.is_some() {
            return Err(CoreError::new(
                "run-terminal",
                "terminal results are immutable",
            ));
        }
        let index = self
            .scope
            .iter()
            .copied()
            .find(|&index| self.plan.nodes[index].id == node)
            .ok_or_else(|| {
                CoreError::new("unrequested-node", "node is outside the target ancestors")
            })?;
        if self.records.contains_key(node) || self.discarded.contains(node) {
            return Err(CoreError::new(
                "duplicate-result",
                "node already resolved or discarded",
            ));
        }
        Ok(index)
    }
    pub fn record(&mut self, node: &str, execution: NodeExecution) -> Result<(), CoreError> {
        self.validate_record(node, &execution)?;
        self.record_validated(node, execution)
    }
    pub(crate) fn validate_record(
        &self,
        node: &str,
        execution: &NodeExecution,
    ) -> Result<(), CoreError> {
        let index = self.require_open_node(node)?;
        let definition = &self.plan.nodes[index].node.definition;
        if execution.outputs.len() != definition.outputs.len() {
            return Err(invalid("output set does not match declaration"));
        }
        for port in &definition.outputs {
            match execution.outputs.get(&port.id) {
                Some(ResolvedValue::Data(packet)) => {
                    packet.validate().map_err(|e| invalid(&e.to_string()))?;
                    if !port.root.accepts(&packet.root) {
                        return Err(invalid("output root violates declaration"));
                    }
                    if execution.error_outputs.contains_key(&port.id) {
                        return Err(invalid("Data cannot also be Error"));
                    }
                }
                Some(ResolvedValue::NoOutput) => {
                    if execution.error_outputs.contains_key(&port.id) {
                        return Err(invalid("NoOutput cannot also be Error"));
                    }
                }
                Some(ResolvedValue::Error(diagnostic)) => {
                    self.check_diagnostic(diagnostic)?;
                    let errors = execution
                        .error_outputs
                        .get(&port.id)
                        .ok_or_else(|| invalid("Error output needs causes"))?;
                    if errors.causes()[0].diagnostic != *diagnostic {
                        return Err(invalid("error diagnostic differs from first cause"));
                    }
                }
                None => return Err(invalid("missing declared output")),
            }
        }
        if let Some(diagnostic) = &execution.diagnostic {
            self.check_diagnostic(diagnostic)?;
        }
        for errors in execution
            .error_outputs
            .values()
            .chain(execution.input_errors.iter())
        {
            for cause in errors.causes() {
                self.check_cause(&cause.id)?;
                self.check_diagnostic(&cause.diagnostic)?;
            }
        }
        if execution
            .error_outputs
            .keys()
            .any(|port| !matches!(execution.outputs.get(port), Some(ResolvedValue::Error(_))))
        {
            return Err(invalid("error cause entry must name an Error output"));
        }
        for record in execution
            .recoveries
            .iter()
            .chain(execution.output_recoveries.values().flatten())
        {
            self.check_recovery(record)?;
        }
        if execution
            .recoveries
            .iter()
            .any(|record| record.node_id != node)
        {
            return Err(invalid("own recovery record names another node"));
        }
        if execution
            .output_recoveries
            .keys()
            .any(|port| !matches!(execution.outputs.get(port), Some(ResolvedValue::Data(_))))
        {
            return Err(invalid("output recoveries must name Data outputs"));
        }
        let failure = matches!(execution.status, NodeStatus::Failed | NodeStatus::Blocked);
        if failure != execution.diagnostic.is_some()
            || (failure
                && execution
                    .outputs
                    .values()
                    .any(|value| !matches!(value, ResolvedValue::Error(_))))
            || (execution.status == NodeStatus::Skipped
                && execution
                    .outputs
                    .values()
                    .any(|value| !matches!(value, ResolvedValue::NoOutput)))
        {
            return Err(invalid("status, diagnostic and outputs disagree"));
        }
        Ok(())
    }
    // Only Graph-issued opaque prepared tokens reach this boundary without
    // revalidating immutable Packet trees. Still reject terminal/duplicate nodes.
    pub(crate) fn record_validated(
        &mut self,
        node: &str,
        execution: NodeExecution,
    ) -> Result<(), CoreError> {
        self.require_open_node(node)?;
        self.records.insert(node.into(), execution);
        Ok(())
    }
    pub fn discard_unneeded(&mut self, node: &str) -> Result<(), CoreError> {
        self.require_open_node(node)?;
        self.discarded.insert(node.into());
        Ok(())
    }
    fn check_cause(&self, cause: &CauseId) -> Result<(), CoreError> {
        if cause.run_id != self.key.run_id.get()
            || cause.plan_version != self.key.plan_version.get()
            || !self
                .scope
                .iter()
                .any(|&index| self.plan.nodes[index].id == cause.node_id)
        {
            return Err(invalid("cause belongs to another run or unrequested node"));
        }
        Ok(())
    }
    fn check_diagnostic(&self, diagnostic: &Diagnostic) -> Result<(), CoreError> {
        self.check_cause(&CauseId {
            run_id: diagnostic.run_id,
            plan_version: diagnostic.plan_version,
            node_id: diagnostic.node_id.clone(),
        })
    }
    fn check_recovery(&self, record: &RecoveryRecord) -> Result<(), CoreError> {
        if !self
            .scope
            .iter()
            .any(|&index| self.plan.nodes[index].id == record.node_id)
        {
            return Err(invalid("unrequested recovery node"));
        }
        let node = self
            .plan
            .nodes
            .iter()
            .find(|node| node.id == record.node_id)
            .ok_or_else(|| invalid("unknown recovery node"))?;
        if !node
            .node
            .definition
            .inputs
            .iter()
            .any(|port| port.port.id == record.input)
            || !node
                .node
                .definition
                .outputs
                .iter()
                .any(|port| port.id == record.output)
            || record.causes.is_empty()
        {
            return Err(invalid("invalid recovery endpoints or causes"));
        }
        for cause in &record.causes {
            self.check_cause(cause)?;
        }
        Ok(())
    }
    fn cause_id(&self, id: &CauseId) -> Value {
        json!({"sessionEpoch":self.key.session_epoch,"runId":id.run_id.to_string(),"planVersion":id.plan_version.to_string(),"nodeId":id.node_id})
    }
    fn diagnostic(&self, diagnostic: &Diagnostic) -> Value {
        json!({"sessionEpoch":self.key.session_epoch,"runId":diagnostic.run_id.to_string(),"planVersion":diagnostic.plan_version.to_string(),"nodeId":diagnostic.node_id,"port":diagnostic.port,"error":diagnostic.error})
    }
    fn recovery(&self, record: &RecoveryRecord) -> Value {
        json!({"nodeId":record.node_id,"input":record.input,"output":record.output,"causes":record.causes.iter().map(|id|self.cause_id(id)).collect::<Vec<_>>()})
    }
    /// The host chooses completion/cancellation/expiry. This method supplies no clock
    /// and does not decide which pending branch should become unnecessary.
    pub fn preview(&self, reason: TerminalReason) -> Result<TerminalResult, CoreError> {
        self.project(reason)
    }
    pub(crate) fn project(&self, reason: TerminalReason) -> Result<TerminalResult, CoreError> {
        if let Some(result) = &self.terminal {
            return Ok(result.clone());
        }
        if reason == TerminalReason::Failed {
            return Err(invalid(
                "host must supply completed/cancelled/expired, not invent a Core failure",
            ));
        }
        let abort = reason != TerminalReason::Completed;
        if !abort
            && self
                .targets
                .iter()
                .any(|target| !self.records.contains_key(&target.node))
        {
            return Err(CoreError::new(
                "run-pending",
                "every requested target must be resolved",
            ));
        }
        let label = if reason == TerminalReason::Expired {
            "expired"
        } else {
            "cancelled"
        };
        let mut result =
            TerminalResult::completed(self.targets.clone(), vec![], RunDelivery::Complete);
        result.reason = reason;
        let mut causes = None;
        let mut recovery_records = vec![];
        for &index in &self.scope {
            let node = &self.plan.nodes[index];
            let record = self.records.get(&node.id);
            let status = match record.map(|record| record.status) {
                Some(NodeStatus::Executed) => "executed",
                Some(NodeStatus::Skipped) => "skipped",
                Some(NodeStatus::Failed) => "failed",
                Some(NodeStatus::Blocked) => "blocked",
                None => "discarded",
            };
            result.nodes.push(json!({"nodeId":node.id,"status":status,"diagnostic":record.and_then(|record|record.diagnostic.as_ref()).map(|d|self.diagnostic(d))}));
            if let Some(record) = record {
                if let Some(diagnostic) = &record.diagnostic
                    && record.status == NodeStatus::Failed
                {
                    merge_errors(&mut causes, &ErrorSet::new(diagnostic.clone()));
                }
                for errors in record
                    .input_errors
                    .iter()
                    .chain(record.error_outputs.values())
                {
                    merge_errors(&mut causes, errors);
                }
                for recovery in &record.recoveries {
                    if !recovery_records.contains(recovery) {
                        recovery_records.push(recovery.clone());
                    }
                }
            }
            for port in &node.node.definition.outputs {
                let value = if abort {
                    json!({"status":"discarded","reason":label})
                } else {
                    match record.and_then(|record| record.outputs.get(&port.id)) {
                        Some(ResolvedValue::Data(packet)) => {
                            let packet = match packet
                                .to_json()
                                .ok()
                                .and_then(|json| serde_json::from_str::<Value>(&json).ok())
                            {
                                Some(packet) => packet,
                                None => return Ok(report_failed(&self.targets)),
                            };
                            json!({"status":"data","packet":packet})
                        }
                        Some(ResolvedValue::NoOutput) => json!({"status":"no-output"}),
                        Some(ResolvedValue::Error(_)) => {
                            json!({"status":"error","causes":record.expect("resolved node").error_outputs[&port.id].causes().iter().map(|cause|self.cause_id(&cause.id)).collect::<Vec<_>>()})
                        }
                        None => json!({"status":"discarded","reason":"unneeded"}),
                    }
                };
                result.ports.push(ResolvedPort {
                    endpoint: Endpoint::new(&node.id, &port.id),
                    value,
                });
            }
        }
        result.causes=causes.map(|errors:ErrorSet|errors.causes().iter().map(|cause|json!({"id":self.cause_id(&cause.id),"diagnostic":self.diagnostic(&cause.diagnostic),"paths":cause.paths})).collect()).unwrap_or_default();
        result.recoveries = recovery_records
            .iter()
            .map(|record| self.recovery(record))
            .collect();
        result.delivery = if abort {
            if reason == TerminalReason::Expired {
                RunDelivery::Expired
            } else {
                RunDelivery::Cancelled
            }
        } else {
            let mut error = false;
            let mut no_output = false;
            let mut recovered = false;
            for target in &self.targets {
                let record = &self.records[&target.node];
                match &record.outputs[&target.port] {
                    ResolvedValue::Error(_) => error = true,
                    ResolvedValue::NoOutput => no_output = true,
                    ResolvedValue::Data(_) => {
                        recovered |= record
                            .output_recoveries
                            .get(&target.port)
                            .is_some_and(|records| !records.is_empty())
                    }
                }
            }
            if error {
                RunDelivery::Failed
            } else if no_output {
                RunDelivery::NoOutput
            } else if recovered {
                RunDelivery::Recovered
            } else {
                RunDelivery::Complete
            }
        };
        Ok(result)
    }
    pub(crate) fn seal(&mut self, result: TerminalResult) {
        self.records.clear();
        self.discarded.clear();
        self.terminal = Some(result);
    }
}

pub(crate) fn target_scope(plan: &ExecutionPlan, targets: &[Endpoint]) -> Vec<usize> {
    let indices: BTreeMap<_, _> = plan
        .nodes
        .iter()
        .enumerate()
        .map(|(i, node)| (node.id.as_str(), i))
        .collect();
    let mut stack: Vec<_> = targets
        .iter()
        .map(|target| indices[target.node.as_str()])
        .collect();
    let mut required = BTreeSet::new();
    while let Some(index) = stack.pop() {
        if required.insert(index) {
            stack.extend(
                plan.nodes[index]
                    .sources
                    .values()
                    .map(|source| indices[source.node.as_str()]),
            );
        }
    }
    required.into_iter().collect()
}
fn invalid(message: &str) -> CoreError {
    CoreError::new("invalid-result", message)
}
fn report_failed(targets: &[Endpoint]) -> TerminalResult {
    TerminalResult {
        reason: TerminalReason::Failed,
        delivery: RunDelivery::Failed,
        targets: targets.to_vec(),
        ports: vec![],
        nodes: vec![],
        causes: vec![],
        recoveries: vec![],
        error: Some(CoreError::new(
            "report-failed",
            "Packet cannot cross the validated JSON boundary",
        )),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        builtins::register_demo_nodes,
        execution_control::{DecimalId, SessionEpoch},
        graph::Graph,
        node::{Inputs, Node, NodeDefinition, Outputs, Port, Registry, RunContext},
        runtime::Engine,
    };
    struct Fail;
    impl Node for Fail {
        fn definition(&self) -> NodeDefinition {
            NodeDefinition::new("test.fail", 1, vec![], vec![Port::packet("out")])
        }
        fn execute(&self, _: RunContext<'_>, _: &Inputs, _: &Value) -> Result<Outputs, CoreError> {
            Err(CoreError::new("original", "failure"))
        }
    }
    fn plan(registry: &Registry) -> ExecutionPlan {
        ExecutionPlan::compile(&Graph::from_json(&json!({"version":1,"nodes":[{"id":"fail","kind":"test.fail","version":1},{"id":"middle","kind":"demo.identity","version":1},{"id":"sink","kind":"demo.identity","version":1}],"edges":[{"from":{"node":"fail","port":"out"},"to":{"node":"middle","port":"in"}},{"from":{"node":"middle","port":"out"},"to":{"node":"sink","port":"in"}}]}).to_string()).unwrap(),registry).unwrap()
    }
    fn key(run: u64, version: u64) -> RunKey {
        RunKey {
            session_epoch: SessionEpoch::new("0123456789abcdef0123456789abcdef").unwrap(),
            run_id: DecimalId::new(run).unwrap(),
            plan_version: DecimalId::new(version).unwrap(),
            input_revision: None,
            delivery_generation: DecimalId::new(1).unwrap(),
        }
    }
    #[test]
    fn projected_causes_keep_every_real_engine_error_hop() {
        let mut registry = Registry::new();
        register_demo_nodes(&mut registry).unwrap();
        registry.register(Fail).unwrap();
        let targets = vec![Endpoint::new("sink", "out")];
        let mut engine = Engine::new(plan(&registry));
        let report = engine.run_with_targets(&targets, None).unwrap();
        let expected: Value = serde_json::from_str(&report.to_json().unwrap()).unwrap();
        let mut results = ExecutionResults::new(
            key(report.report.run_id, report.report.plan_version),
            Arc::new(plan(&registry)),
            targets,
        );
        for (id, execution) in report.report.nodes {
            results.record(&id, execution).unwrap();
        }
        let result = results.preview(TerminalReason::Completed).unwrap();
        assert_eq!(result.causes[0]["paths"], expected["causes"][0]["paths"]);
        assert_eq!(result.causes[0]["paths"].as_array().unwrap().len(), 2);
    }
    #[test]
    fn projected_identifiers_above_js_safe_integer_remain_exact_decimal_strings() {
        let mut registry = Registry::new();
        register_demo_nodes(&mut registry).unwrap();
        registry.register(Fail).unwrap();
        let key = key(u64::MAX, u64::MAX);
        let diagnostic = Diagnostic {
            run_id: u64::MAX,
            plan_version: u64::MAX,
            node_id: "fail".into(),
            port: None,
            error: CoreError::new("original", "failure"),
        };
        let errors = ErrorSet::new(diagnostic.clone());
        let execution = NodeExecution {
            status: NodeStatus::Failed,
            outputs: [("out".into(), ResolvedValue::Error(diagnostic.clone()))].into(),
            diagnostic: Some(diagnostic),
            input_errors: None,
            error_outputs: [("out".into(), errors)].into(),
            recoveries: vec![],
            output_recoveries: BTreeMap::new(),
        };
        let mut results = ExecutionResults::new(
            key,
            Arc::new(plan(&registry)),
            vec![Endpoint::new("fail", "out")],
        );
        results.record("fail", execution).unwrap();
        let result = results.preview(TerminalReason::Completed).unwrap();
        assert_eq!(result.causes[0]["id"]["runId"], "18446744073709551615");
        assert_eq!(
            result.causes[0]["diagnostic"]["planVersion"],
            "18446744073709551615"
        );
        assert_eq!(result.ports[0].value["causes"][0], result.causes[0]["id"]);
    }
}
