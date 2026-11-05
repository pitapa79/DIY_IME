//! Cooperative malicious Mock Nodes through the real Engine and private host kernels.
//! All resources below are memory canaries. This is not an OS sandbox test.
#[path = "../examples/security_support/authority.rs"]
mod authority;
#[path = "../examples/security_support/input_gate.rs"]
mod input_gate;
use authority::{Authority, Binding, Permit};
use diy_ime2_core::{
    Candidate, Packet,
    graph::{ExecutionPlan, Graph},
    node::{
        CoreError, InputMode, Inputs, Node, NodeDefinition, OutputValue, Outputs, Port, Registry,
        RunContext, RuntimeInput,
    },
    runtime::{Engine, RunReport},
};
use input_gate::{Denial, InputGate, Ticket};
use serde_json::{Value, json};
use std::sync::{
    Arc, Mutex,
    atomic::{AtomicUsize, Ordering},
};
const OPS: [&str; 6] = [
    "mock.input",
    "mock.read",
    "mock.write",
    "mock.connect",
    "mock.spawn",
    "mock.clipboard",
];
#[derive(Default)]
struct Probe {
    calls: AtomicUsize,
    seen: Mutex<Vec<String>>,
    effects: Mutex<Vec<String>>,
}
struct Attacker {
    authority: Arc<Mutex<Authority>>,
    binding: Binding,
    permit: Option<Permit>,
    probe: Arc<Probe>,
}
impl Node for Attacker {
    fn definition(&self) -> NodeDefinition {
        NodeDefinition::new("test.attacker", 1, vec![], vec![Port::packet("out")])
    }
    fn validate_config(&self, value: &Value) -> Result<(), CoreError> {
        if value.is_object() {
            Ok(())
        } else {
            Err(CoreError::new("invalid-config", "object required"))
        }
    }
    fn execute(
        &self,
        context: RunContext<'_>,
        _: &Inputs,
        _: &Value,
    ) -> Result<Outputs, CoreError> {
        let text = &context.input.unwrap().text;
        self.probe.calls.fetch_add(1, Ordering::SeqCst);
        self.probe.seen.lock().unwrap().push(text.clone());
        for op in &OPS[1..] {
            // Mock broker owns the policy and binding; Node config cannot issue authority.
            if self
                .authority
                .lock()
                .unwrap()
                .allows(&self.binding, self.permit.as_ref(), op)
            {
                self.probe
                    .effects
                    .lock()
                    .unwrap()
                    .push(format!("{op}:{text}"));
            }
        }
        Ok([(
            "out".into(),
            OutputValue::Data(Arc::new(Packet::new(Candidate::Text {
                text: text.clone(),
                meta: None,
            }))),
        )]
        .into())
    }
}
fn binding() -> Binding {
    Binding {
        principal: "mock-author".into(),
        node: "probe".into(),
        artifact: "v1".into(),
        graph: "g1".into(),
        session: "s1".into(),
        run: "r1".into(),
    }
}
fn configure(approved: &[&str]) -> (Arc<Mutex<Authority>>, Binding, Permit) {
    let a = Arc::new(Mutex::new(Authority::default()));
    let b = binding();
    a.lock().unwrap().replace(b.clone(), &OPS, approved);
    let permit = a.lock().unwrap().issue(&b).unwrap();
    (a, b, permit)
}
fn engine(
    a: &Arc<Mutex<Authority>>,
    b: &Binding,
    p: Option<&Permit>,
    probe: &Arc<Probe>,
    config: Value,
) -> Engine {
    let mut registry = Registry::new();
    registry
        .register(Attacker {
            authority: a.clone(),
            binding: b.clone(),
            permit: p.cloned(),
            probe: probe.clone(),
        })
        .unwrap();
    let graph=Graph::from_json(&json!({"version":1,"nodes":[{"id":"probe","kind":"test.attacker","version":1,"config":config}],"edges":[]}).to_string()).unwrap();
    Engine::new(ExecutionPlan::compile(&graph, &registry).unwrap())
}
fn attempt(
    gate: &InputGate,
    a: &Arc<Mutex<Authority>>,
    b: &Binding,
    p: Option<&Permit>,
    engine: &mut Engine,
    text: &str,
    revision: u64,
) -> Result<(Ticket, RunReport), Denial> {
    let allowed = a.lock().unwrap().allows(b, p, "mock.input");
    gate.dispatch(allowed, || {
        engine
            .run_with_input(&RuntimeInput {
                revision,
                text: text.into(),
                mode: InputMode::Compose,
            })
            .unwrap()
    })
}
#[test]
fn no_permission_and_forged_config_cannot_invoke_node_or_expand_side_effects() {
    let (a, b, p) = configure(&[]);
    let probe = Arc::new(Probe::default());
    let gate = InputGate::default();
    let mut engine = engine(
        &a,
        &b,
        Some(&p),
        &probe,
        json!({"trusted":true,"permissions":["*"],"protected":false,"principal":"admin"}),
    );
    for _ in 0..256 {
        assert!(matches!(
            attempt(&gate, &a, &b, Some(&p), &mut engine, "denied-canary", 1),
            Err(Denial::NotAllowed)
        ));
    }
    assert_eq!(probe.calls.load(Ordering::SeqCst), 0);
    assert!(probe.seen.lock().unwrap().is_empty());
    assert!(probe.effects.lock().unwrap().is_empty());
}
#[test]
fn explicit_input_and_read_allow_normal_data_but_not_write_network_spawn_or_clipboard() {
    let (a, b, p) = configure(&["mock.input", "mock.read"]);
    let probe = Arc::new(Probe::default());
    let gate = InputGate::default();
    let mut engine = engine(
        &a,
        &b,
        Some(&p),
        &probe,
        json!({"permissions":OPS,"trusted":true}),
    );
    let (ticket, report) =
        attempt(&gate, &a, &b, Some(&p), &mut engine, "normal-canary", 1).unwrap();
    assert!(report.succeeded());
    assert!(gate.accepts(&ticket));
    assert_eq!(probe.calls.load(Ordering::SeqCst), 1);
    assert_eq!(*probe.effects.lock().unwrap(), ["mock.read:normal-canary"]);
    assert_eq!(
        report.packet("probe", "out").unwrap().root,
        Candidate::Text {
            text: "normal-canary".into(),
            meta: None
        }
    );
}
#[test]
fn all_mock_grants_are_overridden_by_either_protection_flag_and_direct_input_remains() {
    for (manual, automatic) in [(true, false), (false, true), (true, true)] {
        let (a, b, p) = configure(&OPS);
        let probe = Arc::new(Probe::default());
        let mut gate = InputGate::default();
        gate.set_manual(manual, || Ok::<_, ()>(())).unwrap();
        gate.set_automatic(automatic, || Ok::<_, ()>(())).unwrap();
        assert_eq!(gate.manual(), manual);
        assert_eq!(gate.automatic(), automatic);
        let mut engine = engine(
            &a,
            &b,
            Some(&p),
            &probe,
            json!({"protected":false,"permissions":["*"]}),
        );
        let frame = (
            "protected-body-canary",
            "key-event-canary",
            "surrounding-context-canary",
        );
        let mut direct = String::new();
        for _ in 0..16 {
            assert!(matches!(
                attempt(&gate, &a, &b, Some(&p), &mut engine, frame.0, 1),
                Err(Denial::Protected)
            ));
            direct.push_str(frame.0); // trusted fake direct path, never Node/learning/log/API
        }
        assert!(gate.blocked());
        assert!(!direct.is_empty());
        assert_eq!(probe.calls.load(Ordering::SeqCst), 0);
        assert!(probe.seen.lock().unwrap().is_empty());
        assert!(probe.effects.lock().unwrap().is_empty());
    }
}
#[test]
fn stolen_permits_cannot_cross_any_host_binding_or_a_new_authority() {
    let (a, b, p) = configure(&OPS);
    let gate = InputGate::default();
    let probe = Arc::new(Probe::default());
    for i in 0..6 {
        let mut wrong = b.clone();
        let field = match i {
            0 => &mut wrong.principal,
            1 => &mut wrong.node,
            2 => &mut wrong.artifact,
            3 => &mut wrong.graph,
            4 => &mut wrong.session,
            _ => &mut wrong.run,
        };
        *field = "forged".into();
        let mut engine = engine(&a, &wrong, Some(&p), &probe, json!({}));
        assert!(matches!(
            attempt(
                &gate,
                &a,
                &wrong,
                Some(&p),
                &mut engine,
                "cross-binding-canary",
                1
            ),
            Err(Denial::NotAllowed)
        ));
    }
    let (other, _, _) = configure(&OPS);
    let mut engine = engine(&other, &b, Some(&p), &probe, json!({}));
    assert!(matches!(
        attempt(
            &gate,
            &other,
            &b,
            Some(&p),
            &mut engine,
            "cross-host-canary",
            1
        ),
        Err(Denial::NotAllowed)
    ));
    assert_eq!(probe.calls.load(Ordering::SeqCst), 0);
}
#[test]
fn revocation_recreation_and_policy_rollback_do_not_revive_old_engine_permits() {
    let (a, b, p) = configure(&OPS);
    let gate = InputGate::default();
    let probe = Arc::new(Probe::default());
    let mut old_engine = engine(&a, &b, Some(&p), &probe, json!({}));
    assert!(attempt(&gate, &a, &b, Some(&p), &mut old_engine, "before-revoke", 1).is_ok());
    a.lock().unwrap().revoke(&b);
    assert!(matches!(
        attempt(&gate, &a, &b, Some(&p), &mut old_engine, "revoked", 2),
        Err(Denial::NotAllowed)
    ));
    a.lock().unwrap().replace(b.clone(), &OPS, &OPS);
    assert!(matches!(
        attempt(&gate, &a, &b, Some(&p), &mut old_engine, "rolled-back", 3),
        Err(Denial::NotAllowed)
    ));
    let fresh = a.lock().unwrap().issue(&b).unwrap();
    let mut new_engine = engine(&a, &b, Some(&fresh), &probe, json!({}));
    assert!(
        attempt(
            &gate,
            &a,
            &b,
            Some(&fresh),
            &mut new_engine,
            "fresh-after-approval",
            4
        )
        .is_ok()
    );
    assert_eq!(
        *probe.seen.lock().unwrap(),
        ["before-revoke", "fresh-after-approval"]
    );
}
#[test]
fn retained_result_ticket_is_rejected_after_protection_and_only_fresh_input_resumes() {
    let (a, b, p) = configure(&OPS);
    let probe = Arc::new(Probe::default());
    let mut gate = InputGate::default();
    let mut engine = engine(&a, &b, Some(&p), &probe, json!({}));
    let (old, report) = attempt(&gate, &a, &b, Some(&p), &mut engine, "old-normal", 1).unwrap();
    assert_eq!(
        gate.set_manual(true, || Err("invalidate-failed")),
        Err("invalidate-failed")
    );
    gate.set_automatic(true, || Ok::<_, ()>(())).unwrap();
    gate.set_manual(false, || Ok::<_, ()>(())).unwrap();
    assert!(gate.automatic());
    assert!(matches!(
        attempt(&gate, &a, &b, Some(&p), &mut engine, "protected-secret", 2),
        Err(Denial::Protected)
    ));
    assert_eq!(
        gate.set_automatic(false, || Err("host-lost")),
        Err("host-lost")
    );
    assert!(matches!(
        attempt(
            &gate,
            &a,
            &b,
            Some(&p),
            &mut engine,
            "still-protected-secret",
            3
        ),
        Err(Denial::Protected)
    ));
    gate.set_automatic(false, || Ok::<_, ()>(())).unwrap();
    let mut adopted = vec![];
    if gate.accepts(&old) {
        adopted.push(report);
    }
    assert!(adopted.is_empty());
    let (fresh, report) = attempt(&gate, &a, &b, Some(&p), &mut engine, "fresh-normal", 4).unwrap();
    assert!(gate.accepts(&fresh));
    assert!(report.succeeded());
    assert_eq!(*probe.seen.lock().unwrap(), ["old-normal", "fresh-normal"]);
}
