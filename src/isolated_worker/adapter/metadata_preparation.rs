//! Private preparation experiment: immutable compiler output only. Each metadata
//! request still owns a fresh AppContainer, Store/Instance and physical cleanup.
use super::*;

fn enabled(name: &str) -> bool {
    std::env::var_os(name).as_deref() == Some(std::ffi::OsStr::new("1"))
}

fn metadata_request(request: &Value) -> bool {
    matches!(
        request["operation"]["method"].as_str(),
        Some("describe" | "validate")
    )
}

impl WasmArtifact {
    pub(super) fn call_metadata(
        &self,
        request: &Value,
        cancel: CancellationToken,
    ) -> Result<Vec<u8>, CoreError> {
        self.call_metadata_with_options(
            request,
            cancel,
            enabled("DIY_EXPERIMENT_PREPARATION_CODE_CACHE"),
            enabled("DIY_EXPERIMENT_COMPILED_JIT"),
        )
    }

    fn call_metadata_with_options(
        &self,
        request: &Value,
        cancel: CancellationToken,
        experiment: bool,
        compiler_enabled: bool,
    ) -> Result<Vec<u8>, CoreError> {
        if !metadata_request(request) {
            return Err(fail("worker-request", "metadata operation required"));
        }
        // The factory-wide legacy cell is also shared with other modules. Only
        // the actual fixed official source AND policy retain that legacy path.
        let legacy = self.0.compiled.is_some()
            && self.0.identity.rsplit(':').next()
                == Some(super::super::runtime::OFFICIAL_WASM_HASH)
            && self.0.policy.fuel_per_call == Some(50_000_000)
            && self.0.policy.wasm_memory_bytes == Some(128 * 1024 * 1024);
        if !experiment || legacy {
            return self.call(request, cancel);
        }
        if cancel.is_cancelled() {
            return Err(fail("worker-cancelled", "Worker was not started"));
        }
        let bytes = serde_json::to_vec(request)
            .map_err(|_| fail("worker-request", "request serialization failed"))?;
        if bytes.len() > 8 * 1024 * 1024 {
            return Err(fail("worker-request", "request exceeds guest wire limit"));
        }
        // No raw-JIT fallback: unavailable/busy/cancelled producers cannot hide
        // a cleanup failure or cause a second guest execution attempt.
        let image = self.prepared_jit_image_if_enabled(&cancel, None, compiler_enabled);
        if !matches!(image, WorkerImage::PreparedCompiled(..)) {
            return Err(fail(
                "worker-preparation",
                "metadata compiled code unavailable",
            ));
        }
        self.0.requests.fetch_add(1, Ordering::AcqRel);
        self.call_worker(
            WorkerRequest::Jit {
                request: Box::new(move || {
                    Ok(JitDelivery {
                        bytes,
                        check: None,
                        run_cancel: None,
                        detailed: false,
                        late_code: None,
                    })
                }),
                trace: None,
            },
            cancel,
            None,
            0,
            image,
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn metadata_dispatch_never_accepts_execution_or_missing_methods() {
        for method in ["describe", "validate"] {
            assert!(metadata_request(&wire::request(
                "test",
                1,
                json!({"method":method})
            )));
        }
        for operation in [
            json!({}),
            json!({"method":null}),
            json!({"method":"execute"}),
            json!({"method":"executePrepared"}),
        ] {
            assert!(!metadata_request(&wire::request("test", 1, operation)));
        }
    }

    #[test]
    #[ignore = "requires current Worker, pinned DLL, real fixture Wasm and AppContainer"]
    fn real_metadata_code_cache_reuses_only_code_and_refuses_unavailable_producers() {
        assert!(enabled("DIY_EXPERIMENT_PREPARATION_CODE_CACHE"));
        assert!(enabled("DIY_EXPERIMENT_COMPILED_JIT"));
        let asset = |name| PathBuf::from(std::env::var_os(name).expect("explicit metadata asset"));
        let worker = asset("DIY_RANGE_WORKER_BINARY");
        let dll = asset("DIY_RANGE_RUNTIME_DLL");
        let wasm = std::fs::read(asset("DIY_METADATA_FIXTURE_WASM")).unwrap();
        let workspace = std::env::temp_dir().join(format!(
            "metadata-code-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        std::fs::create_dir(&workspace).unwrap();
        // Cache owns only its unpredictable child directories, not these
        // caller-owned staging containers (Cache::new_named contract).
        let staging = workspace.join("target/appcontainer-probes");
        std::fs::create_dir_all(&staging).unwrap();
        let artifact = WasmArtifact::capture(
            &workspace,
            &worker,
            &dll,
            wasm.clone(),
            WorkerPolicy {
                fuel_per_call: Some(50_000_000),
                wasm_memory_bytes: Some(128 * 1024 * 1024),
                ..WorkerPolicy::bounded_probe()
            },
        )
        .unwrap()
        .enable_official_compilation();
        let describe = wire::request("fixture.wasm", 1, json!({"method":"describe"}));
        let validate = wire::request(
            "fixture.wasm",
            1,
            json!({"method":"validate","config":{"mode":"echo"}}),
        );
        // Even a shared legacy cell cannot select fixed-official handling for
        // this third-party source. Disabled compiler must refuse before guest.
        assert_eq!(
            artifact
                .call_metadata_with_options(&describe, CancellationToken::default(), true, false)
                .unwrap_err()
                .code,
            "worker-preparation"
        );
        assert_eq!(artifact.request_count(), 0);
        assert_eq!(artifact.timings().settled_calls, 0);
        for round in 0..2 {
            let definition = wire::reply(
                &artifact
                    .call_metadata_with_options(&describe, CancellationToken::default(), true, true)
                    .unwrap(),
            )
            .unwrap();
            assert_eq!(definition["kind"], "fixture.wasm");
            assert_eq!(artifact.active_processes(), 0);
            assert_eq!(
                wire::reply(
                    &artifact
                        .call_metadata_with_options(
                            &validate,
                            CancellationToken::default(),
                            true,
                            true
                        )
                        .unwrap()
                )
                .unwrap(),
                json!({"valid":true})
            );
            assert_eq!(artifact.active_processes(), 0);
            assert_eq!(artifact.request_count(), 2 * (round + 1));
            // One compiler plus each original metadata process; no process is
            // retained and the second round must not start another compiler.
            assert_eq!(artifact.timings().settled_calls, 1 + 2 * (round + 1));
            assert!(matches!(
                *artifact.0.jit_compiled.lock().unwrap(),
                CompiledState::Ready(_)
            ));
        }
        let invalid = wire::request(
            "fixture.wasm",
            1,
            json!({"method":"validate","config":{"mode":"echo","extra":true}}),
        );
        let reply = artifact
            .call_metadata_with_options(&invalid, CancellationToken::default(), true, true)
            .unwrap();
        assert_eq!(wire::reply(&reply).unwrap_err().code, "invalid-config");
        assert_eq!(artifact.timings().settled_calls, 6);
        let cancelled = CancellationToken::default();
        cancelled.cancel();
        assert_eq!(
            artifact
                .call_metadata_with_options(&describe, cancelled, true, true)
                .unwrap_err()
                .code,
            "worker-cancelled"
        );
        assert_eq!(artifact.request_count(), 5);
        let node = artifact
            .prepare(
                "fixture.wasm",
                1,
                vec![
                    json!({"mode":"echo"}),
                    json!({"mode":"forward"}),
                    json!({"mode":"forward"}),
                ],
                ExecutionGate::default(),
                CancellationToken::default(),
            )
            .unwrap();
        assert_eq!(
            *node.configs,
            vec![json!({"mode":"echo"}), json!({"mode":"forward"})]
        );
        assert_eq!(artifact.request_count(), 8);
        assert_eq!(artifact.timings().settled_calls, 9);
        assert_eq!(artifact.active_processes(), 0);
        drop(node);
        let other = artifact
            .with_module(wasm, artifact.0.policy.clone())
            .unwrap();
        assert!(matches!(
            *other.0.jit_compiled.lock().unwrap(),
            CompiledState::Pending
        ));
        *other.0.jit_compiled.lock().unwrap() = CompiledState::Unavailable;
        assert_eq!(
            other
                .call_metadata_with_options(&describe, CancellationToken::default(), true, true)
                .unwrap_err()
                .code,
            "worker-preparation"
        );
        assert_eq!(other.request_count(), 0);
        assert_eq!(other.timings().settled_calls, 0);
        // OFF remains the raw-JIT path, even when the experiment cache is
        // unavailable. This is an explicit caller mode, never a retry.
        assert!(
            wire::reply(
                &other
                    .call_metadata_with_options(
                        &describe,
                        CancellationToken::default(),
                        false,
                        true
                    )
                    .unwrap()
            )
            .is_ok()
        );
        assert_eq!(other.timings().settled_calls, 1);
        assert_eq!(other.active_processes(), 0);
        let broken = artifact
            .with_module(b"\0asm\x01\0\0\0\xff".to_vec(), artifact.0.policy.clone())
            .unwrap();
        for _ in 0..2 {
            assert_eq!(
                broken
                    .call_metadata_with_options(&describe, CancellationToken::default(), true, true)
                    .unwrap_err()
                    .code,
                "worker-preparation"
            );
            assert_eq!(broken.request_count(), 0);
            assert_eq!(broken.timings().settled_calls, 1);
            assert_eq!(broken.active_processes(), 0);
        }
        assert!(matches!(
            *broken.0.jit_compiled.lock().unwrap(),
            CompiledState::Unavailable
        ));
        drop(broken);
        let official_bytes = std::fs::read(asset("DIY_METADATA_OFFICIAL_WASM")).unwrap();
        assert_eq!(
            sha256(&official_bytes).unwrap(),
            super::super::super::runtime::OFFICIAL_WASM_HASH
        );
        let official = artifact
            .with_module(official_bytes, artifact.0.policy.clone())
            .unwrap();
        let official_describe = wire::request("demo.identity", 1, json!({"method":"describe"}));
        for round in 0..2 {
            let reply = official
                .call_metadata_with_options(
                    &official_describe,
                    CancellationToken::default(),
                    true,
                    true,
                )
                .unwrap();
            assert_eq!(wire::reply(&reply).unwrap()["kind"], "demo.identity");
            assert_eq!(official.active_processes(), 0);
            assert_eq!(official.timings().settled_calls, round + 2);
            assert!(matches!(
                *official.0.compiled.as_ref().unwrap().lock().unwrap(),
                CompiledState::Ready(_)
            ));
            assert!(matches!(
                *official.0.jit_compiled.lock().unwrap(),
                CompiledState::Pending
            ));
        }
        drop(official);
        let budget = artifact.0.code_budget.clone();
        assert!(budget.used.load(Ordering::Acquire) > 0);
        drop(other);
        drop(artifact);
        assert_eq!(budget.used.load(Ordering::Acquire), 0);
        assert_eq!(std::fs::read_dir(&staging).unwrap().count(), 0);
        std::fs::remove_dir(staging).unwrap();
        std::fs::remove_dir(workspace.join("target")).unwrap();
        assert_eq!(std::fs::read_dir(&workspace).unwrap().count(), 0);
        std::fs::remove_dir(workspace).unwrap();
    }
}
