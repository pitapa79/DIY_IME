//! Hostが明示したローカル資源の取得。辞書形式や変換の意味は扱わない。
use crate::{
    graph::{ExecutionPlan, Graph},
    node::{CoreError, PrepareResources, Registry},
};
use std::{
    collections::BTreeMap,
    fs::File,
    io::Read,
    path::Path,
    sync::Arc,
    time::{Duration, Instant},
};
#[cfg(windows)]
mod windows;
#[cfg(windows)]
use windows as platform;
#[cfg(not(windows))]
mod platform {
    use super::*;
    pub struct Root;
    impl Root {
        pub fn open(_: &Path) -> Result<Self, CoreError> {
            Err(CoreError::new(
                "resource-platform",
                "local resource access is only implemented for Windows",
            ))
        }
        pub fn file(&self, _: &str) -> Result<OpenedResource, CoreError> {
            Err(CoreError::new(
                "resource-platform",
                "local resource access is only implemented for Windows",
            ))
        }
    }
}
type FileKey = (u64, [u8; 16]);
pub(super) struct OpenedResource {
    file: File,
    key: FileKey,
    ancestors: Vec<File>,
}

pub const RESOURCE_ROOT_ENV: &str = "DIY_IME_RESOURCE_ROOT";
#[derive(Debug, Clone)]
pub struct ResourceLimits {
    pub max_file_bytes: usize,
    pub max_total_bytes: usize,
    pub max_files: usize,
    /// 協調期限。同期OS I/Oの強制中断を保証しない。
    pub budget: Duration,
}
impl Default for ResourceLimits {
    fn default() -> Self {
        Self {
            max_file_bytes: 8 * 1024 * 1024,
            max_total_bytes: 16 * 1024 * 1024,
            max_files: 32,
            budget: Duration::from_millis(500),
        }
    }
}
pub struct ResourceRoot {
    root: platform::Root,
    limits: ResourceLimits,
}
impl ResourceRoot {
    pub fn open(path: &Path, limits: ResourceLimits) -> Result<Self, CoreError> {
        if limits.max_file_bytes == 0
            || limits.max_total_bytes == 0
            || limits.max_files == 0
            || limits.budget.is_zero()
        {
            return Err(CoreError::new(
                "resource-config",
                "resource limits must be positive",
            ));
        }
        Ok(Self {
            root: platform::Root::open(path)?,
            limits,
        })
    }
    /// 起動側だけが呼ぶ。未設定は許可なし。空文字は設定エラー。
    pub fn from_env() -> Result<Option<Arc<Self>>, CoreError> {
        match std::env::var_os(RESOURCE_ROOT_ENV) {
            None => Ok(None),
            Some(value) if value.is_empty() => Err(CoreError::new(
                "resource-config",
                "DIY_IME_RESOURCE_ROOT must not be empty",
            )),
            Some(value) => Self::open(Path::new(&value), ResourceLimits::default())
                .map(|root| Some(Arc::new(root))),
        }
    }
    pub fn batch(&self) -> ResourceBatch<'_> {
        ResourceBatch {
            root: self,
            files: BTreeMap::new(),
            total: 0,
            start: Instant::now(),
        }
    }
    pub fn compile(&self, graph: &Graph, registry: &Registry) -> Result<ExecutionPlan, CoreError> {
        ExecutionPlan::compile_with_resources(graph, registry, &mut self.batch())
    }
}
struct Snapshot {
    bytes: Arc<[u8]>,
    _file: File,
    _ancestors: Vec<File>,
}
pub struct ResourceBatch<'a> {
    root: &'a ResourceRoot,
    files: BTreeMap<FileKey, Snapshot>,
    total: usize,
    start: Instant,
}
fn failure(code: &str, message: &str) -> CoreError {
    CoreError::new(code, message)
}
impl ResourceBatch<'_> {
    fn time(&self) -> Result<(), CoreError> {
        if self.start.elapsed() >= self.root.limits.budget {
            Err(failure(
                "resource-deadline",
                "resource preparation budget elapsed",
            ))
        } else {
            Ok(())
        }
    }
}
impl PrepareResources for ResourceBatch<'_> {
    fn read(&mut self, reference: &str, max_bytes: usize) -> Result<Arc<[u8]>, CoreError> {
        self.time()?;
        let OpenedResource {
            mut file,
            key,
            ancestors,
        } = self.root.root.file(reference)?;
        self.time()?;
        let maximum = max_bytes.min(self.root.limits.max_file_bytes);
        if let Some(snapshot) = self.files.get(&key) {
            return if snapshot.bytes.len() > maximum {
                Err(failure(
                    "resource-limit",
                    "resource exceeds requested bytes",
                ))
            } else {
                Ok(Arc::clone(&snapshot.bytes))
            };
        }
        if self.files.len() >= self.root.limits.max_files {
            return Err(failure("resource-limit", "resource file count exceeded"));
        }
        let maximum = maximum.min(self.root.limits.max_total_bytes.saturating_sub(self.total));
        let len = file
            .metadata()
            .map_err(|_| failure("resource-read", "cannot inspect resource"))?
            .len();
        if len > maximum as u64 {
            return Err(failure("resource-limit", "resource bytes exceeded"));
        }
        let mut bytes = Vec::new();
        let mut chunk = [0u8; 64 * 1024];
        loop {
            self.time()?;
            let remaining = maximum.saturating_sub(bytes.len());
            let capacity = chunk.len().min(remaining.saturating_add(1));
            let count = file
                .read(&mut chunk[..capacity])
                .map_err(|_| failure("resource-read", "cannot read resource"))?;
            self.time()?;
            if count == 0 {
                break;
            }
            if count > remaining {
                return Err(failure(
                    "resource-limit",
                    "resource bytes exceeded while reading",
                ));
            }
            bytes.extend_from_slice(&chunk[..count]);
        }
        let bytes: Arc<[u8]> = Arc::from(bytes);
        self.total += bytes.len();
        self.files.insert(
            key,
            Snapshot {
                bytes: Arc::clone(&bytes),
                _file: file,
                _ancestors: ancestors,
            },
        );
        Ok(bytes)
    }
}
/// 既存compileと同じ既定拒否を保ち、Host設定がある場合だけ新バッチを作る。
pub fn compile(
    graph: &Graph,
    registry: &Registry,
    root: Option<&ResourceRoot>,
) -> Result<ExecutionPlan, CoreError> {
    match root {
        Some(root) => root.compile(graph, registry),
        None => ExecutionPlan::compile(graph, registry),
    }
}
