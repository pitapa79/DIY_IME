#![cfg(windows)]
use diy_ime2_core::{
    node::PrepareResources,
    resources::{ResourceLimits, ResourceRoot},
};
use std::{
    fs::{self, OpenOptions},
    path::{Path, PathBuf},
    sync::{
        Arc,
        atomic::{AtomicUsize, Ordering},
    },
    time::Duration,
};
struct Scratch(PathBuf);
impl Scratch {
    fn new() -> Self {
        static NEXT: AtomicUsize = AtomicUsize::new(0);
        let path = std::env::temp_dir()
            .join("diy-ime2-resource-tests")
            .join(format!(
                "{}-{}",
                std::process::id(),
                NEXT.fetch_add(1, Ordering::SeqCst)
            ));
        fs::create_dir_all(&path).unwrap();
        Self(path)
    }
    fn path(&self, name: &str) -> PathBuf {
        self.0.join(name)
    }
    fn write(&self, name: &str, bytes: &[u8]) {
        fs::write(self.path(name), bytes).unwrap();
    }
    fn root(&self) -> ResourceRoot {
        ResourceRoot::open(
            &self.0,
            ResourceLimits {
                budget: Duration::from_secs(10),
                ..ResourceLimits::default()
            },
        )
        .unwrap()
    }
}
impl Drop for Scratch {
    fn drop(&mut self) {
        let allowed = std::env::temp_dir().join("diy-ime2-resource-tests");
        assert!(self.0.is_absolute() && self.0.starts_with(&allowed) && self.0 != allowed);
        let _ = fs::remove_dir_all(&self.0);
    }
}
#[test]
fn explicit_local_root_reads_unicode_nested_and_empty_files() {
    let d = Scratch::new();
    fs::create_dir(d.path("sub")).unwrap();
    d.write("sub/辞書.tsv", "かな\t仮名".as_bytes());
    d.write("empty", b"");
    let root = d.root();
    let mut batch = root.batch();
    assert_eq!(
        &*batch.read("sub/辞書.tsv", 100).unwrap(),
        "かな\t仮名".as_bytes()
    );
    assert!(batch.read("empty", 0).unwrap().is_empty());
}
#[test]
fn invalid_paths_are_rejected_before_file_content_is_read() {
    let d = Scratch::new();
    let root = d.root();
    let mut b = root.batch();
    for name in [
        "",
        "..",
        "../secret",
        "a/../secret",
        "./a",
        "/absolute",
        "C:/secret",
        "C:secret",
        r"\server\share",
        r"a\b",
        "https://x/a",
        "a:stream",
        "a//b",
        "a/",
        "a.",
        "a ",
        "CON",
        "NUL.tsv",
        "COM1",
        "LPT².txt",
        "CONIN$",
        "a*",
        "a?",
        "a|b",
        "a\0b",
    ] {
        assert_eq!(
            b.read(name, 100).unwrap_err().code,
            "resource-path",
            "{name:?}"
        );
    }
    assert_eq!(b.read("absent", 100).unwrap_err().code, "resource-read");
}
#[test]
fn file_total_count_and_cooperative_deadline_limits_fail_without_truncation() {
    let d = Scratch::new();
    d.write("one", b"1234");
    d.write("two", b"5678");
    let root = d.root();
    assert_eq!(
        root.batch().read("one", 3).unwrap_err().code,
        "resource-limit"
    );
    assert_eq!(&*root.batch().read("one", 4).unwrap(), b"1234");
    let root = ResourceRoot::open(
        &d.0,
        ResourceLimits {
            max_file_bytes: 3,
            ..ResourceLimits::default()
        },
    )
    .unwrap();
    assert_eq!(
        root.batch().read("one", 10).unwrap_err().code,
        "resource-limit"
    );
    let root = ResourceRoot::open(
        &d.0,
        ResourceLimits {
            max_total_bytes: 7,
            budget: Duration::from_secs(10),
            ..ResourceLimits::default()
        },
    )
    .unwrap();
    let mut batch = root.batch();
    batch.read("one", 10).unwrap();
    assert_eq!(batch.read("two", 10).unwrap_err().code, "resource-limit");
    assert_eq!(&*batch.read("one", 4).unwrap(), b"1234");
    drop(batch);
    let root = ResourceRoot::open(
        &d.0,
        ResourceLimits {
            max_files: 1,
            budget: Duration::from_secs(10),
            ..ResourceLimits::default()
        },
    )
    .unwrap();
    let mut batch = root.batch();
    batch.read("one", 4).unwrap();
    assert_eq!(batch.read("two", 4).unwrap_err().code, "resource-limit");
    drop(batch);
    let root = ResourceRoot::open(
        &d.0,
        ResourceLimits {
            budget: Duration::from_millis(1),
            ..ResourceLimits::default()
        },
    )
    .unwrap();
    let mut batch = root.batch();
    std::thread::sleep(Duration::from_millis(3));
    assert_eq!(batch.read("one", 4).unwrap_err().code, "resource-deadline");
}
#[test]
fn same_opened_file_identity_reuses_bytes_across_aliases_and_checks_each_requested_limit() {
    let d = Scratch::new();
    d.write("one", b"abcd");
    fs::hard_link(d.path("one"), d.path("alias")).unwrap();
    let root = d.root();
    let mut batch = root.batch();
    let first = batch.read("one", 4).unwrap();
    let second = batch.read("alias", 4).unwrap();
    assert!(Arc::ptr_eq(&first, &second));
    assert_eq!(batch.read("alias", 3).unwrap_err().code, "resource-limit");
    assert_eq!(&*first, b"abcd");
}
#[test]
fn read_batch_excludes_writers_delete_and_parent_rename_then_releases_handles() {
    let d = Scratch::new();
    fs::create_dir(d.path("sub")).unwrap();
    d.write("sub/one", b"old");
    let root = d.root();
    let mut batch = root.batch();
    let old = batch.read("sub/one", 10).unwrap();
    assert!(
        OpenOptions::new()
            .write(true)
            .open(d.path("sub/one"))
            .is_err()
    );
    assert!(fs::remove_file(d.path("sub/one")).is_err());
    assert!(fs::rename(d.path("sub"), d.path("moved")).is_err());
    drop(batch);
    d.write("sub/one", b"new");
    assert_eq!(&*root.batch().read("sub/one", 10).unwrap(), b"new");
    assert_eq!(&*old, b"old");
    fs::remove_file(d.path("sub/one")).unwrap();
    assert_eq!(&*old, b"old");
}
#[test]
fn existing_writer_is_rejected_and_failed_read_does_not_poison_next_batch() {
    let d = Scratch::new();
    d.write("one", b"old");
    let root = d.root();
    let writer = OpenOptions::new().write(true).open(d.path("one")).unwrap();
    assert_eq!(
        root.batch().read("one", 10).unwrap_err().code,
        "resource-busy"
    );
    drop(writer);
    assert_eq!(&*root.batch().read("one", 10).unwrap(), b"old");
}
fn junction(link: &Path, target: &Path) {
    let output = std::process::Command::new("cmd")
        .args(["/c", "mklink", "/J"])
        .arg(link)
        .arg(target)
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "junction creation failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
}
#[test]
fn internal_external_and_replaced_junctions_are_rejected_without_following() {
    let d = Scratch::new();
    let outside = Scratch::new();
    outside.write("secret", b"canary");
    fs::create_dir(d.path("inside")).unwrap();
    d.write("inside/file", b"inside");
    junction(&d.path("link"), &outside.0);
    let root = d.root();
    assert_eq!(
        root.batch().read("link/secret", 100).unwrap_err().code,
        "resource-path"
    );
    fs::remove_dir(d.path("link")).unwrap();
    junction(&d.path("link"), &d.path("inside"));
    assert_eq!(
        root.batch().read("link/file", 100).unwrap_err().code,
        "resource-path"
    );
    fs::remove_dir(d.path("link")).unwrap();
    // The already held snapshot is not re-opened through a replaced directory name.
    let bytes = root.batch().read("inside/file", 100).unwrap();
    fs::rename(d.path("inside"), d.path("saved")).unwrap();
    junction(&d.path("inside"), &outside.0);
    assert_eq!(&*bytes, b"inside");
    assert_eq!(
        root.batch().read("inside/secret", 100).unwrap_err().code,
        "resource-path"
    );
    fs::remove_dir(d.path("inside")).unwrap();
    assert_eq!(fs::read(outside.path("secret")).unwrap(), b"canary");
}
#[test]
fn root_must_be_explicit_absolute_directory_and_its_handle_prevents_replacement() {
    let d = Scratch::new();
    d.write("file", b"data");
    for path in [
        Path::new("."),
        Path::new("C:relative"),
        Path::new(r"\\server\share"),
    ] {
        assert!(ResourceRoot::open(path, ResourceLimits::default()).is_err());
    }
    assert!(ResourceRoot::open(&d.path("file"), ResourceLimits::default()).is_err());
    assert!(
        ResourceRoot::open(
            &d.0,
            ResourceLimits {
                max_files: 0,
                ..ResourceLimits::default()
            }
        )
        .is_err()
    );
    let root = d.root();
    let moved = d.0.with_extension("moved");
    let allowed = std::env::temp_dir().join("diy-ime2-resource-tests");
    assert!(
        d.0.is_absolute()
            && moved.is_absolute()
            && d.0.starts_with(&allowed)
            && moved.starts_with(&allowed)
    );
    assert!(fs::rename(&d.0, moved).is_err());
    drop(root);
}

