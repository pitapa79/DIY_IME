//! 明示的な全文入力・確定・取消を検査する偽文書ホスト。TSF/OS接続ではない。
#[allow(dead_code)]
#[path = "../tests/support/demo.rs"]
mod test_nodes;
use std::{error::Error, fmt, sync::Arc};
use test_nodes::register_builtin_nodes;

use diy_ime2_core::{
    Candidate,
    graph::{ExecutionPlan, Graph},
    node::{CoreError, InputMode, Registry, RuntimeInput},
    runtime::{Engine, ResolvedValue, RunReport},
};

#[derive(Debug, Clone, PartialEq, Eq)]
enum HostError {
    Core(String),
    NoComposition,
    BusyComposition,
    StaleResponse,
    DuplicateResponse,
    NoReadyResponse,
    NoUsableOutput,
    ConversionFailed,
    IncompleteFinalization,
    DocumentChanged,
    DocumentRejected,
    CounterExhausted,
}
impl fmt::Display for HostError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{self:?}")
    }
}
impl Error for HostError {}
impl From<CoreError> for HostError {
    fn from(error: CoreError) -> Self {
        Self::Core(error.to_string())
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct DocumentStamp {
    context: u64,
    revision: u64,
}

#[derive(Debug)]
struct Preedit {
    owner: u64,
    text: String,
}

/// この偽物だけは、失敗なら本文・preedit・世代が全て変わらないと保証する。
struct FakeDocument {
    stamp: DocumentStamp,
    committed: String,
    preedit: Option<Preedit>,
    commits: usize,
    reject_next: bool,
}
enum Edit<'a> {
    Preview(&'a str),
    Commit(&'a str),
    Cancel,
}

impl FakeDocument {
    fn new(committed: &str) -> Self {
        Self {
            stamp: DocumentStamp {
                context: 1,
                revision: 0,
            },
            committed: committed.into(),
            preedit: None,
            commits: 0,
            reject_next: false,
        }
    }
    fn apply(
        &mut self,
        expected: DocumentStamp,
        owner: u64,
        edit: Edit<'_>,
    ) -> Result<DocumentStamp, HostError> {
        if self.stamp != expected
            || self
                .preedit
                .as_ref()
                .is_some_and(|preedit| preedit.owner != owner)
        {
            return Err(HostError::DocumentChanged);
        }
        let revision = self
            .stamp
            .revision
            .checked_add(1)
            .ok_or(HostError::CounterExhausted)?;
        let commits = if matches!(edit, Edit::Commit(_)) {
            self.commits
                .checked_add(1)
                .ok_or(HostError::CounterExhausted)?
        } else {
            self.commits
        };
        if self.reject_next {
            self.reject_next = false;
            return Err(HostError::DocumentRejected);
        }
        match edit {
            Edit::Preview(text) => {
                self.preedit = Some(Preedit {
                    owner,
                    text: text.into(),
                })
            }
            Edit::Commit(text) => {
                self.committed.push_str(text);
                self.preedit = None;
            }
            Edit::Cancel => self.preedit = None,
        }
        self.commits = commits;
        self.stamp.revision = revision;
        Ok(self.stamp)
    }
    fn preview(&self) -> Option<&str> {
        self.preedit.as_ref().map(|preedit| preedit.text.as_str())
    }

    /// 試験用の外部変更。実際のフォーカス喪失・外部編集ポリシーを採用しない。
    #[cfg(test)]
    fn external_change(&mut self, new_context: bool, committed: &str) {
        if new_context {
            self.stamp.context += 1;
            self.stamp.revision = 0;
        } else {
            self.stamp.revision += 1;
        }
        self.committed = committed.into();
        self.preedit = None;
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Intent {
    Preview,
    Commit,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct RequestStamp {
    composition: u64,
    input: RuntimeInput,
    document: DocumentStamp,
    plan: u64,
    run: u64,
    intent: Intent,
}
#[derive(Debug, Clone)]
struct Response {
    stamp: RequestStamp,
    report: Arc<RunReport>,
}
#[derive(Clone)]
struct Ready {
    response: Response,
    text: String,
}
struct Composition {
    generation: u64,
    input: RuntimeInput,
    document: DocumentStamp,
    plan: u64,
}
enum Command {
    ReplaceInput(String),
    Commit,
    Cancel,
}

struct Host {
    registry: Registry,
    engine: Engine,
    document: FakeDocument,
    composition: Option<Composition>,
    revision: u64,
    generation: u64,
    latest: Option<RequestStamp>,
    ready: Option<Ready>,
}

impl Host {
    fn new(graph: &Graph, committed: &str) -> Result<Self, HostError> {
        let mut registry = Registry::new();
        register_builtin_nodes(&mut registry)?;
        registry.register(FixtureConversion)?;
        let engine = Engine::new(ExecutionPlan::compile(graph, &registry)?);
        Ok(Self {
            registry,
            engine,
            document: FakeDocument::new(committed),
            composition: None,
            revision: 0,
            generation: 0,
            latest: None,
            ready: None,
        })
    }
    fn next_input(&self, text: String, mode: InputMode) -> Result<RuntimeInput, HostError> {
        let revision = self
            .revision
            .checked_add(1)
            .ok_or(HostError::CounterExhausted)?;
        let input = RuntimeInput {
            revision,
            text,
            mode,
        };
        input.validate()?;
        Ok(input)
    }
    fn ensure_composition(&self) -> Result<&Composition, HostError> {
        let composition = self.composition.as_ref().ok_or(HostError::NoComposition)?;
        if composition.document != self.document.stamp
            || composition.plan != self.engine.plan_version()
        {
            return Err(HostError::DocumentChanged);
        }
        Ok(composition)
    }
    fn dispatch(&mut self, command: Command) -> Result<Option<Response>, HostError> {
        match command {
            Command::ReplaceInput(text) => {
                let input = self.next_input(text, InputMode::Compose)?;
                if self.composition.is_none() {
                    let generation = self
                        .generation
                        .checked_add(1)
                        .ok_or(HostError::CounterExhausted)?;
                    self.composition = Some(Composition {
                        generation,
                        input: input.clone(),
                        document: self.document.stamp,
                        plan: self.engine.plan_version(),
                    });
                    self.generation = generation;
                } else {
                    self.ensure_composition()?;
                }
                self.revision = input.revision;
                self.composition.as_mut().unwrap().input = input;
                self.issue(Intent::Preview).map(Some)
            }
            Command::Commit => {
                let composition = self.ensure_composition()?;
                if composition.input.mode != InputMode::Finalize {
                    let input =
                        self.next_input(composition.input.text.clone(), InputMode::Finalize)?;
                    self.revision = input.revision;
                    self.composition.as_mut().unwrap().input = input;
                }
                self.issue(Intent::Commit).map(Some)
            }
            Command::Cancel => {
                // 取消が文書側で失敗しても、以前の確定要求は後から適用させない。
                self.latest = None;
                self.ready = None;
                let composition = self.composition.as_ref().ok_or(HostError::NoComposition)?;
                if self
                    .document
                    .preedit
                    .as_ref()
                    .is_some_and(|preedit| preedit.owner == composition.generation)
                {
                    self.document.apply(
                        self.document.stamp,
                        composition.generation,
                        Edit::Cancel,
                    )?;
                }
                // 対象preeditがない場合は、明示取消でホストの入力だけを捨てる。
                self.composition = None;
                Ok(None)
            }
        }
    }
    fn issue(&mut self, intent: Intent) -> Result<Response, HostError> {
        self.latest = None;
        self.ready = None;
        let composition = self.composition.as_ref().ok_or(HostError::NoComposition)?;
        let report = self.engine.run_with_input(&composition.input)?;
        let stamp = RequestStamp {
            composition: composition.generation,
            input: composition.input.clone(),
            document: composition.document,
            plan: composition.plan,
            run: report.run_id,
            intent,
        };
        self.latest = Some(stamp.clone());
        Ok(Response {
            stamp,
            report: Arc::new(report),
        })
    }
    fn check(&self, response: &Response) -> Result<(), HostError> {
        let Some(composition) = &self.composition else {
            return Err(HostError::StaleResponse);
        };
        let stamp = &response.stamp;
        if self.latest.as_ref() != Some(stamp)
            || composition.generation != stamp.composition
            || composition.input != stamp.input
            || composition.document != stamp.document
            || self.document.stamp != stamp.document
            || composition.plan != stamp.plan
            || self.engine.plan_version() != stamp.plan
            || response.report.plan_version != stamp.plan
            || response.report.run_id != stamp.run
            || response.report.input_revision != Some(stamp.input.revision)
            || !self.engine.is_current(&response.report)
        {
            return Err(HostError::StaleResponse);
        }
        Ok(())
    }
    fn receive(&mut self, response: Response) -> Result<(), HostError> {
        self.check(&response)?;
        if self.ready.is_some() {
            return Err(HostError::DuplicateResponse);
        }
        let decoded = decode(&response);
        match decoded {
            Ok(text) => {
                self.ready = Some(Ready { response, text });
                Ok(())
            }
            Err(error) => {
                self.latest = None;
                Err(error)
            }
        }
    }
    fn apply_ready(&mut self) -> Result<(), HostError> {
        let ready = self.ready.clone().ok_or(HostError::NoReadyResponse)?;
        // 受取から文書適用までに文書・計画・入力が変わっていないか再照合する。
        self.check(&ready.response)?;
        let stamp = &ready.response.stamp;
        let edit = match stamp.intent {
            Intent::Preview => Edit::Preview(&ready.text),
            Intent::Commit => Edit::Commit(&ready.text),
        };
        let document = self
            .document
            .apply(stamp.document, stamp.composition, edit)?;
        match stamp.intent {
            Intent::Preview => self.composition.as_mut().unwrap().document = document,
            Intent::Commit => self.composition = None,
        }
        self.ready = None;
        self.latest = None;
        Ok(())
    }
    fn prepare(&mut self, graph: &Graph) -> Result<(), HostError> {
        self.engine
            .prepare(graph, &self.registry)
            .map_err(Into::into)
    }
    fn activate(&mut self) -> Result<bool, HostError> {
        if self.composition.is_some() {
            return Err(HostError::BusyComposition);
        }
        self.engine.activate_pending().map_err(Into::into)
    }
    fn rollback(&mut self) -> Result<bool, HostError> {
        if self.composition.is_some() {
            return Err(HostError::BusyComposition);
        }
        self.engine.rollback().map_err(Into::into)
    }
}

fn decode(response: &Response) -> Result<String, HostError> {
    if !response.report.succeeded() {
        return Err(HostError::ConversionFailed);
    }
    let text = |port: &str| -> Result<String, HostError> {
        let node = response
            .report
            .nodes
            .get("romaji")
            .ok_or(HostError::NoUsableOutput)?;
        let Some(ResolvedValue::Data(packet)) = node.outputs.get(port) else {
            return Err(HostError::NoUsableOutput);
        };
        packet.validate().map_err(|_| HostError::NoUsableOutput)?;
        match &packet.root {
            Candidate::Text { text, .. } => Ok(text.clone()),
            _ => Err(HostError::NoUsableOutput),
        }
    };
    let out = text("out")?;
    let pending = text("pending")?;
    match response.stamp.intent {
        Intent::Preview => Ok(out + &pending),
        Intent::Commit if pending.is_empty() => Ok(out),
        Intent::Commit => Err(HostError::IncompleteFinalization),
    }
}

fn graph() -> Graph {
    fixture_graph(include_str!("graphs/runtime-romaji.json")).unwrap()
}
fn deliver(host: &mut Host, command: Command) -> Result<(), HostError> {
    if let Some(response) = host.dispatch(command)? {
        host.receive(response)?;
        host.apply_ready()?;
    }
    Ok(())
}

fn main() -> Result<(), Box<dyn Error>> {
    let graph = graph();
    let mut host = Host::new(&graph, "本文:")?;
    for input in ["k", "ky", "kya", "ky", "", "n"] {
        deliver(&mut host, Command::ReplaceInput(input.into()))?;
        println!(
            "ReplaceInput({input:?}): preedit={:?}, committed={:?}",
            host.document.preview(),
            host.document.committed
        );
    }
    let response = host.dispatch(Command::Commit)?.unwrap();
    println!(
        "finalize response produced; committed is still {:?}",
        host.document.committed
    );
    host.receive(response)?;
    host.document.reject_next = true;
    assert_eq!(host.apply_ready(), Err(HostError::DocumentRejected));
    assert_eq!(host.composition.as_ref().unwrap().input.text, "n");
    host.apply_ready()?; // この偽物では前回の失敗が原子的・確定的なので再試行できる。
    println!(
        "commit applied once: {:?} ({} commit)",
        host.document.committed, host.document.commits
    );
    deliver(&mut host, Command::ReplaceInput("ka".into()))?;
    host.prepare(&graph)?;
    assert_eq!(host.activate(), Err(HostError::BusyComposition));
    deliver(&mut host, Command::Cancel)?;
    assert!(host.activate()?);
    assert!(host.rollback()?);
    println!(
        "cancel preserved {:?}; explicit activate/rollback reached plan v{}",
        host.document.committed,
        host.engine.plan_version()
    );
    Ok(())
}

// A finite response table for fake-document ownership tests. This is not a
// romanizer or production Node; actual processing is checked through official Wasm.
struct FixtureConversion;
fn fixture_graph(source: &str) -> Result<Graph, CoreError> {
    let mut graph = Graph::from_json(source)?;
    for node in &mut graph.nodes {
        if node.kind == "text.romaji-to-hiragana" {
            node.kind = "test.conversion-response".into();
        }
    }
    Ok(graph)
}
impl diy_ime2_core::node::Node for FixtureConversion {
    fn definition(&self) -> diy_ime2_core::node::NodeDefinition {
        use diy_ime2_core::node::{InputPort, NodeDefinition, Port, RootConstraint};
        NodeDefinition::new(
            "test.conversion-response",
            1,
            vec![InputPort::required(
                Port::packet("in")
                    .with_language("ja-Latn")
                    .with_root(RootConstraint::Text),
            )],
            vec![
                Port::packet("out")
                    .with_language("ja")
                    .with_root(RootConstraint::Text),
                Port::packet("pending")
                    .with_language("ja-Latn")
                    .with_root(RootConstraint::Text),
            ],
        )
    }
    fn validate_config(&self, config: &serde_json::Value) -> Result<(), CoreError> {
        if config.as_object().is_some_and(|object| object.is_empty())
            || (config["overrides"] == serde_json::json!([])
                && matches!(config["unknown"].as_str(), Some("preserve" | "error")))
        {
            Ok(())
        } else {
            Err(CoreError::new(
                "fixture-config",
                "invalid synthetic response mode",
            ))
        }
    }
    fn execute(
        &self,
        context: diy_ime2_core::node::RunContext<'_>,
        inputs: &diy_ime2_core::node::Inputs,
        config: &serde_json::Value,
    ) -> Result<diy_ime2_core::node::Outputs, CoreError> {
        use diy_ime2_core::{Packet, node::OutputValue};
        let text = match &inputs["in"] {
            OutputValue::Data(packet) => match &packet.root {
                Candidate::Text { text, .. } => text.as_str(),
                _ => return Err(CoreError::new("fixture-input", "Text required")),
            },
            OutputValue::NoOutput => {
                return Ok([
                    ("out".into(), OutputValue::NoOutput),
                    ("pending".into(), OutputValue::NoOutput),
                ]
                .into());
            }
        };
        let finalize = context
            .input
            .is_some_and(|input| input.mode == InputMode::Finalize);
        let (out, pending) = match (text, finalize) {
            ("", _) => ("", ""),
            ("k", false) => ("", "k"),
            ("ky", false) => ("", "ky"),
            ("k", true) => ("k", ""),
            ("ky", true) => ("ky", ""),
            ("a", _) => ("あ", ""),
            ("ka", _) => ("か", ""),
            ("kya", _) => ("きゃ", ""),
            ("na", _) => ("な", ""),
            ("n", false) => ("", "n"),
            ("n", true) => ("ん", ""),
            ("𠮷30%desu", _) => ("𠮷30%です", ""),
            ("q", _) if config["unknown"] == "error" => {
                return Err(CoreError::new(
                    "fixture-error",
                    "synthetic conversion failure",
                ));
            }
            ("q", _) => ("q", ""),
            _ => {
                return Err(CoreError::new(
                    "fixture-input",
                    "no synthetic response for this test vector",
                ));
            }
        };
        Ok([("out".into(), out), ("pending".into(), pending)]
            .map(|(port, text)| {
                (
                    port,
                    OutputValue::Data(Arc::new(Packet::new(Candidate::Text {
                        text: text.into(),
                        meta: None,
                    }))),
                )
            })
            .into())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use diy_ime2_core::{
        Packet,
        node::{MAX_INPUT_CODE_POINTS, MAX_INPUT_REVISION},
    };
    use serde_json::json;

    fn host() -> Host {
        Host::new(&graph(), "既存:").unwrap()
    }
    fn request(host: &mut Host, command: Command) -> Response {
        host.dispatch(command).unwrap().unwrap()
    }
    fn replace(host: &mut Host, text: &str) {
        deliver(host, Command::ReplaceInput(text.into())).unwrap();
    }
    fn input(host: &Host) -> &RuntimeInput {
        &host.composition.as_ref().unwrap().input
    }
    fn strict_graph() -> Graph {
        let mut graph = graph();
        graph
            .nodes
            .iter_mut()
            .find(|node| node.id == "romaji")
            .unwrap()
            .config = Some(json!({"overrides":[],"unknown":"error"}));
        graph
    }

    #[test]
    fn full_snapshots_pending_shortening_and_empty_preview_preserve_committed_text() {
        let mut host = host();
        for (text, expected) in [
            ("k", "k"),
            ("ky", "ky"),
            ("kya", "きゃ"),
            ("ky", "ky"),
            ("", ""),
            ("𠮷30%desu", "𠮷30%です"),
        ] {
            replace(&mut host, text);
            assert_eq!(host.document.preview(), Some(expected));
            assert_eq!(host.document.committed, "既存:");
            assert_eq!(host.document.commits, 0);
            assert_eq!(host.engine.plan_version(), 1);
            assert_eq!(input(&host).text, text);
        }
    }

    #[test]
    fn finalize_production_and_receipt_do_not_commit_until_document_apply() {
        let mut host = host();
        replace(&mut host, "n");
        assert_eq!(host.document.preview(), Some("n"));
        let before = input(&host).revision;
        let response = request(&mut host, Command::Commit);
        assert_eq!(response.stamp.input.mode, InputMode::Finalize);
        assert!(response.stamp.input.revision > before);
        assert_eq!(decode(&response), Ok("ん".into()));
        assert_eq!(host.document.committed, "既存:");
        host.receive(response).unwrap();
        assert_eq!(host.document.committed, "既存:");
        host.apply_ready().unwrap();
        assert_eq!(host.document.committed, "既存:ん");
        assert_eq!(host.document.preview(), None);
        assert!(host.composition.is_none());
        assert_eq!(host.document.commits, 1);
    }

    #[test]
    fn explicit_commit_of_empty_input_never_deletes_existing_text() {
        let mut host = host();
        replace(&mut host, "kya");
        replace(&mut host, "");
        deliver(&mut host, Command::Commit).unwrap();
        assert_eq!(host.document.committed, "既存:");
        assert_eq!(host.document.commits, 1);
        assert_eq!(host.document.preview(), None);
        assert!(host.composition.is_none());
    }

    #[test]
    fn old_and_duplicate_responses_cannot_overwrite_a_new_ready_response() {
        let mut host = host();
        let old = request(&mut host, Command::ReplaceInput("k".into()));
        let current = request(&mut host, Command::ReplaceInput("kya".into()));
        let duplicate = current.clone();
        host.receive(current).unwrap();
        assert_eq!(host.receive(old), Err(HostError::StaleResponse));
        assert_eq!(host.receive(duplicate), Err(HostError::DuplicateResponse));
        host.apply_ready().unwrap();
        assert_eq!(host.document.preview(), Some("きゃ"));
        assert_eq!(host.document.committed, "既存:");
    }

    #[test]
    fn repeated_commit_requests_and_duplicate_delivery_apply_only_once() {
        let mut host = host();
        replace(&mut host, "n");
        let first = request(&mut host, Command::Commit);
        let second = request(&mut host, Command::Commit);
        assert_eq!(first.stamp.input.revision, second.stamp.input.revision);
        let duplicate = second.clone();
        assert_eq!(host.receive(first), Err(HostError::StaleResponse));
        host.receive(second).unwrap();
        assert_eq!(
            host.receive(duplicate.clone()),
            Err(HostError::DuplicateResponse)
        );
        host.apply_ready().unwrap();
        assert_eq!(host.apply_ready(), Err(HostError::NoReadyResponse));
        assert_eq!(host.receive(duplicate), Err(HostError::StaleResponse));
        assert!(matches!(
            host.dispatch(Command::Commit),
            Err(HostError::NoComposition)
        ));
        assert_eq!(host.document.committed, "既存:ん");
        assert_eq!(host.document.commits, 1);
    }

    #[test]
    fn cancel_invalidates_pending_commit_and_preserves_already_committed_text() {
        let mut host = host();
        replace(&mut host, "ka");
        deliver(&mut host, Command::Commit).unwrap();
        replace(&mut host, "n");
        let pending_commit = request(&mut host, Command::Commit);
        deliver(&mut host, Command::Cancel).unwrap();
        assert_eq!(host.receive(pending_commit), Err(HostError::StaleResponse));
        assert!(host.composition.is_none());
        assert_eq!(host.document.preview(), None);
        assert_eq!(host.document.committed, "既存:か");
        assert_eq!(host.document.commits, 1);
    }

    #[test]
    fn same_text_in_a_new_composition_does_not_reuse_a_cancelled_response() {
        let mut host = host();
        let old = request(&mut host, Command::ReplaceInput("n".into()));
        deliver(&mut host, Command::Cancel).unwrap();
        let current = request(&mut host, Command::ReplaceInput("n".into()));
        assert_ne!(old.stamp.composition, current.stamp.composition);
        assert_eq!(host.receive(old), Err(HostError::StaleResponse));
        host.receive(current).unwrap();
        host.apply_ready().unwrap();
        assert_eq!(host.document.preview(), Some("n"));
    }

    #[test]
    fn document_and_context_changes_reject_results_at_both_delivery_and_apply() {
        for new_context in [false, true] {
            for received in [false, true] {
                let mut host = host();
                let response = request(&mut host, Command::ReplaceInput("n".into()));
                if received {
                    host.receive(response.clone()).unwrap();
                }
                host.document.external_change(new_context, "別の本文");
                let error = if received {
                    host.apply_ready()
                } else {
                    host.receive(response)
                };
                assert_eq!(error, Err(HostError::StaleResponse));
                assert_eq!(input(&host).text, "n");
                assert_eq!(host.document.committed, "別の本文");
                assert!(matches!(
                    host.dispatch(Command::ReplaceInput("na".into())),
                    Err(HostError::DocumentChanged)
                ));
                deliver(&mut host, Command::Cancel).unwrap();
                assert_eq!(host.document.committed, "別の本文");
                replace(&mut host, "ka");
                assert_eq!(host.document.preview(), Some("か"));
            }
        }
    }

    #[test]
    fn atomic_preview_failure_retains_input_and_can_retry_without_rerunning_engine() {
        let mut host = host();
        let response = request(&mut host, Command::ReplaceInput("kya".into()));
        host.receive(response).unwrap();
        let stamp = host.document.stamp;
        host.document.reject_next = true;
        assert_eq!(host.apply_ready(), Err(HostError::DocumentRejected));
        assert_eq!(host.document.stamp, stamp);
        assert_eq!(host.document.preview(), None);
        assert_eq!(input(&host).text, "kya");
        host.apply_ready().unwrap();
        assert_eq!(host.document.preview(), Some("きゃ"));
        assert_eq!(host.document.committed, "既存:");
    }

    #[test]
    fn definitive_atomic_commit_failure_keeps_input_and_retry_commits_exactly_once() {
        let mut host = host();
        replace(&mut host, "n");
        let response = request(&mut host, Command::Commit);
        let duplicate = response.clone();
        host.receive(response).unwrap();
        let stamp = host.document.stamp;
        host.document.reject_next = true;
        assert_eq!(host.apply_ready(), Err(HostError::DocumentRejected));
        assert_eq!(host.document.stamp, stamp);
        assert_eq!(host.document.committed, "既存:");
        assert_eq!(host.document.preview(), Some("n"));
        assert_eq!(input(&host).text, "n");
        assert_eq!(host.document.commits, 0);
        host.apply_ready().unwrap();
        assert_eq!(host.document.committed, "既存:ん");
        assert_eq!(host.document.commits, 1);
        assert_eq!(host.receive(duplicate), Err(HostError::StaleResponse));
    }

    #[test]
    fn input_changed_after_failed_commit_invalidates_its_retry() {
        let mut host = host();
        replace(&mut host, "n");
        let response = request(&mut host, Command::Commit);
        host.receive(response.clone()).unwrap();
        host.document.reject_next = true;
        assert_eq!(host.apply_ready(), Err(HostError::DocumentRejected));
        let current = request(&mut host, Command::ReplaceInput("ka".into()));
        assert_eq!(host.receive(response), Err(HostError::StaleResponse));
        host.receive(current).unwrap();
        host.apply_ready().unwrap();
        deliver(&mut host, Command::Commit).unwrap();
        assert_eq!(host.document.committed, "既存:か");
        assert_eq!(host.document.commits, 1);
    }

    #[test]
    fn failed_cancel_keeps_input_but_invalidates_earlier_commit_intent() {
        let mut host = host();
        replace(&mut host, "n");
        let commit = request(&mut host, Command::Commit);
        host.receive(commit.clone()).unwrap();
        host.document.reject_next = true;
        assert!(matches!(
            host.dispatch(Command::Cancel),
            Err(HostError::DocumentRejected)
        ));
        assert_eq!(input(&host).text, "n");
        assert_eq!(host.document.preview(), Some("n"));
        assert_eq!(host.document.committed, "既存:");
        assert_eq!(host.receive(commit), Err(HostError::StaleResponse));
        assert_eq!(host.apply_ready(), Err(HostError::NoReadyResponse));
        deliver(&mut host, Command::Cancel).unwrap();
        assert!(host.composition.is_none());
        assert_eq!(host.document.preview(), None);
        assert_eq!(host.document.commits, 0);
    }

    #[test]
    fn conversion_failures_keep_input_and_previous_preview_without_committing() {
        let mut host = Host::new(&strict_graph(), "既存:").unwrap();
        replace(&mut host, "ka");
        let response = request(&mut host, Command::ReplaceInput("q".into()));
        assert!(!response.report.succeeded());
        assert_eq!(host.receive(response), Err(HostError::ConversionFailed));
        assert_eq!(input(&host).text, "q");
        assert_eq!(host.document.preview(), Some("か"));
        let commit = request(&mut host, Command::Commit);
        assert_eq!(host.receive(commit), Err(HostError::ConversionFailed));
        assert_eq!(host.document.committed, "既存:");
        assert_eq!(host.document.commits, 0);
    }

    #[test]
    fn unavailable_or_malformed_outputs_never_clear_the_document_or_preview() {
        for injected in 0..4 {
            let mut host = host();
            replace(&mut host, "ka");
            let mut response = request(&mut host, Command::ReplaceInput("kya".into()));
            let report = Arc::get_mut(&mut response.report).unwrap();
            let outputs = &mut report.nodes.get_mut("romaji").unwrap().outputs;
            match injected {
                0 => {
                    outputs.insert("out".into(), ResolvedValue::NoOutput);
                }
                1 => {
                    outputs.remove("out");
                }
                2 => {
                    outputs.insert(
                        "out".into(),
                        ResolvedValue::Data(Arc::new(Packet::new(Candidate::Sequence {
                            items: vec![],
                        }))),
                    );
                }
                _ => {
                    outputs.insert(
                        "out".into(),
                        ResolvedValue::Data(Arc::new(Packet {
                            version: 99,
                            root: Candidate::Text {
                                text: "".into(),
                                meta: None,
                            },
                        })),
                    );
                }
            }
            assert_eq!(host.receive(response), Err(HostError::NoUsableOutput));
            assert_eq!(host.apply_ready(), Err(HostError::NoReadyResponse));
            assert_eq!(host.document.committed, "既存:");
            assert_eq!(host.document.preview(), Some("か"));
            assert_eq!(input(&host).text, "kya");
        }
    }

    #[test]
    fn commit_rejects_nonempty_pending_even_if_result_was_otherwise_successful() {
        let mut host = host();
        replace(&mut host, "n");
        let mut response = request(&mut host, Command::Commit);
        Arc::get_mut(&mut response.report)
            .unwrap()
            .nodes
            .get_mut("romaji")
            .unwrap()
            .outputs
            .insert(
                "pending".into(),
                ResolvedValue::Data(Arc::new(Packet::new(Candidate::Text {
                    text: "n".into(),
                    meta: None,
                }))),
            );
        assert_eq!(
            host.receive(response),
            Err(HostError::IncompleteFinalization)
        );
        assert_eq!(host.document.committed, "既存:");
        assert_eq!(input(&host).text, "n");
    }

    #[test]
    fn prepare_can_complete_during_composition_but_activation_and_rollback_wait() {
        let mut host = host();
        replace(&mut host, "n");
        let response = request(&mut host, Command::ReplaceInput("n".into()));
        host.prepare(&strict_graph()).unwrap();
        assert_eq!(host.activate(), Err(HostError::BusyComposition));
        assert_eq!(host.rollback(), Err(HostError::BusyComposition));
        assert_eq!(host.engine.plan_version(), 1);
        host.receive(response).unwrap();
        host.apply_ready().unwrap();
        deliver(&mut host, Command::Commit).unwrap();
        assert!(host.activate().unwrap());
        assert_eq!(host.engine.plan_version(), 2);
        let strict = request(&mut host, Command::ReplaceInput("q".into()));
        assert_eq!(host.receive(strict), Err(HostError::ConversionFailed));
        host.prepare(&strict_graph()).unwrap();
        assert_eq!(host.rollback(), Err(HostError::BusyComposition));
        deliver(&mut host, Command::Cancel).unwrap();
        assert!(host.rollback().unwrap());
        assert_eq!(host.engine.plan_version(), 3);
        assert!(!host.activate().unwrap()); // 元Engineのrollbackはpendingも破棄する。
        replace(&mut host, "q");
        assert_eq!(host.document.preview(), Some("q"));
        assert_eq!(host.document.committed, "既存:ん");
    }

    #[test]
    fn failed_prepare_preserves_active_input_and_an_earlier_pending_plan() {
        let mut host = host();
        let response = request(&mut host, Command::ReplaceInput("n".into()));
        host.prepare(&strict_graph()).unwrap();
        let mut invalid = graph();
        invalid.edges.clear();
        assert!(matches!(host.prepare(&invalid), Err(HostError::Core(_))));
        assert_eq!(host.engine.plan_version(), 1);
        host.receive(response).unwrap();
        host.apply_ready().unwrap();
        deliver(&mut host, Command::Commit).unwrap();
        assert!(host.activate().unwrap());
        let response = request(&mut host, Command::ReplaceInput("q".into()));
        assert_eq!(host.receive(response), Err(HostError::ConversionFailed));
    }

    #[test]
    fn forged_report_metadata_and_external_plan_changes_are_rejected() {
        for mismatch in 0..3 {
            let mut host = host();
            let mut response = request(&mut host, Command::ReplaceInput("kya".into()));
            let report = Arc::get_mut(&mut response.report).unwrap();
            match mismatch {
                0 => report.input_revision = Some(999),
                1 => report.plan_version += 1,
                _ => report.run_id += 1,
            }
            assert_eq!(host.receive(response), Err(HostError::StaleResponse));
            assert_eq!(host.document.committed, "既存:");
            assert_eq!(input(&host).text, "kya");
        }
        let mut host = host();
        let response = request(&mut host, Command::ReplaceInput("kya".into()));
        host.engine.prepare(&graph(), &host.registry).unwrap();
        host.engine.activate_pending().unwrap(); // 試験だけでホストの切替ガードを迂回する。
        assert_eq!(host.receive(response), Err(HostError::StaleResponse));
        assert_eq!(host.document.commits, 0);
    }

    #[test]
    fn invalid_input_does_not_discard_an_existing_ready_result_or_input() {
        let mut host = host();
        let response = request(&mut host, Command::ReplaceInput("n".into()));
        host.receive(response).unwrap();
        let revision = input(&host).revision;
        let latest = host.latest.clone();
        assert!(matches!(
            host.dispatch(Command::ReplaceInput(
                "𠮷".repeat(MAX_INPUT_CODE_POINTS + 1)
            )),
            Err(HostError::Core(_))
        ));
        assert_eq!(input(&host).revision, revision);
        assert_eq!(input(&host).text, "n");
        assert_eq!(host.latest, latest);
        host.apply_ready().unwrap();
        assert_eq!(host.document.preview(), Some("n"));
    }

    #[test]
    fn counters_reject_exhaustion_before_creating_or_replacing_host_state() {
        let mut host = host();
        host.revision = MAX_INPUT_REVISION;
        assert!(matches!(
            host.dispatch(Command::ReplaceInput("a".into())),
            Err(HostError::Core(_))
        ));
        assert!(host.composition.is_none());
        host.revision = 0;
        host.generation = u64::MAX;
        assert!(matches!(
            host.dispatch(Command::ReplaceInput("a".into())),
            Err(HostError::CounterExhausted)
        ));
        assert!(host.composition.is_none());
        assert_eq!(host.revision, 0);
        assert_eq!(host.document.committed, "既存:");
    }

    #[test]
    fn fake_document_owner_check_protects_a_foreign_preedit_at_the_final_write() {
        let mut host = host();
        let response = request(&mut host, Command::ReplaceInput("ka".into()));
        host.receive(response).unwrap();
        host.document.preedit = Some(Preedit {
            owner: 999,
            text: "他の未確定文字列".into(),
        });
        assert_eq!(host.apply_ready(), Err(HostError::DocumentChanged));
        assert_eq!(input(&host).text, "ka");
        deliver(&mut host, Command::Cancel).unwrap();
        assert_eq!(host.document.preview(), Some("他の未確定文字列"));
        assert_eq!(host.document.committed, "既存:");
    }

    #[test]
    fn security_command_like_output_is_committed_as_literal_text_only() {
        let mut host = host();
        replace(&mut host, "a");
        let mut response = request(&mut host, Command::Commit);
        let literal = "<script>canary</script> $(canary) & canary\u{0}\u{1b}[31m日本語😀";
        Arc::get_mut(&mut response.report)
            .unwrap()
            .nodes
            .get_mut("romaji")
            .unwrap()
            .outputs
            .insert(
                "out".into(),
                ResolvedValue::Data(Arc::new(Packet::new(Candidate::Text {
                    text: literal.into(),
                    meta: None,
                }))),
            );
        host.receive(response).unwrap();
        host.apply_ready().unwrap();
        assert_eq!(host.document.committed, format!("既存:{literal}"));
        assert_eq!(host.document.commits, 1);
        assert_eq!(host.apply_ready(), Err(HostError::NoReadyResponse));
    }
}
