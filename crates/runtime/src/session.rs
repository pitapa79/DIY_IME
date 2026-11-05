//! 常駐 CLI 用のバージョン付き JSON Lines プロトコル。
//! 1セッションにつき1エンジンを保持し、コマンドの間を計画切替の境界とする。
use std::io::{self, BufRead, Read, Write};

use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

use crate::{
    graph::{ExecutionPlan, Graph},
    node::{CoreError, Registry, RuntimeInput},
    runtime::Engine,
};

pub const PROTOCOL_VERSION: u32 = 2;
pub const MAX_REQUEST_BYTES: usize = crate::execution_control::protocol::MAX_REQUEST_BYTES;

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Envelope {
    protocol: u32,
    id: u64,
    request: Value,
}

#[derive(Deserialize)]
#[serde(tag = "method", rename_all = "kebab-case", deny_unknown_fields)]
pub enum Request {
    Status {},
    Definitions {},
    Validate {
        graph: Graph,
    },
    Prepare {
        graph: Graph,
    },
    Activate {
        token: String,
    },
    Run {
        #[serde(rename = "expectedPlanVersion")]
        expected_plan_version: u64,
        #[serde(default, deserialize_with = "run_input")]
        input: Option<Value>,
    },
    Rollback {
        #[serde(rename = "expectedPlanVersion")]
        expected_plan_version: u64,
    },
}