use diy_ime2_core::{
    Candidate, Packet,
    graph::Graph,
    node::{
        CoreError, Inputs, Node, NodeDefinition, OutputValue, Outputs, Port, PreparedNode,
        Registry, RunContext,
    },
    session::{Request, Session},
};
use serde_json::{Value, json};
struct FileSource;
struct FileSnapshot(Arc<[u8]>);
impl Node for FileSource {
    fn definition(&self) -> NodeDefinition {
        NodeDefinition::new("test.file", 1, vec![], vec![Port::packet("out")])
    }
    fn prepare(
        &self,
        _: &Value,
        resources: &mut dyn PrepareResources,
    ) -> Result<Option<Arc<dyn PreparedNode>>, CoreError> {
        Ok(Some(Arc::new(FileSnapshot(resources.read("one", 32)?))))
    }
    fn execute(&self, _: RunContext<'_>, _: &Inputs, _: &Value) -> Result<Outputs, CoreError> {
        panic!("unprepared")
    }
}
impl PreparedNode for FileSnapshot {
    fn execute(&self, _: RunContext<'_>, _: &Inputs) -> Result<Outputs, CoreError> {
        Ok([(
            "out".into(),
            OutputValue::Data(Arc::new(Packet::new(Candidate::Text {
                text: String::from_utf8(self.0.to_vec()).unwrap(),
                meta: None,
            }))),
        )]
        .into())
    }
}
fn registry() -> Registry {
    let mut r = Registry::new();
    r.register(FileSource).unwrap();
    r
}
fn file_graph() -> Graph {
    Graph::from_json(
        &json!({"version":1,"nodes":[{"id":"source","kind":"test.file","version":1}],"edges":[]})
            .to_string(),
    )
    .unwrap()
}
fn v2_value(session: &mut Session, version: u64) -> Value {
    session
        .handle(Request::Run {
            expected_plan_version: version,
            input: None,
        })
        .unwrap()["nodes"]["source"]["outputs"]["out"]["packet"]["root"]["text"]
        .clone()
}
#[test]
fn v2_sessions_keep_plan_snapshots_after_deletion_failed_reload_and_rollback() {
    let a = Scratch::new();
    let b = Scratch::new();
    a.write("one", b"A");
    b.write("one", b"B");
    let mut denied = Session::new(registry());
    assert_eq!(
        denied
            .handle(Request::Prepare {
                graph: file_graph()
            })
            .unwrap_err()
            .code,
        "resource-unavailable"
    );
    let mut first = Session::with_resource_root(registry(), Some(Arc::new(a.root())));
    let mut second = Session::with_resource_root(registry(), Some(Arc::new(b.root())));
    first
        .handle(Request::Prepare {
            graph: file_graph(),
        })
        .unwrap();
    second
        .handle(Request::Prepare {
            graph: file_graph(),
        })
        .unwrap();
    fs::remove_file(a.path("one")).unwrap();
    first
        .handle(Request::Activate { token: "1".into() })
        .unwrap();
    second
        .handle(Request::Activate { token: "1".into() })
        .unwrap();
    assert_eq!(v2_value(&mut first, 1), "A");
    assert_eq!(v2_value(&mut second, 1), "B");
    let before = serde_json::to_value(first.state()).unwrap();
    assert!(
        first
            .handle(Request::Prepare {
                graph: file_graph()
            })
            .is_err()
    );
    assert_eq!(serde_json::to_value(first.state()).unwrap(), before);
    assert_eq!(v2_value(&mut first, 1), "A");
    a.write("one", b"new");
    first
        .handle(Request::Prepare {
            graph: file_graph(),
        })
        .unwrap();
    first
        .handle(Request::Activate { token: "2".into() })
        .unwrap();
    assert_eq!(v2_value(&mut first, 2), "new");
    first
        .handle(Request::Rollback {
            expected_plan_version: 2,
        })
        .unwrap();
    assert_eq!(v2_value(&mut first, 3), "A");
    assert_eq!(v2_value(&mut second, 1), "B");
}
fn service() -> diy_ime2_core::execution_control::ExecutionService {
    use diy_ime2_core::execution_control::{ExecutionHost, ExecutionService, ResourcePolicy};
    static NEXT: AtomicUsize = AtomicUsize::new(0);
    let profile = serde_json::from_str::<Value>(include_str!("contracts/execution_control.json"))
        .unwrap()["resourceProfiles"]["bounded"]
        .clone();
    ExecutionService::new(
        ExecutionHost::new(
            format!("resource-test-{}", NEXT.fetch_add(1, Ordering::SeqCst)),
            ResourcePolicy::from_value(profile).unwrap(),
        )
        .unwrap(),
    )
    .unwrap()
}
fn v3(session: &mut diy_ime2_core::execution_control::session::SessionV3, request: Value) -> Value {
    serde_json::from_slice(
        &session
            .reply(&serde_json::to_vec(&json!({"protocol":3,"id":"1","request":request})).unwrap())
            .unwrap(),
    )
    .unwrap()
}
#[test]
fn v3_session_uses_explicit_host_root_and_snapshot_without_changing_wire_contract() {
    use diy_ime2_core::execution_control::session::SessionV3;
    let d = Scratch::new();
    d.write("one", b"v3-snapshot");
    let service = service();
    let mut denied = SessionV3::new(&service, registry()).unwrap();
    let response = v3(
        &mut denied,
        json!({"method":"prepare","graph":file_graph()}),
    );
    assert_eq!(response["error"]["code"], "resource-unavailable");
    let mut session = SessionV3::with_registry_and_resource_root(
        &service,
        Arc::new(registry()),
        Some(Arc::new(d.root())),
    )
    .unwrap();
    let prepared = v3(
        &mut session,
        json!({"method":"prepare","graph":file_graph()}),
    );
    assert_eq!(prepared["ok"], true, "{prepared}");
    fs::remove_file(d.path("one")).unwrap();
    assert_eq!(
        v3(
            &mut session,
            json!({"method":"activate","token":prepared["data"]["token"]})
        )["ok"],
        true
    );
    let run = v3(
        &mut session,
        json!({"method":"start-run","startKey":"1","expectedPlanVersion":"1","targets":[{"node":"source","port":"out"}]}),
    );
    assert_eq!(run["ok"], true, "{run}");
    let key = run["data"]["key"].clone();
    let end = std::time::Instant::now() + Duration::from_secs(5);
    loop {
        let state = v3(&mut session, json!({"method":"get-run","key":key}));
        assert_eq!(state["ok"], true, "{state}");
        if !state["data"]["result"].is_null() {
            assert_eq!(
                state["data"]["result"]["ports"][0]["value"]["packet"]["root"]["text"],
                "v3-snapshot"
            );
            break;
        }
        assert!(std::time::Instant::now() < end);
        std::thread::yield_now();
    }
}
#[test]
fn broker_passes_root_to_each_session_and_client_cannot_supply_one() {
    use diy_ime2_core::execution_control::broker::Broker;
    fn send(b: &mut Broker, request: Value) -> Value {
        serde_json::from_slice(
            &b.reply(&serde_json::to_vec(&json!({"broker":1,"id":"1","request":request})).unwrap())
                .unwrap(),
        )
        .unwrap()
    }
    let d = Scratch::new();
    d.write("one", b"broker");
    let service = service();
    let mut b = Broker::with_resource_root(&service, registry(), Some(Arc::new(d.root())));
    let invalid = send(
        &mut b,
        json!({"method":"open-session","resourceRoot":"C:/"}),
    );
    assert_eq!(invalid["ok"], false);
    for _ in 0..2 {
        let epoch = send(&mut b, json!({"method":"open-session"}))["data"]["sessionEpoch"].clone();
        let prepared = send(
            &mut b,
            json!({"method":"request","sessionEpoch":epoch,"message":{"protocol":3,"id":"1","request":{"method":"prepare","graph":file_graph()}}}),
        );
        assert_eq!(prepared["data"]["ok"], true, "{prepared}");
        assert_eq!(
            send(
                &mut b,
                json!({"method":"close-session","sessionEpoch":epoch})
            )["ok"],
            true
        );
    }
}
#[test]
fn cli_reads_explicit_root_only_at_startup_and_default_commands_remain_compatible() {
    use diy_ime2_core::resources::RESOURCE_ROOT_ENV;
    use std::process::{Command, Stdio};
    let d = Scratch::new();
    for env in [None, Some(d.0.as_os_str())] {
        let mut command = Command::new(env!("CARGO_BIN_EXE_diy-ime2-test-host"));
        command.arg("session").stdin(Stdio::null());
        if let Some(value) = env {
            command.env(RESOURCE_ROOT_ENV, value);
        } else {
            command.env_remove(RESOURCE_ROOT_ENV);
        }
        let output = command.output().unwrap();
        assert!(
            output.status.success(),
            "{}",
            String::from_utf8_lossy(&output.stderr)
        );
    }
    let invalid = Command::new(env!("CARGO_BIN_EXE_diy-ime2-test-host"))
        .arg("session")
        .env(RESOURCE_ROOT_ENV, "")
        .stdin(Stdio::null())
        .output()
        .unwrap();
    assert!(!invalid.status.success());
    assert!(String::from_utf8_lossy(&invalid.stderr).contains("resource-config"));
    let definitions = Command::new(env!("CARGO_BIN_EXE_diy-ime2-test-host"))
        .arg("list-nodes")
        .env(RESOURCE_ROOT_ENV, "")
        .output()
        .unwrap();
    assert!(definitions.status.success());
}

