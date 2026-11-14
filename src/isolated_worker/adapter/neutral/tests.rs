use super::*;

#[test]
#[ignore = "requires production Worker, pinned DLL and actual AppContainer"]
fn real_neutral_stock_selects_one_source_after_ready_and_preserves_factory_and_cleanup() {
    assert!(enabled());
    let contract: Value = serde_json::from_str(include_str!(
        "../../../../tests/contracts/execution_control.json"
    ))
    .unwrap();
    let mut resources = crate::execution_control::ResourcePolicy::from_value(
        contract["resourceProfiles"]["bounded"].clone(),
    )
    .unwrap();
    resources.max_physical_jobs = 2;
    let host = ExecutionHost::new("neutral-single-use", resources).unwrap();
    let worker = std::env::var_os("DIY_RANGE_WORKER_BINARY").unwrap();
    let dll = std::env::var_os("DIY_RANGE_RUNTIME_DLL").unwrap();
    let fixture = |name| {
        crate::isolated_worker::runtime::compiled_jit_probe::worker_fixture(Path::new(&dll), name)
    };
    let policy = WorkerPolicy {
        fuel_per_call: Some(10_000),
        wasm_memory_bytes: Some(1024 * 1024),
        ..WorkerPolicy::bounded_probe()
    };
    let factory = WasmArtifact::capture(
        Path::new(env!("CARGO_MANIFEST_DIR")),
        Path::new(&worker),
        Path::new(&dll),
        fixture("trap"),
        policy.clone(),
    )
    .unwrap();
    let first = factory
        .with_module(fixture("state"), policy.clone())
        .unwrap();
    let second = factory
        .with_module(fixture("different"), policy.clone())
        .unwrap();
    let standard = factory
        .with_module(fixture("standard-state"), policy.clone())
        .unwrap();
    let fuel = factory
        .with_module(fixture("fuel"), policy.clone())
        .unwrap();
    let memory = factory
        .with_module(fixture("memory"), policy.clone())
        .unwrap();
    let changed = factory
        .with_module(
            fixture("state"),
            WorkerPolicy {
                fuel_per_call: Some(11_000),
                ..policy.clone()
            },
        )
        .unwrap();
    let foreign = WasmArtifact::capture(
        Path::new(env!("CARGO_MANIFEST_DIR")),
        Path::new(&worker),
        Path::new(&dll),
        fixture("state"),
        policy,
    )
    .unwrap();
    {
        let _compiler = host
            .try_reserve(ResourceUsage {
                physical_jobs: 1,
                ..ResourceUsage::default()
            })
            .unwrap();
        for source in [
            &first, &second, &standard, &fuel, &memory, &changed, &foreign,
        ] {
            assert!(matches!(
                source.prepared_jit_image(&CancellationToken::default()),
                WorkerImage::PreparedCompiled(..)
            ));
            assert_eq!(source.active_processes(), 0);
        }
    }
    let ready = || {
        let mut owner = PreparedJitOwner::start_scoped_using(
            &host,
            factory.clone(),
            &CancellationToken::default(),
            true,
        )
        .unwrap();
        assert!(
            owner.observe_ready(Duration::from_secs(5)),
            "neutral stock did not become ready"
        );
        assert_eq!(factory.active_processes(), 1);
        assert_eq!(host.usage().physical_jobs, 1);
        assert!(
            matches!(
                *factory.0.jit_compiled.lock().unwrap(),
                CompiledState::Pending
            ),
            "the factory's Node was compiled before late selection"
        );
        owner
    };
    let delivery = |code, check| JitDelivery {
        bytes: b"{}".to_vec(),
        check,
        run_cancel: None,
        detailed: true,
        late_code: Some(code),
    };
    // Source selection is after the distinct runtime-ready receipt. Each owner
    // holds no Module/Store at that point and is consumed by exactly one input.
    for (source, value) in [
        (&second, 2),
        (&first, 1),
        (&second, 2),
        (&standard, 1),
        (&memory, 1),
    ] {
        let owner = ready();
        let baseline = factory.0.code_budget.used.load(Ordering::Acquire);
        let code = source.late_jit_code(&CancellationToken::default()).unwrap();
        assert!(code.matches(&factory));
        let weak = Arc::downgrade(&code);
        let reply = owner.dispatch(delivery(code, None), None).unwrap();
        assert_eq!(wire::reply(&reply).unwrap()["value"], value);
        assert!(
            weak.upgrade().is_none(),
            "late code pin survived settled dispatch"
        );
        assert_eq!(factory.0.code_budget.used.load(Ordering::Acquire), baseline);
        assert_eq!(factory.active_processes(), 0);
        assert_eq!(host.usage().physical_jobs, 0);
    }
    for source in [&changed, &foreign] {
        let owner = ready();
        let code = source.late_jit_code(&CancellationToken::default()).unwrap();
        assert!(!code.matches(&factory));
        assert!(matches!(
            owner.try_dispatch(delivery(code, None), None),
            PreparedDispatch::Undelivered(_)
        ));
        assert_eq!(factory.active_processes(), 0);
        assert_eq!(host.usage().physical_jobs, 0);
    }
    let owner = ready();
    let code = first.late_jit_code(&CancellationToken::default()).unwrap();
    let weak = Arc::downgrade(&code);
    assert!(
        owner
            .dispatch(
                delivery(
                    code,
                    Some(Box::new(|| Err("withdrawn before delivery".into())))
                ),
                None
            )
            .is_err()
    );
    assert!(weak.upgrade().is_none());
    assert_eq!(factory.active_processes(), 0);
    assert_eq!(host.usage().physical_jobs, 0);

    let owner = ready();
    let code = fuel.late_jit_code(&CancellationToken::default()).unwrap();
    let before = factory.timings().settled_calls;
    assert!(owner.dispatch(delivery(code, None), None).is_err());
    assert_eq!(
        factory.timings().settled_calls,
        before + 1,
        "fuel failure retried execution"
    );
    assert_eq!(factory.active_processes(), 0);
    assert_eq!(host.usage().physical_jobs, 0);

    let scope = CancellationToken::default();
    let mut owner =
        PreparedJitOwner::start_scoped_using(&host, factory.clone(), &scope, true).unwrap();
    assert!(owner.observe_ready(Duration::from_secs(5)));
    scope.cancel();
    drop(owner); // original process/IO/profile must settle before return
    assert_eq!(factory.active_processes(), 0);
    assert_eq!(host.usage().physical_jobs, 0);

    let owner = ready();
    let code = first.late_jit_code(&CancellationToken::default()).unwrap();
    assert_eq!(
        wire::reply(&owner.dispatch(delivery(code, None), None).unwrap()).unwrap()["value"],
        1
    );
    assert_eq!(factory.active_processes(), 0);
    assert_eq!(host.usage().physical_jobs, 0);
}
