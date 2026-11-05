//! Real Wasm permission/session/flow integration. Synthetic input and data only.
#[cfg(all(windows, target_arch = "x86_64"))]
mod probe {
    use diy_ime2_core::{
        execution_control::{
            CancellationToken, DecimalId, DeferredNode, ExecutionHost, ExecutionService,
            ResourcePolicy, RunKey, RunPhase, StartRequest,
        },
        graph::Graph,
        isolated_worker::{
            adapter::{ExecutionGate, WasmArtifact, WorkerPolicy},
            package::{
                Manifest, Package, PackageLimits, PackageVerifier, RuntimeKind, VerifiedPackage,
                module_sha256,
                permissions::{
                    ApprovalKind, PermissionBook, PermissionLifetime, PermissionPlan,
                    PermissionSession,
                },
            },
        },
    };
    use serde_json::{Value, json};
    use std::{
        collections::{BTreeMap, BTreeSet},
        fs,
        path::PathBuf,
        time::{Duration, Instant},
    };
    fn id(n: u64) -> DecimalId {
        DecimalId::new(n).unwrap()
    }
    fn wait(done: impl Fn() -> bool) -> Result<(), Box<dyn std::error::Error>> {
        let end = Instant::now() + Duration::from_secs(15);
        while !done() {
            if Instant::now() > end {
                return Err("permission Worker did not settle".into());
            }
            std::thread::sleep(Duration::from_millis(2));
        }
        Ok(())
    }
    fn graph(mode: &str) -> Graph {
        serde_json::from_value(json!({"version":1,"nodes":[{"id":"source","kind":"input.text","version":1},{"id":"worker","kind":"fixture.wasm","version":1,"config":{"mode":mode}}],"edges":[{"from":{"node":"source","port":"out"},"to":{"node":"worker","port":"in"}}]})).unwrap()
    }
    fn plan(
        graph: &Graph,
        package: &VerifiedPackage,
    ) -> Result<PermissionPlan, diy_ime2_core::node::CoreError> {
        let map = graph
            .nodes
            .iter()
            .filter(|n| n.kind == "fixture.wasm")
            .map(|n| (n.id.clone(), package.clone()))
            .collect::<BTreeMap<_, _>>();
        PermissionPlan::capture(graph, &map)
    }
    fn ids(plan: &PermissionPlan) -> BTreeSet<String> {
        plan.requests()
            .iter()
            .map(|r| r.subject().id().to_string())
            .collect()
    }
    fn all(
        pending: &diy_ime2_core::isolated_worker::package::permissions::PendingPermissionSession,
    ) -> Vec<String> {
        pending
            .missing()
            .iter()
            .map(|r| r.subject().id().into())
            .collect()
    }
    fn start(session: &PermissionSession) -> Result<RunKey, diy_ime2_core::node::CoreError> {
        let request:StartRequest=serde_json::from_value(json!({"startKey":"1","expectedPlanVersion":"1","targets":[{"node":"worker","port":"out"}],"input":{"revision":1,"text":"kya","mode":"compose"}})).unwrap();
        Ok(session.controller().start_run(request)?.key)
    }
    fn normal(
        session: &PermissionSession,
        host: &ExecutionHost,
    ) -> Result<RunKey, Box<dyn std::error::Error>> {
        let key = start(session)?;
        wait(|| {
            session
                .controller()
                .get_run(&key)
                .is_ok_and(|s| s.state == RunPhase::Terminal)
        })?;
        let value = serde_json::to_value(session.controller().get_run(&key)?)?;
        let text = value["result"]["ports"]
            .as_array()
            .and_then(|ports| ports.iter().find(|p| p["endpoint"]["node"] == "worker"))
            .and_then(|p| p["value"]["packet"]["root"]["text"].as_str())
            .ok_or("permission result missing")?;
        assert_eq!(
            serde_json::from_str::<Value>(text)?["operation"]["inputs"]["in"]["packet"]["root"]["text"],
            "kya"
        );
        wait(|| host.usage().physical_jobs == 0)?;
        Ok(key)
    }
    pub fn run() -> Result<(), Box<dyn std::error::Error>> {
        let args: Vec<_> = std::env::args().collect();
        if args.len() != 4 {
            return Err("usage: permission_probe WORKER DLL FIXTURE_WASM".into());
        }
        let root =
            std::env::temp_dir().join(format!("diyime-permission-probe-{}", std::process::id()));
        fs::create_dir(&root)?;
        let data = root.join("permissions");
        fs::create_dir(&data)?;
        let workspace = root.join("workers");
        fs::create_dir(&workspace)?;
        let module = fs::read(&args[3])?;
        let mut policy = WorkerPolicy::bounded_probe();
        policy.fuel_per_call = None;
        policy.walltime = None;
        policy.termination_grace = Duration::from_millis(5);
        let runtime = WasmArtifact::capture(
            &workspace,
            &PathBuf::from(&args[1]),
            &PathBuf::from(&args[2]),
            module.clone(),
            policy.clone(),
        )?;
        let definition = runtime
            .prepare(
                "fixture.wasm",
                1,
                vec![],
                ExecutionGate::default(),
                CancellationToken::default(),
            )?
            .definition();
        let manifest = Manifest {
            package_id: "test.permission".into(),
            version: "1.0.0".parse()?,
            display_name: "Permission fixture".into(),
            runtime: RuntimeKind::Wasm32,
            guest_api_version: 1,
            node: definition,
            module_sha256: module_sha256(&module)?,
            capabilities: vec![],
        };
        let limits = PackageLimits::default();
        let gate = ExecutionGate::default();
        let verifier = PackageVerifier::new(runtime.clone(), policy);
        let configs = vec![json!({"mode":"hold"}), json!({"mode":"noOutput"})];
        let package = verifier.verify_configs(
            Package::encode(manifest.clone(), &module, &limits)?,
            configs.clone(),
            gate.clone(),
            CancellationToken::default(),
        )?;
        let node = package.node();
        let echo = graph("echo");
        let baseline = ids(&plan(&echo, &package)?);
        assert_eq!(baseline.len(), 2);
        let mut reorder = echo.clone();
        reorder.nodes.reverse();
        reorder.edges.reverse();
        assert_eq!(ids(&plan(&reorder, &package)?), baseline);
        let mut unrelated = echo.clone();
        let mut extra = unrelated.nodes[0].clone();
        extra.id = "unrelated".into();
        unrelated.nodes.push(extra);
        assert_eq!(ids(&plan(&unrelated, &package)?), baseline);
        let mut fork = echo.clone();
        let mut second = fork.nodes[1].clone();
        second.id = "second".into();
        fork.nodes.push(second);
        let mut edge = fork.edges[0].clone();
        edge.to.node = "second".into();
        fork.edges.push(edge);
        let captured = plan(&fork, &package)?;
        assert_eq!(
            captured
                .requests()
                .iter()
                .filter(|r| matches!(r.kind, ApprovalKind::Execute))
                .count(),
            1
        );
        assert_eq!(captured.requests().len(), 3);
        let mut independent = fork.clone();
        let mut second_source = independent.nodes[0].clone();
        second_source.id = "second-source".into();
        independent.nodes.push(second_source);
        independent.edges[1].from.node = "second-source".into();
        let independent_ids = ids(&plan(&independent, &package)?);
        let mut new_source = independent.clone();
        new_source
            .nodes
            .iter_mut()
            .find(|n| n.id == "second-source")
            .unwrap()
            .id = "changed-second-source".into();
        new_source.edges[1].from.node = "changed-second-source".into();
        assert_eq!(
            independent_ids
                .intersection(&ids(&plan(&new_source, &package)?))
                .count(),
            2
        );
        let cancelled = CancellationToken::default();
        cancelled.cancel();
        assert!(
            PermissionPlan::capture_with_cancel(
                &echo,
                &[("worker".into(), package.clone())].into(),
                cancelled
            )
            .is_err()
        );
        let changed = graph("noOutput");
        let changed_ids = ids(&plan(&changed, &package)?);
        assert_eq!(baseline.intersection(&changed_ids).count(), 1);
        let mut rerouted = echo.clone();
        rerouted.nodes[0].id = "other-source".into();
        rerouted.edges[0].from.node = "other-source".into();
        assert_eq!(
            baseline
                .intersection(&ids(&plan(&rerouted, &package)?))
                .count(),
            1
        );
        let mut bad = echo.clone();
        bad.edges[0].to.port = "unknown".into();
        assert!(plan(&bad, &package).is_err());
        let mut metadata = manifest.clone();
        metadata.display_name = "renamed".into();
        metadata.version = "2.0.0".parse()?;
        let metadata = verifier.verify(
            Package::encode(metadata, &module, &limits)?,
            gate.clone(),
            CancellationToken::default(),
        )?;
        assert_eq!(ids(&plan(&echo, &metadata)?), baseline);
        let mut rebuilt = module.clone();
        rebuilt.extend_from_slice(&[0, 2, 1, b'x']);
        let mut rebuilt_manifest = manifest.clone();
        rebuilt_manifest.module_sha256 = module_sha256(&rebuilt)?;
        let rebuilt = verifier.verify(
            Package::encode(rebuilt_manifest, &rebuilt, &limits)?,
            gate.clone(),
            CancellationToken::default(),
        )?;
        assert!(baseline.is_disjoint(&ids(&plan(&echo, &rebuilt)?)));
        let mixed = [
            ("worker".into(), package.clone()),
            ("second".into(), rebuilt.clone()),
        ]
        .into();
        assert!(PermissionPlan::capture(&fork, &mixed).is_err());
        let combined = [
            ("worker".into(), metadata.clone()),
            ("second".into(), package.clone()),
        ]
        .into();
        let mut combined_graph = fork.clone();
        combined_graph
            .nodes
            .iter_mut()
            .find(|n| n.id == "second")
            .unwrap()
            .config = Some(json!({"mode":"hold"}));
        assert!(PermissionPlan::capture(&combined_graph, &combined).is_ok());
        let mut capability = manifest;
        capability
            .capabilities
            .push(diy_ime2_core::isolated_worker::package::CapabilityRequest {
                id: "network.request".into(),
                binding: "selected".into(),
            });
        assert!(
            verifier
                .verify(
                    Package::encode(capability, &module, &limits)?,
                    gate.clone(),
                    CancellationToken::default()
                )
                .is_err()
        );
        let mut resource = ResourcePolicy::from_value(
            serde_json::from_str::<Value>(include_str!(
                "../tests/contracts/execution_control.json"
            ))?["resourceProfiles"]["bounded"]
                .clone(),
        )?;
        resource.max_physical_jobs = 1;
        resource.max_queued_jobs = 0;
        let host =
            ExecutionHost::new(format!("permission-probe-{}", std::process::id()), resource)?;
        let service = ExecutionService::new(host.clone())?;
        let book = PermissionBook::open(&data, 1024 * 1024)?;
        let denied = book.open_session(&service, plan(&echo, &package)?, id(1))?;
        assert_eq!(denied.missing().len(), 2);
        assert!(denied.activate().is_err());
        assert_eq!(node.active_processes(), 0);
        let pending = book.open_session(&service, plan(&echo, &package)?, id(1))?;
        assert!(
            pending
                .approve(&["f".repeat(64)], PermissionLifetime::Saved)
                .is_err()
        );
        assert_eq!(pending.missing().len(), 2);
        assert!(
            pending
                .approve(
                    &[all(&pending)[0].clone(), "f".repeat(64)],
                    PermissionLifetime::Saved
                )
                .is_err()
        );
        assert_eq!(pending.missing().len(), 2);
        assert!(book.saved_ids().is_empty());
        pending.approve(&all(&pending), PermissionLifetime::ThisSession)?;
        let session = pending.activate()?;
        let key = normal(&session, &host)?;
        let retained = session.controller().clone();
        drop(session);
        assert!(retained.get_run(&key).is_err());
        drop(retained);
        assert!(book.saved_ids().is_empty());
        let pending = book.open_session(&service, plan(&echo, &package)?, id(1))?;
        assert_eq!(pending.missing().len(), 2);
        drop(pending);
        drop(book);
        let book = PermissionBook::open(&data, 1024 * 1024)?;
        let pending = book.open_session(&service, plan(&echo, &package)?, id(1))?;
        assert_eq!(pending.missing().len(), 2);
        pending.approve(&all(&pending), PermissionLifetime::Saved)?;
        let session = pending.activate()?;
        normal(&session, &host)?;
        drop(session);
        drop(book);
        let saved = fs::read_to_string(data.join("permissions.json"))?;
        for forbidden in [
            "kya", "source", "worker", "mode", "compose", "config", "Packet",
        ] {
            assert!(!saved.contains(forbidden));
        }
        let book = PermissionBook::open(&data, 1024 * 1024)?;
        let pending = book.open_session(&service, plan(&echo, &package)?, id(1))?;
        assert!(pending.missing().is_empty());
        let session = pending.activate()?;
        normal(&session, &host)?;
        drop(session);
        let pending = book.open_session(&service, plan(&changed, &package)?, id(1))?;
        assert_eq!(pending.missing().len(), 1);
        drop(pending);
        let pending = book.open_session(&service, plan(&echo, &rebuilt)?, id(1))?;
        assert_eq!(pending.missing().len(), 2);
        drop(pending);
        let pending = book.open_session(&service, plan(&echo, &package)?, id(1))?;
        book.set_protection(true, true);
        book.set_protection(false, true);
        assert!(pending.activate().is_err());
        book.set_protection(false, false);
        // Real running infinite guest: withdrawal denies retained results immediately,
        // while Core/Worker still own the physical slot until true process/IO end.
        let pending = book.open_session(&service, plan(&graph("hold"), &package)?, id(1))?;
        let flow = pending.missing()[0].subject().id().to_string();
        pending.approve(&all(&pending), PermissionLifetime::Saved)?;
        let session = pending.activate()?;
        let retained = session.controller().clone();
        let key = start(&session)?;
        wait(|| node.active_processes() > 0)?;
        book.revoke_id(&flow)?;
        assert!(retained.get_run(&key).is_err());
        assert!(start(&session).is_err());
        wait(|| node.active_processes() == 0 && host.usage().physical_jobs == 0)?;
        drop(session);
        drop(retained);
        let pending = book.open_session(&service, plan(&echo, &package)?, id(1))?;
        assert!(pending.missing().is_empty());
        let session = pending.activate()?;
        normal(&session, &host)?;
        let retained = session.controller().clone();
        book.set_protection(true, false);
        book.set_protection(false, false);
        assert!(retained.control_state().is_err());
        drop(retained);
        drop(session);
        println!(
            "Permissions: real execute/input-flow consent, Session-only expiry, saved reopen, changed code/config/route refusal, metadata/order stability, deduplicated execute, protection, live withdrawal/true exit and recovery passed; diagnostics {}",
            root.display()
        );
        Ok(())
    }
}
fn main() {
    #[cfg(all(windows, target_arch = "x86_64"))]
    if let Err(error) = probe::run() {
        eprintln!("{error}");
        std::process::exit(1);
    }
    #[cfg(not(all(windows, target_arch = "x86_64")))]
    {
        eprintln!("Windows x64 required");
        std::process::exit(1);
    }
}