#[test]
fn writable_memory_mapping_is_rejected_even_after_original_file_handle_closes() {
    use std::{
        ffi::c_void,
        os::windows::{fs::OpenOptionsExt, io::AsRawHandle},
    };
    #[link(name = "kernel32")]
    unsafe extern "system" {
        fn CreateFileMappingW(
            file: *mut c_void,
            attributes: *mut c_void,
            protect: u32,
            high: u32,
            low: u32,
            name: *const u16,
        ) -> *mut c_void;
        fn MapViewOfFile(
            mapping: *mut c_void,
            access: u32,
            high: u32,
            low: u32,
            count: usize,
        ) -> *mut c_void;
        fn UnmapViewOfFile(view: *const c_void) -> i32;
        fn CloseHandle(handle: *mut c_void) -> i32;
    }
    struct Mapping {
        handle: *mut c_void,
        view: *mut c_void,
    }
    impl Drop for Mapping {
        fn drop(&mut self) {
            // SAFETY: both objects were created by the calls below, and each is released once.
            unsafe {
                if !self.view.is_null() {
                    UnmapViewOfFile(self.view);
                }
                if !self.handle.is_null() {
                    CloseHandle(self.handle);
                }
            }
        }
    }
    let d = Scratch::new();
    d.write("one", b"mapping");
    let root = d.root();
    let writer = OpenOptions::new()
        .read(true)
        .write(true)
        .share_mode(7)
        .open(d.path("one"))
        .unwrap();
    // SAFETY: live file handle; null optional security/name pointers; read/write protection.
    let handle = unsafe {
        CreateFileMappingW(
            writer.as_raw_handle(),
            std::ptr::null_mut(),
            4,
            0,
            0,
            std::ptr::null(),
        )
    };
    assert!(!handle.is_null());
    let mut mapping = Mapping {
        handle,
        view: std::ptr::null_mut(),
    };
    // SAFETY: live mapping, offset 0 and length 0 select its valid full view.
    mapping.view = unsafe { MapViewOfFile(handle, 2, 0, 0, 0) };
    assert!(!mapping.view.is_null());
    drop(writer);
    assert_eq!(
        root.batch().read("one", 100).unwrap_err().code,
        "resource-busy"
    );
    drop(mapping);
    assert_eq!(&*root.batch().read("one", 100).unwrap(), b"mapping");
}
