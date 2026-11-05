use diy_ime2_core::{
    error_input::{ErrorAwareExecution, ErrorAwareValue},
    execution_control::{
        CancellationToken, CompletionPermit, CompletionWakeStatus, DecimalId, DeferredNode,
        DispatchEvent, DispatchQueue, ExecutionHost, GraphExecution, GraphNodePhase,
        OwnedInvocation, ResourcePolicy, RunLedger, SessionEpoch, StartRequest,
    },
    graph::{ExecutionPlan, Graph},
    node::{CoreError, NodeDefinition, Port, Registry},
};
use serde_json::{Value, json};
use std::{
    collections::BTreeMap,
    sync::{
        atomic::{AtomicU64, Ordering},
        mpsc,
    },
    time::{Duration, Instant},
};

struct Started {
    permit: CompletionPermit,
    release: mpsc::Sender<()>,
    cancel: CancellationToken,
}
struct Deferred(mpsc::Sender<(String, Started)>);
impl DeferredNode for Deferred {
    fn definition(&self) -> NodeDefinition {
        NodeDefinition::new("test.wake", 1, vec![], vec![Port::packet("out")])
    }
    fn start(
        &self,
        inv: OwnedInvocation,
        permit: CompletionPermit,
        cancel: CancellationToken,
    ) -> Result<(), CoreError> {
        let (release, wait) = mpsc::channel();
        self.0
            .send((
                inv.context().node_id.to_owned(),
                Started {
                    permit,
                    release,
                    cancel,
                },
            ))
            .unwrap();
        wait.recv().unwrap();
        Ok(())
    }
}
fn fixture() -> (
    ExecutionHost,
    RunLedger,
    DispatchQueue,
    GraphExecution,
    BTreeMap<String, Started>,
) {
    static NEXT: AtomicU64 = AtomicU64::new(1);
    let policy =
        ResourcePolicy::from_value(
            serde_json::from_str::<Value>(include_str!("contracts/execution_control.json"))
                .unwrap()["resourceProfiles"]["bounded"]
                .clone(),
        )
        .unwrap();
    let host = ExecutionHost::new(
        format!("wake-test-{}", NEXT.fetch_add(1, Ordering::Relaxed)),
        policy,
    )
    .unwrap();
    let (send, entered) = mpsc::channel();
    let mut registry = Registry::new();
    registry.register_deferred(Deferred(send)).unwrap();
    let graph = Graph::from_json(&json!({"version":1,"nodes":[{"id":"a","kind":"test.wake","version":1},{"id":"b","kind":"test.wake","version":1}],"edges":[]}).to_string()).unwrap();
    let mut ledger = RunLedger::new(
        host.clone(),
        SessionEpoch::new("0123456789abcdef0123456789abcdef").unwrap(),
        ExecutionPlan::compile(&graph, &registry).unwrap(),
        DecimalId::new(1).unwrap(),
    );
    let mut queue = DispatchQueue::new(host.clone()).unwrap();
    let request = serde_json::from_value::<StartRequest>(json!({"startKey":"1","expectedPlanVersion":"1","targets":[{"node":"a","port":"out"},{"node":"b","port":"out"}]})).unwrap();
    let admitted = queue.admit(&mut ledger, request).unwrap();
    assert!(admitted.events.is_empty());
    let started = (0..2)
        .map(|_| entered.recv_timeout(Duration::from_secs(5)).unwrap())
        .collect();
    (host, ledger, queue, admitted.graph.unwrap(), started)
}
fn result() -> ErrorAwareExecution {
    ErrorAwareExecution::new([("out".into(), ErrorAwareValue::NoOutput)].into())
}
fn next(queue: &mut DispatchQueue) -> DispatchEvent {
    let end = Instant::now() + Duration::from_secs(5);
    loop {
        if let Some(event) = queue.poll_one() {
            return event;
        }
        assert_eq!(
            queue
                .wake_signal()
                .wait(Some(end.saturating_duration_since(Instant::now())))
                .unwrap(),
            CompletionWakeStatus::Notified
        );
    }
}

