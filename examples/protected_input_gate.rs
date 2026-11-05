//! Fake input adapter demonstration; actual SessionV3 integration is tested separately.
#[path = "security_support/input_gate.rs"]
mod input_gate;
use input_gate::{Denial, InputGate};
fn main() {
    let mut gate = InputGate::default();
    let (old, text) = gate.dispatch(true, || "normal-canary").unwrap();
    assert_eq!(text, "normal-canary");
    assert!(gate.accepts(&old));
    gate.set_manual(true, || Ok::<_, ()>(())).unwrap();
    gate.set_automatic(true, || Ok::<_, ()>(())).unwrap();
    assert!(gate.manual() && gate.automatic());
    assert!(matches!(
        gate.dispatch(true, || panic!("must not deliver")),
        Err(Denial::Protected)
    ));
    gate.set_manual(false, || Ok::<_, ()>(())).unwrap();
    assert!(gate.blocked());
    gate.set_automatic(false, || Ok::<_, ()>(())).unwrap();
    assert!(!gate.accepts(&old));
    assert_eq!(
        gate.dispatch(true, || "fresh-input-only").unwrap().1,
        "fresh-input-only"
    );
    println!("Mock protection: independent OR flags, no delivery, stale result rejected");
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn manual_and_automatic_are_independent_and_neither_can_clear_the_other() {
        let mut g = InputGate::default();
        for (manual, auto) in [
            (false, false),
            (true, false),
            (true, true),
            (false, true),
            (true, true),
            (true, false),
            (false, false),
        ] {
            g.set_manual(manual, || Ok::<_, ()>(())).unwrap();
            g.set_automatic(auto, || Ok::<_, ()>(())).unwrap();
            assert_eq!(g.manual(), manual);
            assert_eq!(g.automatic(), auto);
            assert_eq!(g.blocked(), manual || auto);
            let mut calls = 0;
            let result = g.dispatch(true, || {
                calls += 1;
            });
            assert_eq!(calls, usize::from(!g.blocked()));
            assert_eq!(result.is_ok(), !g.blocked());
        }
    }
    #[test]
    fn new_protected_input_never_reaches_node_learning_logs_or_api_and_direct_input_survives() {
        let mut g = InputGate::default();
        g.set_manual(true, || Ok::<_, ()>(())).unwrap();
        let input = ("body-canary", "key-canary", "context-canary");
        let mut sinks = vec![];
        let mut direct = String::new();
        for allowed in [false, true] {
            let result = g.dispatch(allowed, || sinks.push(input));
            assert!(matches!(result, Err(Denial::Protected)));
            direct.push_str(input.0); // trusted direct path; no graph/learning/log delivery
        }
        assert!(sinks.is_empty());
        assert_eq!(direct, "body-canarybody-canary");
    }
    #[test]
    fn failure_to_invalidate_blocks_even_after_explicit_exit_and_retry_recovers() {
        let mut g = InputGate::default();
        let old = g.dispatch(true, || ()).unwrap().0;
        assert_eq!(g.set_manual(true, || Err("host-lost")), Err("host-lost"));
        assert_eq!(g.set_manual(false, || Err("host-lost")), Err("host-lost"));
        assert!(!g.manual());
        assert!(g.blocked());
        assert!(!g.accepts(&old));
        assert!(matches!(g.dispatch(true, || ()), Err(Denial::Protected)));
        g.set_manual(false, || Ok::<_, ()>(())).unwrap();
        assert!(!g.blocked());
        assert!(!g.accepts(&old));
        assert!(g.dispatch(true, || ()).is_ok());
    }
    #[test]
    fn old_tickets_do_not_revive_on_exit_repeated_transitions_or_another_gate() {
        let mut g = InputGate::default();
        let old = g.dispatch(true, || ()).unwrap().0;
        assert!(!InputGate::default().accepts(&old));
        for _ in 0..32 {
            g.set_automatic(true, || Ok::<_, ()>(())).unwrap();
            g.set_automatic(false, || Ok::<_, ()>(())).unwrap();
        }
        assert!(!g.accepts(&old));
        assert!(matches!(g.dispatch(false, || ()), Err(Denial::NotAllowed)));
        let new = g.dispatch(true, || ()).unwrap().0;
        assert!(g.accepts(&new));
    }
}
