//! 同期ランタイム。非同期ノードと IME 固有の切替境界は後続段階で扱います。
use std::{
    collections::BTreeMap,
    panic::{AssertUnwindSafe, catch_unwind},
    sync::{
        Arc,
        atomic::{AtomicU64, Ordering},
    },
};

use serde::Serialize;

use crate::{
    Packet,
    error_input::{
        Delivery, ErrorAwareExecution, ErrorAwareInputs, ErrorAwareValue, ErrorSet, RecoveryRecord,
        TargetedRunReport, merge_errors,
    },
    graph::{Endpoint, ExecutionPlan, Graph, PlannedNode},
    node::{
        CoreError, Inputs, OutputValue, RegisteredImplementation, Registry, RunContext,
        RuntimeInput,
    },
};

static NEXT_RUN_ID: AtomicU64 = AtomicU64::new(1);

pub(crate) fn allocate_run_id() -> Result<u64, CoreError> {
    let mut id = NEXT_RUN_ID.load(Ordering::Relaxed);
    let allocated = loop {
        let next = id
            .checked_add(1)
            .ok_or_else(|| CoreError::new("id-exhausted", "run ID exhausted"))?;
        match NEXT_RUN_ID.compare_exchange_weak(id, next, Ordering::Relaxed, Ordering::Relaxed) {
            Ok(_) => break id,
            Err(actual) => id = actual,
        }
    };
    Ok(allocated)
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Diagnostic {
    pub run_id: u64,
    pub plan_version: u64,
    pub node_id: String,
    pub port: Option<String>,
    pub error: CoreError,
}

#[derive(Debug, Clone)]
pub enum ResolvedValue {
    Data(Arc<Packet>),
    NoOutput,
    Error(Diagnostic),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum NodeStatus {
    Executed,
    Skipped,
    Failed,
    Blocked,
}

#[derive(Debug, Clone)]
pub struct NodeExecution {
    pub status: NodeStatus,
    pub outputs: BTreeMap<String, ResolvedValue>,
    pub diagnostic: Option<Diagnostic>,
    pub input_errors: Option<ErrorSet>,
    pub error_outputs: BTreeMap<String, ErrorSet>,
    pub recoveries: Vec<RecoveryRecord>,
    pub output_recoveries: BTreeMap<String, Vec<RecoveryRecord>>,
}

#[derive(Debug)]
pub struct RunReport {
    pub run_id: u64,
    pub plan_version: u64,
    pub input_revision: Option<u64>,
    pub nodes: BTreeMap<String, NodeExecution>,
}

impl RunReport {
    /// CLI・デバッグ用の結果表示。Packet の検証付き JSON 境界を利用します。
    pub fn to_json(&self) -> Result<String, CoreError> {
        let mut nodes = serde_json::Map::new();
        for (id, node) in &self.nodes {
            let mut outputs = serde_json::Map::new();
            for (port, value) in &node.outputs {
                let value = match value {
                    ResolvedValue::Data(packet) => {
                        let json = packet
                            .to_json()
                            .map_err(|error| CoreError::new("report-json", error.to_string()))?;
                        let packet: serde_json::Value = serde_json::from_str(&json)
                            .map_err(|error| CoreError::new("report-json", error.to_string()))?;
                        serde_json::json!({"status":"data", "packet":packet})
                    }
                    ResolvedValue::NoOutput => serde_json::json!({"status":"no-output"}),
                    ResolvedValue::Error(diagnostic) => {
                        serde_json::json!({"status":"error", "diagnostic":diagnostic})
                    }
                };
                outputs.insert(port.clone(), value);
            }
            let status = match node.status {
                NodeStatus::Executed => "executed",
                NodeStatus::Skipped => "skipped",
                NodeStatus::Failed => "failed",
                NodeStatus::Blocked => "blocked",
            };
            nodes.insert(id.clone(), serde_json::json!({"status":status, "outputs":outputs, "diagnostic":node.diagnostic}));
        }
        let mut report = serde_json::json!({"runId":self.run_id, "planVersion":self.plan_version, "succeeded":self.succeeded(), "nodes":nodes});
        if let Some(revision) = self.input_revision {
            report["inputRevision"] = revision.into();
        }
        serde_json::to_string_pretty(&report)
            .map_err(|error| CoreError::new("report-json", error.to_string()))
    }

    pub fn packet(&self, node: &str, port: &str) -> Option<&Arc<Packet>> {
        match self.nodes.get(node)?.outputs.get(port)? {
            ResolvedValue::Data(packet) => Some(packet),
            _ => None,
        }
    }
    pub fn succeeded(&self) -> bool {
        self.nodes
            .values()
            .all(|node| !matches!(node.status, NodeStatus::Failed | NodeStatus::Blocked))
    }
}

/// 1回の run は同期実行です。編集済み計画は prepare 後、呼出側の安全な境界で
/// activate_pending を呼んだときだけ有効になります。共有された登録ノードは Send + Sync
/// ですが、ノード内部状態のロールバック・実行間分離はノード作者の責務です。
pub struct Engine {
    active: ExecutionPlan,
    pending: Option<ExecutionPlan>,
    previous: Option<ExecutionPlan>,
    plan_version: u64,
    last_run: u64,
}

impl Engine {
    pub fn new(plan: ExecutionPlan) -> Self {
        Self {
            active: plan,
            pending: None,
            previous: None,
            plan_version: 1,
            last_run: 0,
        }
    }
    pub fn plan_version(&self) -> u64 {
        self.plan_version
    }
    pub fn prepare(&mut self, graph: &Graph, registry: &Registry) -> Result<(), CoreError> {
        // 検証失敗時は active / pending のいずれも変更しない。
        let plan = ExecutionPlan::compile(graph, registry)?;
        self.prepare_plan(plan);
        Ok(())
    }
    /// 検証済みの計画を準備する。接続検査を重複して実行せずに連携層から渡せる。
    pub fn prepare_plan(&mut self, plan: ExecutionPlan) {
        self.pending = Some(plan);
    }
    pub fn activate_pending(&mut self) -> Result<bool, CoreError> {
        if self.pending.is_none() {
            return Ok(false);
        }
        let version = self
            .plan_version
            .checked_add(1)
            .ok_or_else(|| CoreError::new("id-exhausted", "plan version exhausted"))?;
        self.previous = Some(std::mem::replace(
            &mut self.active,
            self.pending.take().unwrap(),
        ));
        self.plan_version = version;
        Ok(true)
    }
    /// 直前の計画1件へ戻す。版番号は増やし、過去の実行結果を再び有効にはしない。
    pub fn rollback(&mut self) -> Result<bool, CoreError> {
        if self.previous.is_none() {
            return Ok(false);
        }
        let version = self
            .plan_version
            .checked_add(1)
            .ok_or_else(|| CoreError::new("id-exhausted", "plan version exhausted"))?;
        self.active = self.previous.take().unwrap();
        self.pending = None;
        self.plan_version = version;
        Ok(true)
    }
    /// 現行計画の直近の実行結果かどうか。古い結果を UI に反映しないために使います。
    pub fn is_current(&self, report: &RunReport) -> bool {
        report.plan_version == self.plan_version && report.run_id == self.last_run
    }
    pub fn run(&mut self) -> Result<RunReport, CoreError> {
        self.run_internal(None)
    }
    pub fn run_with_input(&mut self, input: &RuntimeInput) -> Result<RunReport, CoreError> {
        input.validate()?;
        self.run_internal(Some(input))
    }
    pub fn run_with_targets(
        &mut self,
        targets: &[Endpoint],
        input: Option<&RuntimeInput>,
    ) -> Result<TargetedRunReport, CoreError> {
        self.active.validate_targets(targets)?;
        if let Some(input) = input {
            input.validate()?;
        }
        let report = self.run_internal(input)?;
        let mut delivery = Delivery::Complete;
        for target in targets {
            let node = &report.nodes[&target.node];
            match &node.outputs[&target.port] {
                ResolvedValue::Error(_) => {
                    delivery = Delivery::Failed;
                    break;
                }
                ResolvedValue::NoOutput => delivery = Delivery::NoOutput,
                ResolvedValue::Data(_)
                    if delivery != Delivery::NoOutput
                        && node
                            .output_recoveries
                            .get(&target.port)
                            .is_some_and(|records| !records.is_empty()) =>
                {
                    delivery = Delivery::Recovered
                }
                _ => {}
            }
        }
        Ok(TargetedRunReport {
            report,
            targets: targets.to_vec(),
            delivery,
        })
    }
    fn run_internal(&mut self, input: Option<&RuntimeInput>) -> Result<RunReport, CoreError> {
        self.active.require_synchronous()?;
        self.last_run = allocate_run_id()?;
        let mut results: BTreeMap<String, NodeExecution> = BTreeMap::new();
        for planned in &self.active.nodes {
            let context = RunContext {
                run_id: self.last_run,
                plan_version: self.plan_version,
                node_id: &planned.id,
                input,
            };
            let mut inputs = ErrorAwareInputs::new();
            let mut input_recoveries = BTreeMap::new();
            let mut input_errors = None;
            let mut upstream_error = None;
            let mut missing_required = false;
            for input in &planned.node.definition.inputs {
                let value = match planned.sources.get(&input.port.id) {
                    Some(source) => {
                        let previous = &results[&source.node];
                        match &previous.outputs[&source.port] {
                            ResolvedValue::Data(packet) => {
                                input_recoveries.insert(
                                    input.port.id.clone(),
                                    previous
                                        .output_recoveries
                                        .get(&source.port)
                                        .cloned()
                                        .unwrap_or_default(),
                                );
                                ErrorAwareValue::Data(Arc::clone(packet))
                            }
                            ResolvedValue::NoOutput => ErrorAwareValue::NoOutput,
                            ResolvedValue::Error(diagnostic) => {
                                let errors = previous.error_outputs[&source.port].routed(
                                    source.clone(),
                                    Endpoint::new(&planned.id, &input.port.id),
                                );
                                merge_errors(&mut input_errors, &errors);
                                if !planned.node.implementation.receives_error(&input.port.id) {
                                    upstream_error.get_or_insert_with(|| diagnostic.clone());
                                }
                                ErrorAwareValue::Error(errors)
                            }
                        }
                    }
                    None => ErrorAwareValue::NoOutput,
                };
                missing_required |= input.required && matches!(value, ErrorAwareValue::NoOutput);
                inputs.insert(input.port.id.clone(), value);
            }
            let mut execution = if let Some(diagnostic) = upstream_error {
                failed(
                    planned,
                    NodeStatus::Blocked,
                    diagnostic,
                    input_errors.clone(),
                )
            } else if missing_required {
                empty_execution(planned)
            } else {
                execute(
                    planned,
                    context,
                    &inputs,
                    &input_recoveries,
                    input_errors.as_ref(),
                )
            };
            execution.input_errors = input_errors;
            results.insert(planned.id.clone(), execution);
        }
        Ok(RunReport {
            run_id: self.last_run,
            plan_version: self.plan_version,
            input_revision: input.map(|input| input.revision),
            nodes: results,
        })
    }
}

pub(crate) fn empty_execution(node: &PlannedNode) -> NodeExecution {
    NodeExecution {
        status: NodeStatus::Skipped,
        outputs: node
            .node
            .definition
            .outputs
            .iter()
            .map(|port| (port.id.clone(), ResolvedValue::NoOutput))
            .collect(),
        diagnostic: None,
        input_errors: None,
        error_outputs: BTreeMap::new(),
        recoveries: vec![],
        output_recoveries: BTreeMap::new(),
    }
}

pub(crate) fn failed(
    node: &PlannedNode,
    status: NodeStatus,
    diagnostic: Diagnostic,
    errors: Option<ErrorSet>,
) -> NodeExecution {
    let errors = errors.unwrap_or_else(|| ErrorSet::new(diagnostic.clone()));
    NodeExecution {
        status,
        outputs: node
            .node
            .definition
            .outputs
            .iter()
            .map(|port| (port.id.clone(), ResolvedValue::Error(diagnostic.clone())))
            .collect(),
        error_outputs: node
            .node
            .definition
            .outputs
            .iter()
            .map(|port| (port.id.clone(), errors.clone()))
            .collect(),
        diagnostic: Some(diagnostic),
        input_errors: None,
        recoveries: vec![],
        output_recoveries: BTreeMap::new(),
    }
}

pub(crate) fn execute(
    node: &PlannedNode,
    context: RunContext<'_>,
    inputs: &ErrorAwareInputs,
    input_recoveries: &BTreeMap<String, Vec<RecoveryRecord>>,
    input_errors: Option<&ErrorSet>,
) -> NodeExecution {
    let outcome = catch_unwind(AssertUnwindSafe(|| match &node.node.implementation {
        RegisteredImplementation::Legacy(implementation) => {
            let inputs: Inputs = inputs
                .iter()
                .map(|(id, value)| {
                    (
                        id.clone(),
                        match value {
                            ErrorAwareValue::Data(packet) => OutputValue::Data(Arc::clone(packet)),
                            ErrorAwareValue::NoOutput => OutputValue::NoOutput,
                            ErrorAwareValue::Error(_) => {
                                unreachable!("legacy inputs are blocked before execution")
                            }
                        },
                    )
                })
                .collect();
            match &node.prepared {
                Some(prepared) => prepared.execute(context, &inputs),
                None => implementation.execute(context, &inputs, &node.config),
            }
            .map(|outputs| {
                ErrorAwareExecution::new(
                    outputs
                        .into_iter()
                        .map(|(id, value)| {
                            (
                                id,
                                match value {
                                    OutputValue::Data(packet) => ErrorAwareValue::Data(packet),
                                    OutputValue::NoOutput => ErrorAwareValue::NoOutput,
                                },
                            )
                        })
                        .collect(),
                )
            })
        }
        RegisteredImplementation::ErrorAware {
            node: implementation,
            ..
        } => implementation.execute(context, inputs, &node.config),
        RegisteredImplementation::Deferred { .. } | RegisteredImplementation::Control { .. } => {
            Err(CoreError::new(
                "unsupported-capability",
                "requires execution controller",
            ))
        }
    }))
    .unwrap_or_else(|_| {
        Err(CoreError::new(
            "node-panic",
            "node panicked during execution",
        ))
    });
    resolve_outcome(
        node,
        context,
        inputs,
        input_recoveries,
        input_errors,
        outcome,
    )
}

pub(crate) fn resolve_outcome(
    node: &PlannedNode,
    context: RunContext<'_>,
    inputs: &ErrorAwareInputs,
    input_recoveries: &BTreeMap<String, Vec<RecoveryRecord>>,
    input_errors: Option<&ErrorSet>,
    outcome: Result<ErrorAwareExecution, CoreError>,
) -> NodeExecution {
    let outcome = outcome
        .map_err(|error| (None, error))
        .and_then(|execution| {
            validate_execution(node, &execution, inputs, input_errors)?;
            Ok(execution)
        });
    match outcome {
        Ok(execution) => {
            let mut result = empty_execution(node);
            result.status = NodeStatus::Executed;
            result.recoveries = execution
                .recoveries
                .into_iter()
                .map(|claim| RecoveryRecord {
                    node_id: node.id.clone(),
                    input: claim.input,
                    output: claim.output,
                    causes: claim.causes,
                })
                .collect();
            for (id, value) in execution.outputs {
                let resolved = match value {
                    ErrorAwareValue::Data(packet) => {
                        let dependencies = execution.dependencies.get(&id);
                        let mut records = vec![];
                        for (input, inherited) in input_recoveries {
                            if dependencies.is_none_or(|ids| ids.contains(input)) {
                                for record in inherited {
                                    if !records.contains(record) {
                                        records.push(record.clone());
                                    }
                                }
                            }
                        }
                        for record in result
                            .recoveries
                            .iter()
                            .filter(|record| record.output == id)
                        {
                            if !records.contains(record) {
                                records.push(record.clone());
                            }
                        }
                        result.output_recoveries.insert(id.clone(), records);
                        ResolvedValue::Data(packet)
                    }
                    ErrorAwareValue::NoOutput => ResolvedValue::NoOutput,
                    ErrorAwareValue::Error(errors) => {
                        let diagnostic = errors.causes()[0].diagnostic.clone();
                        result.error_outputs.insert(id.clone(), errors);
                        ResolvedValue::Error(diagnostic)
                    }
                };
                result.outputs.insert(id, resolved);
            }
            result
        }
        Err((port, error)) => failed(
            node,
            NodeStatus::Failed,
            Diagnostic {
                run_id: context.run_id,
                plan_version: context.plan_version,
                node_id: node.id.clone(),
                port,
                error,
            },
            None,
        ),
    }
}

fn invalid(code: &str, message: &str) -> (Option<String>, CoreError) {
    (None, CoreError::new(code, message))
}

fn validate_execution(
    node: &PlannedNode,
    execution: &ErrorAwareExecution,
    inputs: &ErrorAwareInputs,
    input_errors: Option<&ErrorSet>,
) -> Result<(), (Option<String>, CoreError)> {
    for id in execution.outputs.keys() {
        if !node
            .node
            .definition
            .outputs
            .iter()
            .any(|port| &port.id == id)
        {
            return Err((
                Some(id.clone()),
                CoreError::new("unknown-output", "node returned an undeclared output"),
            ));
        }
    }
    for port in &node.node.definition.outputs {
        let value = execution.outputs.get(&port.id).ok_or_else(|| {
            (
                Some(port.id.clone()),
                CoreError::new(
                    "missing-output",
                    "return explicit NoOutput for an inactive port",
                ),
            )
        })?;
        match value {
            ErrorAwareValue::Data(packet) => {
                packet.validate().map_err(|error| {
                    (
                        Some(port.id.clone()),
                        CoreError::new("invalid-packet", error.to_string()),
                    )
                })?;
                if !port.root.accepts(&packet.root) {
                    return Err((
                        Some(port.id.clone()),
                        CoreError::new(
                            "invalid-root",
                            "output root violates the declared port constraint",
                        ),
                    ));
                }
            }
            ErrorAwareValue::Error(errors)
                if input_errors.is_none_or(|allowed| !errors.is_subset_of(allowed)) =>
            {
                return Err(invalid(
                    "invalid-error-propagation",
                    "only errors received in this run may be propagated",
                ));
            }
            _ => {}
        }
    }
    let mut claims = std::collections::BTreeSet::new();
    for claim in &execution.recoveries {
        let Some(ErrorAwareValue::Error(errors)) = inputs.get(&claim.input) else {
            return Err(invalid(
                "invalid-recovery",
                "recovery input must contain an error",
            ));
        };
        let unique: std::collections::BTreeSet<_> = claim.causes.iter().collect();
        if claim.causes.is_empty()
            || unique.len() != claim.causes.len()
            || !claims.insert((&claim.input, &claim.output))
            || !matches!(
                execution.outputs.get(&claim.output),
                Some(ErrorAwareValue::Data(_))
            )
            || claim
                .causes
                .iter()
                .any(|id| !errors.causes().iter().any(|cause| &cause.id == id))
        {
            return Err(invalid(
                "invalid-recovery",
                "recovery must name unique received causes and a Data output",
            ));
        }
    }
    for (output, dependencies) in &execution.dependencies {
        let unique: std::collections::BTreeSet<_> = dependencies.iter().collect();
        if !matches!(
            execution.outputs.get(output),
            Some(ErrorAwareValue::Data(_))
        ) || unique.len() != dependencies.len()
            || dependencies
                .iter()
                .any(|input| !matches!(inputs.get(input), Some(ErrorAwareValue::Data(_))))
        {
            return Err(invalid(
                "invalid-dependencies",
                "dependencies must be unique Data inputs of a Data output",
            ));
        }
    }
    Ok(())
}