// 省略(None)と明示的null(Some(Null))を分け、版検査の後に入力エラーを返す。
fn run_input<'de, D: serde::Deserializer<'de>>(deserializer: D) -> Result<Option<Value>, D::Error> {
    Value::deserialize(deserializer).map(Some)
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ActivePlan {
    pub version: u64,
    pub graph: Graph,
}

#[derive(Clone, Serialize)]
pub struct PreparedPlan {
    pub token: String,
    pub graph: Graph,
    pub order: Vec<String>,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SessionState {
    pub revision: u64,
    pub active: Option<ActivePlan>,
    pub prepared: Option<PreparedPlan>,
    pub can_rollback: bool,
}

pub struct Session {
    resource_root: Option<std::sync::Arc<crate::resources::ResourceRoot>>,
    registry: Registry,
    engine: Option<Engine>,
    initial: Option<ExecutionPlan>,
    active_graph: Option<Graph>,
    previous_graph: Option<Graph>,
    prepared: Option<PreparedPlan>,
    next_token: u64,
    revision: u64,
}

impl Session {
    pub fn new(registry: Registry) -> Self {
        Self::with_resource_root(registry, None)
    }
    pub fn with_resource_root(
        registry: Registry,
        resource_root: Option<std::sync::Arc<crate::resources::ResourceRoot>>,
    ) -> Self {
        Self {
            resource_root,
            registry,
            engine: None,
            initial: None,
            active_graph: None,
            previous_graph: None,
            prepared: None,
            next_token: 0,
            revision: 0,
        }
    }
    pub fn state(&self) -> SessionState {
        SessionState {
            revision: self.revision,
            active: self
                .engine
                .as_ref()
                .zip(self.active_graph.as_ref())
                .map(|(engine, graph)| ActivePlan {
                    version: engine.plan_version(),
                    graph: graph.clone(),
                }),
            prepared: self.prepared.clone(),
            can_rollback: self.previous_graph.is_some(),
        }
    }
    fn next_revision(&self) -> Result<u64, CoreError> {
        self.revision
            .checked_add(1)
            .ok_or_else(|| CoreError::new("id-exhausted", "session revision exhausted"))
    }
    fn check_version(&self, expected: u64) -> Result<(), CoreError> {
        let engine = self.engine.as_ref().ok_or_else(|| {
            CoreError::new("no-active-plan", "prepare and activate a graph first")
        })?;
        if engine.plan_version() != expected {
            return Err(CoreError::new(
                "plan-conflict",
                "active plan changed; refresh the session state",
            ));
        }
        Ok(())
    }
    pub fn handle(&mut self, request: Request) -> Result<Value, CoreError> {
        match request {
            Request::Status {} => Ok(json!({})),
            Request::Definitions {} => {
                serde_json::to_value(self.registry.definitions().collect::<Vec<_>>())
                    .map_err(|error| CoreError::new("session-json", error.to_string()))
            }
            Request::Validate { graph } => {
                let plan = crate::resources::compile(
                    &graph,
                    &self.registry,
                    self.resource_root.as_deref(),
                )?;
                plan.require_protocol_v2()?;
                Ok(json!({"valid":true, "order":plan.node_order().collect::<Vec<_>>()}))
            }
            Request::Prepare { graph } => {
                let plan = crate::resources::compile(
                    &graph,
                    &self.registry,
                    self.resource_root.as_deref(),
                )?;
                plan.require_protocol_v2()?;
                let token = self
                    .next_token
                    .checked_add(1)
                    .ok_or_else(|| CoreError::new("id-exhausted", "preparation token exhausted"))?;
                let revision = self.next_revision()?;
                let order = plan.node_order().map(str::to_owned).collect();
                if let Some(engine) = self.engine.as_mut() {
                    engine.prepare_plan(plan);
                } else {
                    self.initial = Some(plan);
                }
                self.prepared = Some(PreparedPlan {
                    token: token.to_string(),
                    graph,
                    order,
                });
                self.next_token = token;
                self.revision = revision;
                Ok(json!({}))
            }
            Request::Activate { token } => {
                if self
                    .prepared
                    .as_ref()
                    .is_none_or(|prepared| prepared.token != token)
                {
                    return Err(CoreError::new(
                        "stale-preparation",
                        "prepare token no longer matches the pending plan",
                    ));
                }
                let revision = self.next_revision()?;
                if let Some(engine) = self.engine.as_mut() {
                    if !engine.activate_pending()? {
                        return Err(CoreError::new("no-pending-plan", "no plan is prepared"));
                    }
                    self.previous_graph = self.active_graph.take();
                } else {
                    let plan = self
                        .initial
                        .take()
                        .ok_or_else(|| CoreError::new("no-pending-plan", "no plan is prepared"))?;
                    self.engine = Some(Engine::new(plan));
                }
                self.active_graph = self.prepared.take().map(|prepared| prepared.graph);
                self.revision = revision;
                Ok(json!({}))
            }
            Request::Run {
                expected_plan_version,
                input,
            } => {
                self.check_version(expected_plan_version)?;
                let input = input.map(RuntimeInput::from_value).transpose()?;
                let engine = self
                    .engine
                    .as_mut()
                    .expect("version check requires an engine");
                let report = match input.as_ref() {
                    Some(input) => engine.run_with_input(input)?,
                    None => engine.run()?,
                };
                serde_json::from_str(&report.to_json()?)
                    .map_err(|error| CoreError::new("session-json", error.to_string()))
            }
            Request::Rollback {
                expected_plan_version,
            } => {
                self.check_version(expected_plan_version)?;
                let revision = self.next_revision()?;
                if !self
                    .engine
                    .as_mut()
                    .expect("version check requires an engine")
                    .rollback()?
                {
                    return Err(CoreError::new(
                        "no-rollback",
                        "no previous plan is available",
                    ));
                }
                self.active_graph = self.previous_graph.take();
                self.prepared = None;
                self.initial = None;
                self.revision = revision;
                Ok(json!({}))
            }
        }
    }
    fn reply(&mut self, line: &[u8]) -> Value {
        let envelope = match serde_json::from_slice::<Envelope>(line) {
            Ok(envelope) => envelope,
            Err(error) => {
                return json!({"protocol":PROTOCOL_VERSION,"id":null,"ok":false,"error":CoreError::new("protocol-json",error.to_string()),"state":self.state()});
            }
        };
        let outcome = if envelope.protocol != PROTOCOL_VERSION {
            Err(CoreError::new(
                "protocol-version",
                "expected session protocol 2",
            ))
        } else {
            serde_json::from_value::<Request>(envelope.request)
                .map_err(|error| CoreError::new("protocol-request", error.to_string()))
                .and_then(|request| self.handle(request))
        };
        match outcome {
            Ok(data) => {
                json!({"protocol":PROTOCOL_VERSION,"id":envelope.id,"ok":true,"data":data,"state":self.state()})
            }
            Err(error) => {
                json!({"protocol":PROTOCOL_VERSION,"id":envelope.id,"ok":false,"error":error,"state":self.state()})
            }
        }
    }
}

/// Serve an explicitly constructed Session through the same v2 codec. This does
/// not change the default Registry or opt legacy clients into new capabilities.
pub fn serve_session(
    mut session: Session,
    reader: impl BufRead,
    mut writer: impl Write,
) -> io::Result<()> {
    let mut reader = reader;
    loop {
        let mut line = Vec::new();
        let count = reader
            .by_ref()
            .take((MAX_REQUEST_BYTES + 2) as u64)
            .read_until(b'\n', &mut line)?;
        if count == 0 {
            return Ok(());
        }
        if line.last() == Some(&b'\n') {
            line.pop();
        }
        if line.len() > MAX_REQUEST_BYTES {
            writeln!(
                writer,
                "{}",
                json!({"protocol":PROTOCOL_VERSION,"id":null,"ok":false,"error":CoreError::new("request-limit","session request exceeds 1 MiB"),"state":session.state()})
            )?;
            writer.flush()?;
            return Ok(()); // 過大フレームの残りを次のコマンドとして実行しない。
        }
        writeln!(writer, "{}", session.reply(&line))?;
        writer.flush()?;
    }
}

#[cfg(test)]
mod prepared_state_tests {
    use super::*;
    use crate::node::{
        Inputs, Node, NodeDefinition, Outputs, PrepareResources, PreparedNode, RunContext,
    };
    use std::sync::{
        Arc,
        atomic::{AtomicUsize, Ordering},
    };
    struct Factory(Arc<AtomicUsize>);
    struct Prepared(Arc<AtomicUsize>);
    impl Drop for Prepared {
        fn drop(&mut self) {
            self.0.fetch_add(1, Ordering::SeqCst);
        }
    }
    impl PreparedNode for Prepared {
        fn execute(&self, _: RunContext<'_>, _: &Inputs) -> Result<Outputs, CoreError> {
            Ok(Outputs::new())
        }
    }
    impl Node for Factory {
        fn definition(&self) -> NodeDefinition {
            NodeDefinition::new("test.owned", 1, vec![], vec![])
        }
        fn prepare(
            &self,
            _: &Value,
            _: &mut dyn PrepareResources,
        ) -> Result<Option<Arc<dyn PreparedNode>>, CoreError> {
            Ok(Some(Arc::new(Prepared(Arc::clone(&self.0)))))
        }
        fn execute(&self, _: RunContext<'_>, _: &Inputs, _: &Value) -> Result<Outputs, CoreError> {
            panic!("not prepared")
        }
    }
    fn graph() -> Graph {
        Graph::from_json(
            r#"{"version":1,"nodes":[{"id":"owned","kind":"test.owned","version":1}],"edges":[]}"#,
        )
        .unwrap()
    }
    #[test]
    fn exhausted_ids_drop_only_new_snapshot_and_keep_old_initial_or_pending() {
        for active in [false, true] {
            for exhaust_token in [false, true] {
                let drops = Arc::new(AtomicUsize::new(0));
                let mut r = Registry::new();
                r.register(Factory(Arc::clone(&drops))).unwrap();
                let mut session = Session::new(r);
                session.handle(Request::Prepare { graph: graph() }).unwrap();
                if active {
                    session
                        .handle(Request::Activate { token: "1".into() })
                        .unwrap();
                    session.handle(Request::Prepare { graph: graph() }).unwrap();
                }
                let token = session.state().prepared.unwrap().token;
                if exhaust_token {
                    session.next_token = u64::MAX;
                } else {
                    session.revision = u64::MAX;
                }
                let before = serde_json::to_value(session.state()).unwrap();
                assert_eq!(
                    session
                        .handle(Request::Prepare { graph: graph() })
                        .unwrap_err()
                        .code,
                    "id-exhausted"
                );
                assert_eq!(serde_json::to_value(session.state()).unwrap(), before);
                assert_eq!(drops.load(Ordering::SeqCst), 1);
                // Restore only the artificial exhausted counter, then use the preserved token.
                if !exhaust_token {
                    session.revision = 0;
                }
                session.handle(Request::Activate { token }).unwrap();
                drop(session);
                assert_eq!(drops.load(Ordering::SeqCst), if active { 3 } else { 2 });
            }
        }
    }
}
