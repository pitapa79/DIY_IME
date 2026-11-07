use super::*;
use crate::isolated_worker::package::{CapabilityRequest, format::digest};
use serde_json::json;

fn graph() -> Graph {
    Graph::from_json(include_str!(
        "../../../../../examples/graphs/official-dictionary.json"
    ))
    .unwrap()
}
fn directory() -> &'static Path {
    Path::new(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/distribution/official"
    ))
}
fn bundle() -> Bundle {
    Bundle::capture(
        directory(),
        &PackageLimits::default(),
        &CancellationToken::default(),
    )
    .unwrap()
}
#[test]
fn exact_shape_is_independent_of_ids_and_json_order_but_excludes_extensions() {
    let mut original = graph();
    assert_eq!(shape(&original).unwrap()[2], "dictionary");
    let names: BTreeMap<_, _> = original
        .nodes
        .iter()
        .enumerate()
        .map(|(n, node)| (node.id.clone(), format!("renamed-{n}")))
        .collect();
    for node in &mut original.nodes {
        node.id = names[&node.id].clone();
    }
    for edge in &mut original.edges {
        edge.from.node = names[&edge.from.node].clone();
        edge.to.node = names[&edge.to.node].clone();
    }
    original.nodes.reverse();
    original.edges.reverse();
    assert_eq!(shape(&original).unwrap()[2], names["dictionary"]);
    for case in 0..7 {
        let mut g = graph();
        match case {
            0 => g.nodes[1].version = 2,
            1 => g.edges[0].from.port = "pending".into(),
            2 => g.edges[2].to.node = "result".into(),
            3 => g.edges.push(g.edges[0].clone()),
            4 => g.nodes.push(g.nodes[0].clone()),
            5 => g.nodes[2].kind = "thirdparty.dictionary".into(),
            _ => g.nodes[2].id = g.nodes[1].id.clone(),
        }
        assert!(shape(&g).is_none(), "extension/substitution {case}");
    }
}
#[test]
fn actual_pinned_frames_exclude_same_names_altered_code_metadata_and_capabilities() {
    let snapshot = bundle();
    let module = std::fs::read(directory().join("official.wasm")).unwrap();
    for kind in &KINDS[1..] {
        let expected = snapshot
            .packages
            .iter()
            .find(|p| p.manifest().node.kind == *kind)
            .unwrap();
        let dictionary = *kind == KINDS[2];
        assert!(same_code(expected, expected, dictionary));
        for case in 0..6 {
            let mut manifest = expected.manifest().clone();
            let mut code = module.clone();
            match case {
                0 => manifest.package_id = "thirdparty.same-kind".into(),
                1 => manifest.node.default_config = json!({"altered":true}),
                2 => {
                    code.extend_from_slice(&[0, 4, 3, b'f', b'o', b'o']);
                    manifest.module_sha256 = digest(&code).unwrap();
                }
                3 => manifest.capabilities.push(CapabilityRequest {
                    id: "network.request".into(),
                    binding: "endpoint".into(),
                }),
                4 => manifest.capabilities.push(CapabilityRequest {
                    id: "secret.use".into(),
                    binding: "endpoint".into(),
                }),
                _ => manifest.capabilities.push(CapabilityRequest {
                    id: "file.write".into(),
                    binding: "file".into(),
                }),
            }
            let changed = Package::encode(manifest, &code, &PackageLimits::default()).unwrap();
            assert!(
                !same_code(&changed, expected, dictionary),
                "{kind} substitution {case}"
            );
        }
    }
}

