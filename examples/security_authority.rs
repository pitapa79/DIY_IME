//! Executable pre-IME authority kernel. No filesystem/network/OS authority.
#[path = "security_support/authority.rs"]
mod authority;
use authority::{Authority, Binding};
fn binding() -> Binding {
    Binding {
        principal: "mock-publisher".into(),
        node: "probe".into(),
        artifact: "mock-v1".into(),
        graph: "graph-1".into(),
        session: "session-1".into(),
        run: "run-1".into(),
    }
}
fn main() {
    let b = binding();
    let mut host = Authority::default();
    assert!(!host.allows(&b, None, "mock.read"));
    host.replace(b.clone(), &["mock.read"], &["mock.read"]);
    let permit = host.issue(&b).unwrap();
    assert!(host.allows(&b, Some(&permit), "mock.read"));
    host.revoke(&b);
    assert!(!host.allows(&b, Some(&permit), "mock.read"));
    println!("Mock authority: default deny, explicit allow, revocation verified");
}
#[cfg(test)]
mod tests {
    use super::*;
    fn configured() -> (Authority, Binding, authority::Permit) {
        let b = binding();
        let mut a = Authority::default();
        a.replace(
            b.clone(),
            &["mock.read", "mock.write"],
            &["mock.read", "mock.admin"],
        );
        let p = a.issue(&b).unwrap();
        (a, b, p)
    }
    #[test]
    fn default_deny_and_requested_approved_intersection() {
        let (a, b, p) = configured();
        assert!(!Authority::default().allows(&b, None, "mock.read"));
        assert!(Authority::default().issue(&b).is_none());
        assert!(!a.allows(&b, None, "mock.read"));
        assert!(a.allows(&b, Some(&p), "mock.read"));
        for op in ["mock.write", "mock.admin", "*", "", "MOCK.READ"] {
            assert!(!a.allows(&b, Some(&p), op));
        }
    }
    #[test]
    fn every_binding_component_is_checked_independently() {
        let (a, b, p) = configured();
        for i in 0..6 {
            let mut forged = b.clone();
            let field = match i {
                0 => &mut forged.principal,
                1 => &mut forged.node,
                2 => &mut forged.artifact,
                3 => &mut forged.graph,
                4 => &mut forged.session,
                _ => &mut forged.run,
            };
            *field = "other".into();
            assert!(!a.allows(&forged, Some(&p), "mock.read"));
        }
        assert!(a.allows(&b, Some(&p), "mock.read"));
    }
    #[test]
    fn same_identifiers_and_policy_in_another_host_do_not_transfer_authority() {
        let (a, b, p) = configured();
        let (other, _, q) = configured();
        assert!(!other.allows(&b, Some(&p), "mock.read"));
        assert!(!a.allows(&b, Some(&q), "mock.read"));
        assert!(other.allows(&b, Some(&q), "mock.read"));
    }
    #[test]
    fn replacement_and_rollback_never_resurrect_old_permits() {
        let (mut a, b, p) = configured();
        a.replace(b.clone(), &["mock.write"], &["mock.write"]);
        let q = a.issue(&b).unwrap();
        assert!(!a.allows(&b, Some(&p), "mock.read"));
        assert!(a.allows(&b, Some(&q), "mock.write"));
        a.replace(
            b.clone(),
            &["mock.read", "mock.write"],
            &["mock.read", "mock.admin"],
        );
        let fresh = a.issue(&b).unwrap();
        assert!(!a.allows(&b, Some(&p), "mock.read"));
        assert!(!a.allows(&b, Some(&q), "mock.write"));
        assert!(a.allows(&b, Some(&fresh), "mock.read"));
    }
    #[test]
    fn deletion_recreation_and_retained_clones_stay_revoked() {
        let (mut a, b, p) = configured();
        let retained = p.clone();
        a.revoke(&b);
        assert!(a.issue(&b).is_none());
        assert!(!a.allows(&b, Some(&retained), "mock.read"));
        a.replace(b.clone(), &["mock.read"], &["mock.read"]);
        assert!(!a.allows(&b, Some(&retained), "mock.read"));
        let current = a.issue(&b).unwrap();
        assert!(a.allows(&b, Some(&current), "mock.read"));
    }
    #[test]
    fn mock_side_effect_rechecks_current_policy_at_use() {
        let (mut a, b, p) = configured();
        let mut sink = vec![];
        let perform = |host: &Authority, out: &mut Vec<&str>| {
            if host.allows(&b, Some(&p), "mock.read") {
                out.push("fake-canary");
            }
        };
        perform(&a, &mut sink);
        a.revoke(&b);
        perform(&a, &mut sink);
        assert_eq!(sink, ["fake-canary"]);
    }
    #[test]
    fn empty_and_unknown_requests_never_become_wildcards() {
        let mut a = Authority::default();
        let b = binding();
        for requested in [&[][..], &["mock.read"][..]] {
            a.replace(b.clone(), requested, &["*"]);
            let p = a.issue(&b).unwrap();
            assert!(!a.allows(&b, Some(&p), "mock.read"));
        }
    }
}