#[test]
fn waking_does_not_adopt_and_poll_one_preserves_each_public_completion_boundary() {
    let (host, _ledger, mut queue, mut graph, started) = fixture();
    let wake = queue.wake_signal();
    assert!(started["b"].permit.complete(Ok(result())));
    assert!(started["a"].permit.complete(Ok(result())));
    assert_eq!(
        wake.wait(Some(Duration::from_secs(5))).unwrap(),
        CompletionWakeStatus::Notified
    );
    assert_eq!(graph.phase("b"), Some(GraphNodePhase::Active));
    assert_eq!(host.usage().physical_jobs, 2);
    let b = queue.poll_one().unwrap();
    assert_eq!(b.stamp().job_id, started["b"].permit.job_id());
    assert!(!b.completion().unwrap().physically_finished());
    b.apply(&mut graph).unwrap();
    assert!(!graph.is_complete());
    assert_eq!(graph.phase("a"), Some(GraphNodePhase::Active));
    queue.poll_one().unwrap().apply(&mut graph).unwrap();
    assert!(graph.is_complete());
    assert!(queue.poll_one().is_none());
    assert_eq!(
        wake.wait(Some(Duration::ZERO)).unwrap(),
        CompletionWakeStatus::TimedOut
    );
    for start in started.values() {
        start.release.send(()).unwrap();
    }
    for _ in 0..2 {
        assert!(next(&mut queue).completion().unwrap().physically_finished());
    }
    assert_eq!(host.usage().physical_jobs, 0);
}

#[test]
fn physical_start_return_wakes_after_logical_completion_and_reclaims_only_that_job() {
    let (host, _ledger, mut queue, _graph, started) = fixture();
    assert!(started["a"].permit.complete(Ok(result())));
    let logical = next(&mut queue);
    assert!(logical.completion().unwrap().execution().is_some());
    assert!(!logical.completion().unwrap().physically_finished());
    started["a"].release.send(()).unwrap();
    let ended = next(&mut queue);
    assert_eq!(ended.stamp().job_id, started["a"].permit.job_id());
    assert!(ended.completion().unwrap().execution().is_none());
    assert!(ended.completion().unwrap().physically_finished());
    assert_eq!(host.usage().physical_jobs, 1);
    queue
        .cancel_run(started["b"].permit.key(), Duration::ZERO)
        .unwrap();
    assert!(started["b"].cancel.is_cancelled());
    assert!(started["b"].permit.finish_cancelled());
    started["b"].release.send(()).unwrap();
    assert!(next(&mut queue).completion().unwrap().physically_finished());
    assert_eq!(host.usage().physical_jobs, 0);
}

#[test]
fn dispatcher_drop_closes_waiters_but_does_not_release_live_producer_ownership() {
    let (host, ledger, queue, graph, started) = fixture();
    let wake = queue.wake_signal();
    drop(queue);
    assert_eq!(wake.wait(None).unwrap(), CompletionWakeStatus::Closed);
    assert_eq!(host.usage().physical_jobs, 2);
    for start in started.values() {
        assert!(start.cancel.is_cancelled());
        assert!(!start.permit.finish_cancelled()); // no receiver, still releases its producer ownership
        start.release.send(()).unwrap();
    }
    drop(graph);
    drop(ledger);
    let end = Instant::now() + Duration::from_secs(5);
    while host.usage().physical_jobs != 0 {
        assert!(Instant::now() < end);
        std::thread::yield_now();
    }
    assert_eq!(
        wake.wait(Some(Duration::ZERO)).unwrap(),
        CompletionWakeStatus::Closed
    );
}

#[test]
fn legacy_batch_poll_keeps_fifo_and_cancellation_suppresses_previously_sent_results() {
    let (host, _ledger, mut queue, _graph, started) = fixture();
    assert!(started["b"].permit.complete(Ok(result())));
    assert!(started["a"].permit.complete(Ok(result())));
    queue
        .cancel_run(started["a"].permit.key(), Duration::ZERO)
        .unwrap();
    assert!(queue.poll().is_empty()); // both starts still physically alive, both results cancelled
    for start in started.values() {
        start.release.send(()).unwrap();
    }
    for _ in 0..2 {
        assert!(next(&mut queue).completion().unwrap().execution().is_none());
    }
    assert_eq!(host.usage().physical_jobs, 0);
}