#[test]
#[ignore = "requires real native Worker and fixed DLL; verifies actual official Wasm without dictionary permission"]
fn real_official_fixed_review() {
    use crate::{
        execution_control::{DecimalId, ExecutionHost, ExecutionService, ResourcePolicy},
        isolated_worker::{
            adapter::{
                DictionaryBindingConfig, DictionaryBindings, ExecutionGate, WasmArtifact,
                WorkerPolicy,
            },
            package::{PackageVerifier, permissions::PermissionBook},
        },
    };
    use std::{fs, os::windows::fs::OpenOptionsExt, path::PathBuf};
    let required =
        |name| PathBuf::from(std::env::var_os(name).expect("explicit native review asset"));
    let root = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("target")
        .join(format!("official-static-review-{}", std::process::id()));
    fs::create_dir(&root).unwrap();
    struct Cleanup(PathBuf);
    impl Drop for Cleanup {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }
    let _cleanup = Cleanup(root.clone());
    // An exclusive lock makes even an accidental Host body read fail. This
    // deliberately invalid TSV must remain unparsed during permission review.
    fs::write(root.join("dictionary.tsv"), b"not valid TSV").unwrap();
    let dictionary_lock = fs::OpenOptions::new()
        .read(true)
        .share_mode(0)
        .open(root.join("dictionary.tsv"))
        .unwrap();
    let bindings = DictionaryBindings::open(BTreeMap::from([(
        "dictionary".into(),
        DictionaryBindingConfig {
            root: root.clone(),
            reference_pointer: "/path".into(),
            max_file_bytes: 1024,
            max_total_bytes: 1024,
            max_files: 1,
            budget_ms: 1000,
        },
    )]))
    .unwrap();
    let module = fs::read(directory().join("official.wasm")).unwrap();
    let policy = WorkerPolicy::bounded_probe();
    let runtime = WasmArtifact::capture(
        &root,
        &required("DIY_RANGE_WORKER_BINARY"),
        &required("DIY_RANGE_RUNTIME_DLL"),
        module.clone(),
        policy.clone(),
    )
    .unwrap();
    let verifier = PackageVerifier::new(runtime, policy).with_dictionary_bindings(bindings);
    let gate = ExecutionGate::default();
    let graph = graph();
    let snapshot = bundle();
    let mut external = BTreeMap::new();
    for node in &graph.nodes {
        if node.kind == KINDS[0] {
            continue;
        }
        let package = snapshot
            .packages
            .iter()
            .find(|p| p.manifest().node.kind == node.kind)
            .unwrap()
            .clone();
        let configs = if node.kind == KINDS[3] {
            vec![json!({"unscored":1.0})]
        } else {
            vec![]
        };
        external.insert(
            node.id.clone(),
            verifier
                .verify_configs(package, configs, gate.clone(), CancellationToken::default())
                .unwrap(),
        );
    }
    let review = OfficialGraphReview::capture(
        &graph,
        &external,
        directory(),
        &PackageLimits::default(),
        CancellationToken::default(),
    )
    .unwrap();
    let proof = review.pipeline().expect("exact official static graph");
    assert!(proof.matches_plan(review.permission_plan()));
    assert_eq!(proof.input_id(), "input");
    assert_eq!(
        proof.stages()[1].config(),
        &json!({"path":"dictionary.tsv"})
    );
    assert_eq!(
        proof.stages()[1].package().package().manifest().node.kind,
        KINDS[2]
    );
    let other = PermissionPlan::capture(&graph, &external).unwrap();
    assert!(
        !proof.matches_plan(&other),
        "equal graph is not the same compiled plan"
    );

    let mut changed_config = graph.clone();
    changed_config
        .nodes
        .iter_mut()
        .find(|n| n.kind == KINDS[3])
        .unwrap()
        .config = Some(json!({"unscored":1.0}));
    let changed = OfficialGraphReview::capture(
        &changed_config,
        &external,
        directory(),
        &PackageLimits::default(),
        CancellationToken::default(),
    )
    .unwrap();
    assert_eq!(
        changed.pipeline().unwrap().stages()[2].config(),
        &json!({"unscored":1.0})
    );
    assert_eq!(proof.stages()[2].config(), &json!({"unscored":0.0}));
    assert!(!proof.matches_plan(changed.permission_plan()));

    let mut renamed = graph.clone();
    for node in &mut renamed.nodes {
        node.id = format!("new-{}", node.id);
    }
    for edge in &mut renamed.edges {
        edge.from.node = format!("new-{}", edge.from.node);
        edge.to.node = format!("new-{}", edge.to.node);
    }
    renamed.nodes.reverse();
    renamed.edges.reverse();
    let renamed_external = external
        .iter()
        .map(|(id, p)| (format!("new-{id}"), p.clone()))
        .collect();
    let renamed = OfficialGraphReview::capture(
        &renamed,
        &renamed_external,
        directory(),
        &PackageLimits::default(),
        CancellationToken::default(),
    )
    .unwrap();
    assert_eq!(
        renamed.pipeline().unwrap().stages()[1].id(),
        "new-dictionary"
    );
    let mut extended = graph.clone();
    let mut extra = extended.nodes[0].clone();
    extra.id = "extra-source".into();
    extended.nodes.push(extra);
    let extended = OfficialGraphReview::capture(
        &extended,
        &external,
        Path::new("absent-bundle-is-not-needed"),
        &PackageLimits::default(),
        CancellationToken::default(),
    )
    .unwrap();
    assert!(extended.pipeline().is_none());
    assert!(!extended.permission_plan().requests().is_empty());

    let original = snapshot
        .packages
        .iter()
        .find(|p| p.manifest().node.kind == KINDS[1])
        .unwrap();
    let mut altered_module = module;
    altered_module.extend_from_slice(&[0, 4, 3, b'f', b'o', b'o']);
    let mut manifest = original.manifest().clone();
    manifest.module_sha256 = digest(&altered_module).unwrap();
    let substituted =
        Package::encode(manifest, &altered_module, &PackageLimits::default()).unwrap();
    let substituted = verifier
        .verify(substituted, gate, CancellationToken::default())
        .unwrap();
    let mut alternate = external.clone();
    alternate.insert("hiragana".into(), substituted);
    let alternate = OfficialGraphReview::capture(
        &graph,
        &alternate,
        directory(),
        &PackageLimits::default(),
        CancellationToken::default(),
    )
    .unwrap();
    assert!(
        alternate.pipeline().is_none(),
        "actual verified same-kind Wasm with another hash must fall back"
    );
    let cancelled = CancellationToken::default();
    cancelled.cancel();
    assert!(
        OfficialGraphReview::capture(
            &graph,
            &external,
            directory(),
            &PackageLimits::default(),
            cancelled
        )
        .is_err()
    );

    let policy = ResourcePolicy::from_value(
        serde_json::from_str::<Value>(include_str!(
            "../../../../../tests/contracts/execution_control.json"
        ))
        .unwrap()["resourceProfiles"]["bounded"]
            .clone(),
    )
    .unwrap();
    let host = ExecutionHost::new("official-static-no-grant", policy).unwrap();
    let service = ExecutionService::new(host.clone()).unwrap();
    let book_root = root.join("permissions");
    fs::create_dir(&book_root).unwrap();
    let book = PermissionBook::open(&book_root, 1024 * 1024).unwrap();
    let (plan, proof) = review.into_parts();
    assert!(proof.unwrap().matches_plan(&plan));
    let pending = book
        .open_session(&service, plan, DecimalId::new(1).unwrap())
        .unwrap();
    assert!(!pending.missing().is_empty());
    assert!(pending.activate().is_err());
    assert!(book.saved_ids().is_empty());
    assert_eq!(host.usage().physical_jobs, 0);
    drop(book);
    drop(dictionary_lock);
    eprintln!(
        "actual official static review: fixed/renamed/config snapshots, equal-plan refusal, extended fallback, verified changed-module fallback, no dictionary body read and no implicit approval"
    );
}
