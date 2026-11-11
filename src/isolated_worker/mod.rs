//! Windows Host Worker launch primitives. No ambient-privilege fallback.
use std::{
    ffi::{OsStr, OsString},
    io::{self, Read},
    mem::{size_of, zeroed},
    os::windows::{ffi::OsStrExt, io::FromRawHandle},
    path::{Path, PathBuf},
    ptr, thread,
};
use windows_sys::Win32::{
    Foundation::*,
    Security::{Authorization::*, Cryptography::*, Isolation::*, *},
    Storage::FileSystem::*,
    System::{JobObjects::*, Pipes::*, Threading::*},
};
fn wide(value: &OsStr) -> Vec<u16> {
    value.encode_wide().chain(Some(0)).collect()
}
fn ok(value: i32, what: &str) -> Result<(), String> {
    if value != 0 {
        Ok(())
    } else {
        Err(format!("{what}: {}", io::Error::last_os_error()))
    }
}
pub struct OwnedHandle(HANDLE);
impl Drop for OwnedHandle {
    fn drop(&mut self) {
        if !self.0.is_null() && self.0 != INVALID_HANDLE_VALUE {
            unsafe {
                CloseHandle(self.0);
            }
        }
    }
}
impl OwnedHandle {
    fn new(handle: HANDLE, what: &str) -> Result<Self, String> {
        if handle.is_null() || handle == INVALID_HANDLE_VALUE {
            Err(format!("{what}: {}", io::Error::last_os_error()))
        } else {
            Ok(Self(handle))
        }
    }
}
struct LocalMemory(*mut std::ffi::c_void);
impl Drop for LocalMemory {
    fn drop(&mut self) {
        if !self.0.is_null() {
            unsafe {
                LocalFree(self.0);
            }
        }
    }
}
pub struct Profile {
    name: Vec<u16>,
    sid: PSID,
    deleted: bool,
}
impl Profile {
    pub fn new() -> Result<Self, String> {
        let mut random = [0u8; 16];
        // SAFETY: system-preferred RNG accepts a null provider and initialized buffer.
        let status = unsafe {
            BCryptGenRandom(
                ptr::null_mut(),
                random.as_mut_ptr(),
                16,
                BCRYPT_USE_SYSTEM_PREFERRED_RNG,
            )
        };
        if status < 0 {
            return Err(format!("profile nonce unavailable {status:#x}"));
        }
        let suffix = random
            .iter()
            .map(|b| format!("{b:02x}"))
            .collect::<String>();
        let name = wide(OsStr::new(&format!("DIYIME2.Worker.{suffix}")));
        let mut sid = ptr::null_mut();
        // SAFETY: valid terminated names, no requested OS capabilities, owned returned SID.
        let hr = unsafe {
            CreateAppContainerProfile(
                name.as_ptr(),
                name.as_ptr(),
                name.as_ptr(),
                ptr::null(),
                0,
                &mut sid,
            )
        };
        if hr < 0 {
            return Err(format!(
                "AppContainer profile unavailable ({hr:#x}); refusing unisolated Worker"
            ));
        }
        if sid.is_null() {
            return Err("AppContainer returned no SID".into());
        }
        Ok(Self {
            name,
            sid,
            deleted: false,
        })
    }
    pub fn delete(&mut self) -> Result<(), String> {
        Self::delete_named(&self.name)?;
        self.deleted = true;
        Ok(())
    }
    fn delete_named(name: &[u16]) -> Result<(), String> {
        if name.last() != Some(&0) {
            return Err("profile name is not terminated".into());
        }
        let hr = unsafe { DeleteAppContainerProfile(name.as_ptr()) };
        if hr < 0 {
            return Err(format!("profile cleanup failed {hr:#x}"));
        }
        Ok(())
    }
    pub fn name(&self) -> String {
        String::from_utf16_lossy(&self.name[..self.name.len() - 1])
    }
}
impl Drop for Profile {
    fn drop(&mut self) {
        unsafe {
            if !self.deleted {
                DeleteAppContainerProfile(self.name.as_ptr());
            }
            if !self.sid.is_null() {
                FreeSid(self.sid);
            }
        }
    }
}

/// A newly created workspace-target cache. ACL edits are confined to this directory.
fn asset_name(name: &str) -> Result<(), String> {
    let stem = name.split('.').next().unwrap_or("").to_ascii_uppercase();
    if name.is_empty()
        || name.starts_with('.')
        || name.ends_with('.')
        || !name
            .bytes()
            .all(|c| c.is_ascii_alphanumeric() || b"._-".contains(&c))
        || matches!(stem.as_str(), "CON" | "PRN" | "AUX" | "NUL")
        || (stem.len() == 4
            && (stem.starts_with("COM") || stem.starts_with("LPT"))
            && matches!(stem.as_bytes()[3], b'1'..=b'9'))
    {
        return Err("invalid cache asset name".into());
    }
    Ok(())
}
pub struct Cache {
    root: PathBuf,
}
impl Cache {
    pub fn new(workspace: &Path, profile: &Profile) -> Result<Self, String> {
        let workspace = std::fs::canonicalize(workspace).map_err(|e| e.to_string())?;
        let target = workspace.join("target/appcontainer-probes");
        std::fs::create_dir_all(&target).map_err(|e| e.to_string())?;
        let target = std::fs::canonicalize(target).map_err(|e| e.to_string())?;
        if !target.starts_with(&workspace) {
            return Err("Worker cache escapes workspace".into());
        }
        let root = target.join(profile.name());
        std::fs::create_dir(&root).map_err(|e| e.to_string())?;
        Ok(Self { root })
    }
    pub fn path(&self) -> &Path {
        &self.root
    }
    pub fn copy(&self, source: &Path, name: &str) -> Result<PathBuf, String> {
        asset_name(name)?;
        let destination = self.root.join(name);
        if destination.exists() {
            return Err("cache asset already exists".into());
        }
        std::fs::copy(source, &destination).map_err(|e| e.to_string())?;
        Ok(destination)
    }
    pub fn write(&self, name: &str, bytes: &[u8]) -> Result<PathBuf, String> {
        asset_name(name)?;
        let p = self.root.join(name);
        let mut file = std::fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&p)
            .map_err(|e| e.to_string())?;
        std::io::Write::write_all(&mut file, bytes).map_err(|e| e.to_string())?;
        Ok(p)
    }
    pub fn private_canary(&self, bytes: &[u8]) -> Result<PathBuf, String> {
        let name = self
            .root
            .file_name()
            .ok_or("invalid cache name")?
            .to_string_lossy();
        let path = self
            .root
            .parent()
            .ok_or("invalid cache parent")?
            .join(format!("{name}.private-canary"));
        let mut file = std::fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&path)
            .map_err(|e| e.to_string())?;
        std::io::Write::write_all(&mut file, bytes).map_err(|e| e.to_string())?;
        drop(file);
        private_acl(&path)?;
        Ok(path)
    }
    fn change_inherited_code_access(
        &self,
        profile: &Profile,
        expected_files: usize,
        allow: bool,
    ) -> Result<(), String> {
        use std::os::windows::fs::MetadataExt;

        // The caller owns read locks on every captured asset. Reject other
        // entries before propagating the SID to existing child files.
        let mut count = 0;
        for entry in std::fs::read_dir(&self.root).map_err(|e| e.to_string())? {
            let metadata = entry
                .and_then(|entry| entry.metadata())
                .map_err(|e| e.to_string())?;
            if !metadata.is_file() || metadata.file_attributes() & FILE_ATTRIBUTE_REPARSE_POINT != 0
            {
                return Err("unexpected immutable code cache object".into());
            }
            count += 1;
        }
        if count != expected_files {
            return Err("unexpected immutable code cache file count".into());
        }
        let mode = if allow { GRANT_ACCESS } else { REVOKE_ACCESS };
        change_access_with_inheritance(&self.root, profile.sid, mode, OBJECT_INHERIT_ACE)
    }
    /// Remove only this invocation's unique SID after its actual process ending.
    pub fn revoke_read_execute(&self, profile: &Profile) -> Result<(), String> {
        for entry in std::fs::read_dir(&self.root).map_err(|e| e.to_string())? {
            let entry = entry.map_err(|e| e.to_string())?;
            if !entry.file_type().map_err(|e| e.to_string())?.is_file() {
                return Err("unexpected shared cache object".into());
            }
            revoke(&entry.path(), profile.sid)?;
        }
        revoke(&self.root, profile.sid)
    }
    pub fn allow_read_execute(&self, profile: &Profile) -> Result<(), String> {
        grant(&self.root, profile.sid)?;
        for entry in std::fs::read_dir(&self.root).map_err(|e| e.to_string())? {
            let entry = entry.map_err(|e| e.to_string())?;
            if !entry.file_type().map_err(|e| e.to_string())?.is_file() {
                return Err("unexpected cache directory/link".into());
            }
            grant(&entry.path(), profile.sid)?;
        }
        Ok(())
    }
}
// Only the newly generated private fixture is edited. This models a user-private
// file even on CI workspaces whose inherited ACL is broadly readable.
fn private_acl(path: &Path) -> Result<(), String> {
    let mut raw = ptr::null_mut();
    ok(
        unsafe { OpenProcessToken(GetCurrentProcess(), TOKEN_QUERY, &mut raw) },
        "open own token",
    )?;
    let token = OwnedHandle::new(raw, "own token")?;
    let mut length = 0;
    unsafe {
        GetTokenInformation(token.0, TokenUser, ptr::null_mut(), 0, &mut length);
    }
    if length == 0 || length > 65536 {
        return Err("invalid own TokenUser length".into());
    }
    let mut buffer = vec![0usize; (length as usize).div_ceil(size_of::<usize>())];
    ok(
        unsafe {
            GetTokenInformation(
                token.0,
                TokenUser,
                buffer.as_mut_ptr().cast(),
                length,
                &mut length,
            )
        },
        "read own TokenUser",
    )?;
    let user = unsafe { &*buffer.as_ptr().cast::<TOKEN_USER>() };
    let mut sid = ptr::null_mut();
    ok(
        unsafe { ConvertSidToStringSidW(user.User.Sid, &mut sid) },
        "format own SID",
    )?;
    let _sid = LocalMemory(sid.cast());
    let mut size = 0;
    while size < 256 && unsafe { *sid.add(size) } != 0 {
        size += 1;
    }
    if size == 256 {
        return Err("invalid own SID text".into());
    }
    let sid_text = String::from_utf16(unsafe { std::slice::from_raw_parts(sid, size) })
        .map_err(|_| "invalid SID encoding")?;
    let sddl = wide(OsStr::new(&format!("D:P(A;;FA;;;SY)(A;;FA;;;{sid_text})")));
    let mut descriptor = ptr::null_mut();
    ok(
        unsafe {
            ConvertStringSecurityDescriptorToSecurityDescriptorW(
                sddl.as_ptr(),
                SDDL_REVISION_1,
                &mut descriptor,
                ptr::null_mut(),
            )
        },
        "build private fixture ACL",
    )?;
    let _descriptor = LocalMemory(descriptor);
    let mut present = 0;
    let mut defaulted = 0;
    let mut acl = ptr::null_mut();
    ok(
        unsafe { GetSecurityDescriptorDacl(descriptor, &mut present, &mut acl, &mut defaulted) },
        "read private fixture ACL",
    )?;
    if present == 0 || acl.is_null() {
        return Err("private fixture ACL absent".into());
    }
    let name = wide(path.as_os_str());
    let status = unsafe {
        SetNamedSecurityInfoW(
            name.as_ptr(),
            SE_FILE_OBJECT,
            DACL_SECURITY_INFORMATION | PROTECTED_DACL_SECURITY_INFORMATION,
            ptr::null_mut(),
            ptr::null_mut(),
            acl,
            ptr::null(),
        )
    };
    if status != 0 {
        return Err(format!("private fixture ACL failed {status}"));
    }
    Ok(())
}
fn grant(path: &Path, sid: PSID) -> Result<(), String> {
    change_access(path, sid, GRANT_ACCESS)
}
fn revoke(path: &Path, sid: PSID) -> Result<(), String> {
    change_access(path, sid, REVOKE_ACCESS)
}
fn change_access(path: &Path, sid: PSID, mode: ACCESS_MODE) -> Result<(), String> {
    change_access_with_inheritance(path, sid, mode, 0)
}
fn change_access_with_inheritance(
    path: &Path,
    sid: PSID,
    mode: ACCESS_MODE,
    inheritance: u32,
) -> Result<(), String> {
    let name = wide(path.as_os_str());
    let mut old = ptr::null_mut();
    let mut descriptor = ptr::null_mut();
    // SAFETY: named generated cache object; returned descriptor owns old ACL lifetime.
    let status = unsafe {
        GetNamedSecurityInfoW(
            name.as_ptr(),
            SE_FILE_OBJECT,
            DACL_SECURITY_INFORMATION,
            ptr::null_mut(),
            ptr::null_mut(),
            &mut old,
            ptr::null_mut(),
            &mut descriptor,
        )
    };
    let _descriptor = LocalMemory(descriptor);
    if status != 0 {
        return Err(format!("read cache ACL failed {status}"));
    }
    let trustee = TRUSTEE_W {
        pMultipleTrustee: ptr::null_mut(),
        MultipleTrusteeOperation: NO_MULTIPLE_TRUSTEE,
        TrusteeForm: TRUSTEE_IS_SID,
        TrusteeType: TRUSTEE_IS_UNKNOWN,
        ptstrName: sid.cast(),
    };
    let entry = EXPLICIT_ACCESS_W {
        grfAccessPermissions: FILE_GENERIC_READ | FILE_GENERIC_EXECUTE,
        grfAccessMode: mode,
        grfInheritance: inheritance,
        Trustee: trustee,
    };
    let mut acl = ptr::null_mut();
    let status = unsafe { SetEntriesInAclW(1, &entry, old, &mut acl) };
    let _acl = LocalMemory(acl.cast());
    if status != 0 {
        return Err(format!("build cache ACL failed {status}"));
    }
    let status = unsafe {
        SetNamedSecurityInfoW(
            name.as_ptr(),
            SE_FILE_OBJECT,
            DACL_SECURITY_INFORMATION,
            ptr::null_mut(),
            ptr::null_mut(),
            acl,
            ptr::null(),
        )
    };
    if status != 0 {
        return Err(format!("write generated cache ACL failed {status}"));
    }
    Ok(())
}
struct Attributes<'a> {
    _buffer: Vec<usize>,
    list: LPPROC_THREAD_ATTRIBUTE_LIST,
    // UpdateProcThreadAttribute retains lpValue until DeleteProcThreadAttributeList.
    _values: std::marker::PhantomData<&'a ()>,
}
impl<'a> Attributes<'a> {
    fn new() -> Result<Self, String> {
        let mut size = 0;
        unsafe {
            InitializeProcThreadAttributeList(ptr::null_mut(), 3, 0, &mut size);
        }
        if size == 0 || size > 65536 {
            return Err("invalid startup attribute size".into());
        }
        let mut buffer = vec![0usize; size.div_ceil(size_of::<usize>())];
        let list = buffer.as_mut_ptr().cast();
        ok(
            unsafe { InitializeProcThreadAttributeList(list, 3, 0, &mut size) },
            "initialize attributes",
        )?;
        Ok(Self {
            _buffer: buffer,
            list,
            _values: std::marker::PhantomData,
        })
    }
    fn set<T>(&mut self, key: u32, value: &'a T) -> Result<(), String> {
        ok(
            unsafe {
                UpdateProcThreadAttribute(
                    self.list,
                    0,
                    key as usize,
                    ptr::from_ref(value).cast(),
                    size_of::<T>(),
                    ptr::null_mut(),
                    ptr::null(),
                )
            },
            "set Worker startup attribute",
        )
    }
}
impl Drop for Attributes<'_> {
    fn drop(&mut self) {
        unsafe {
            DeleteProcThreadAttributeList(self.list);
        }
    }
}
pub struct Job(OwnedHandle);
impl Job {
    /// Developer probe default. Product callers choose their own memory policy.
    pub fn new() -> Result<Self, String> {
        Self::with_memory_limit(Some(512 * 1024 * 1024))
    }
    pub fn with_memory_limit(memory_bytes: Option<usize>) -> Result<Self, String> {
        if memory_bytes == Some(0) {
            return Err("Worker memory limit must be positive".into());
        }
        let job = OwnedHandle::new(
            unsafe { CreateJobObjectW(ptr::null(), ptr::null()) },
            "create Job",
        )?;
        let mut limits: JOBOBJECT_EXTENDED_LIMIT_INFORMATION = unsafe { zeroed() };
        limits.BasicLimitInformation.LimitFlags =
            JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE | JOB_OBJECT_LIMIT_ACTIVE_PROCESS;
        limits.BasicLimitInformation.ActiveProcessLimit = 1;
        if let Some(bytes) = memory_bytes {
            limits.BasicLimitInformation.LimitFlags |= JOB_OBJECT_LIMIT_PROCESS_MEMORY;
            limits.ProcessMemoryLimit = bytes;
        }
        ok(
            unsafe {
                SetInformationJobObject(
                    job.0,
                    JobObjectExtendedLimitInformation,
                    ptr::from_ref(&limits).cast(),
                    size_of::<JOBOBJECT_EXTENDED_LIMIT_INFORMATION>() as u32,
                )
            },
            "set Worker Job limits",
        )?;
        Ok(Self(job))
    }
    fn terminate(&self) {
        unsafe {
            TerminateJobObject(self.0.0, 1);
        }
    }
}
#[derive(Debug)]
pub struct Reply {
    pub process_id: u32,
    pub exit_code: u32,
    pub stdout: Vec<u8>,
}
fn quote(value: &OsStr) -> Vec<u16> {
    let mut result = vec![b'"' as u16];
    let mut slashes = 0;
    for c in value.encode_wide() {
        if c == b'\\' as u16 {
            slashes += 1;
            continue;
        }
        if c == b'"' as u16 {
            result.extend(std::iter::repeat_n(b'\\' as u16, slashes * 2 + 1));
            result.push(c);
        } else {
            result.extend(std::iter::repeat_n(b'\\' as u16, slashes));
            result.push(c);
        }
        slashes = 0;
    }
    result.extend(std::iter::repeat_n(b'\\' as u16, slashes * 2));
    result.push(b'"' as u16);
    result
}
/// Fixed developer probe compatibility; product transport uses execute and typed outcomes.
#[doc(hidden)]
pub fn run(
    profile: &Profile,
    job: &Job,
    cache: &Cache,
    program: &Path,
    args: &[OsString],
    timeout_ms: u32,
) -> Result<Reply, String> {
    let program = std::fs::canonicalize(program).map_err(|e| e.to_string())?;
    let root = std::fs::canonicalize(cache.path()).map_err(|e| e.to_string())?;
    if !program.starts_with(&root) {
        return Err("Worker program is not a cache asset".into());
    }
    let executable = wide(program.as_os_str());
    let mut command = quote(program.as_os_str());
    for arg in args {
        command.push(b' ' as u16);
        command.extend(quote(arg));
    }
    command.push(0);
    let sa = SECURITY_ATTRIBUTES {
        nLength: size_of::<SECURITY_ATTRIBUTES>() as u32,
        lpSecurityDescriptor: ptr::null_mut(),
        bInheritHandle: 1,
    };
    let mut read = ptr::null_mut();
    let mut write = ptr::null_mut();
    ok(
        unsafe { CreatePipe(&mut read, &mut write, &sa, 0) },
        "create Worker pipe",
    )?;
    let read = OwnedHandle::new(read, "Worker read pipe")?;
    let write = OwnedHandle::new(write, "Worker write pipe")?;
    ok(
        unsafe { SetHandleInformation(read.0, HANDLE_FLAG_INHERIT, 0) },
        "protect Parent pipe handle",
    )?;
    let null = wide(OsStr::new("NUL"));
    let input = OwnedHandle::new(
        unsafe {
            CreateFileW(
                null.as_ptr(),
                GENERIC_READ,
                FILE_SHARE_READ | FILE_SHARE_WRITE,
                &sa,
                OPEN_EXISTING,
                FILE_ATTRIBUTE_NORMAL,
                ptr::null_mut(),
            )
        },
        "open Worker stdin",
    )?;
    let caps = SECURITY_CAPABILITIES {
        AppContainerSid: profile.sid,
        Capabilities: ptr::null_mut(),
        CapabilityCount: 0,
        Reserved: 0,
    };
    let handles = [input.0, write.0];
    let jobs = [job.0.0];
    let mut attributes = Attributes::new()?;
    attributes.set(PROC_THREAD_ATTRIBUTE_SECURITY_CAPABILITIES, &caps)?;
    attributes.set(PROC_THREAD_ATTRIBUTE_HANDLE_LIST, &handles)?;
    attributes.set(PROC_THREAD_ATTRIBUTE_JOB_LIST, &jobs)?;
    let mut startup: STARTUPINFOEXW = unsafe { zeroed() };
    startup.StartupInfo.cb = size_of::<STARTUPINFOEXW>() as u32;
    startup.StartupInfo.dwFlags = STARTF_USESTDHANDLES;
    startup.StartupInfo.hStdInput = input.0;
    startup.StartupInfo.hStdOutput = write.0;
    startup.StartupInfo.hStdError = write.0;
    startup.lpAttributeList = attributes.list;
    let system = std::env::var_os("SystemRoot").ok_or("SystemRoot unavailable")?;
    let mut variables = std::collections::BTreeMap::new();
    for name in [
        "SYSTEMROOT",
        "WINDIR",
        "SYSTEMDRIVE",
        "USERPROFILE",
        "LOCALAPPDATA",
        "APPDATA",
        "PROGRAMDATA",
        "PROGRAMFILES",
        "PROGRAMFILES(X86)",
        "COMMONPROGRAMFILES",
        "COMMONPROGRAMFILES(X86)",
    ] {
        if let Some(value) = std::env::var_os(name) {
            variables.insert(name.to_string(), value);
        }
    }
    variables.insert("TEMP".into(), root.as_os_str().to_owned());
    variables.insert("TMP".into(), root.as_os_str().to_owned());
    variables.insert(
        "PATH".into(),
        Path::new(&system).join("System32").into_os_string(),
    );
    let mut environment = Vec::new();
    for (name, value) in variables {
        let mut entry = OsString::from(name);
        entry.push("=");
        entry.push(value);
        environment.extend(wide(&entry));
    }
    environment.push(0);
    let directory = wide(root.as_os_str());
    let mut process: PROCESS_INFORMATION = unsafe { zeroed() };
    // No code runs before AppContainer, handle/env allowlists and Job assignment apply.
    ok(
        unsafe {
            CreateProcessW(
                executable.as_ptr(),
                command.as_mut_ptr(),
                ptr::null(),
                ptr::null(),
                1,
                EXTENDED_STARTUPINFO_PRESENT
                    | CREATE_SUSPENDED
                    | CREATE_UNICODE_ENVIRONMENT
                    | CREATE_NO_WINDOW,
                environment.as_ptr().cast(),
                directory.as_ptr(),
                &startup.StartupInfo,
                &mut process,
            )
        },
        "create isolated Worker (no fallback)",
    )?;
    let process_handle = OwnedHandle::new(process.hProcess, "Worker process")?;
    let thread_handle = OwnedHandle::new(process.hThread, "Worker thread")?;
    drop(write);
    drop(input);
    let mut in_job = 0;
    if unsafe { IsProcessInJob(process_handle.0, job.0.0, &mut in_job) } == 0 || in_job == 0 {
        job.terminate();
        if unsafe { WaitForSingleObject(process_handle.0, 5000) } != WAIT_OBJECT_0 {
            return Err("Worker physical exit unconfirmed; no replacement started".into());
        }
        return Err("Worker Job ownership not confirmed; physical process terminated".into());
    }
    let mut token = ptr::null_mut();
    let verified = (|| -> Result<(), String> {
        ok(
            unsafe { OpenProcessToken(process_handle.0, TOKEN_QUERY, &mut token) },
            "open Worker token",
        )?;
        let token = OwnedHandle::new(token, "Worker token")?;
        let mut is_container = 0u32;
        let mut length = 0;
        ok(
            unsafe {
                GetTokenInformation(
                    token.0,
                    TokenIsAppContainer,
                    ptr::from_mut(&mut is_container).cast(),
                    size_of::<u32>() as u32,
                    &mut length,
                )
            },
            "query Worker AppContainer",
        )?;
        if is_container != 1 {
            return Err("Worker token is not AppContainer".into());
        }
        let mut size = 0;
        unsafe {
            GetTokenInformation(token.0, TokenAppContainerSid, ptr::null_mut(), 0, &mut size);
        }
        if size < size_of::<TOKEN_APPCONTAINER_INFORMATION>() as u32 || size > 65536 {
            return Err("invalid Worker AppContainer SID length".into());
        }
        let mut buffer = vec![0usize; (size as usize).div_ceil(size_of::<usize>())];
        ok(
            unsafe {
                GetTokenInformation(
                    token.0,
                    TokenAppContainerSid,
                    buffer.as_mut_ptr().cast(),
                    size,
                    &mut size,
                )
            },
            "query Worker profile SID",
        )?;
        let info = unsafe { &*buffer.as_ptr().cast::<TOKEN_APPCONTAINER_INFORMATION>() };
        if info.TokenAppContainer.is_null()
            || unsafe { EqualSid(info.TokenAppContainer, profile.sid) } == 0
        {
            return Err("Worker profile SID mismatch".into());
        }
        Ok(())
    })();
    if let Err(error) = verified {
        job.terminate();
        if unsafe { WaitForSingleObject(process_handle.0, 5000) } != WAIT_OBJECT_0 {
            return Err("Worker physical exit unconfirmed; no replacement started".into());
        }
        return Err(format!("{error}; physical process terminated"));
    }
    let file = unsafe { std::fs::File::from_raw_handle(read.0) };
    std::mem::forget(read);
    let output = thread::spawn(move || {
        let mut bytes = Vec::new();
        match file.take(8 * 1024 * 1024 + 1).read_to_end(&mut bytes) {
            Ok(_) => Ok(bytes),
            Err(e) if e.kind() == io::ErrorKind::BrokenPipe => Ok(bytes),
            Err(e) => Err(e.to_string()),
        }
    });
    if unsafe { ResumeThread(thread_handle.0) } == u32::MAX {
        job.terminate();
        if unsafe { WaitForSingleObject(process_handle.0, 5000) } != WAIT_OBJECT_0 {
            return Err("Worker physical exit unconfirmed; no replacement started".into());
        }
        let _ = output.join();
        return Err("Worker resume failed; physical process terminated".into());
    }
    let wait = unsafe { WaitForSingleObject(process_handle.0, timeout_ms) };
    if wait != WAIT_OBJECT_0 {
        job.terminate();
        let ended = unsafe { WaitForSingleObject(process_handle.0, 5000) };
        if ended != WAIT_OBJECT_0 {
            return Err("Worker physical exit unconfirmed; no replacement started".into());
        }
        let _ = output.join();
        return Err("Worker timed out or wait failed; physical process terminated".into());
    }
    let mut code = 0;
    ok(
        unsafe { GetExitCodeProcess(process_handle.0, &mut code) },
        "read Worker exit",
    )?;
    let stdout = output.join().map_err(|_| "Worker pipe reader failed")??;
    if stdout.len() > 8 * 1024 * 1024 {
        return Err("Worker output limit exceeded".into());
    }
    Ok(Reply {
        process_id: process.dwProcessId,
        exit_code: code,
        stdout,
    })
}
#[test]
fn command_arguments_quote_spaces_quotes_and_trailing_slashes() {
    assert_eq!(
        String::from_utf16(&quote(OsStr::new("a b"))).unwrap(),
        "\"a b\""
    );
    assert_eq!(
        String::from_utf16(&quote(OsStr::new("a\\"))).unwrap(),
        "\"a\\\\\""
    );
    assert_eq!(
        String::from_utf16(&quote(OsStr::new("a\"b"))).unwrap(),
        "\"a\\\"b\""
    );
}

#[test]
fn cache_asset_names_reject_windows_path_aliases() {
    for name in [
        "",
        ".",
        "..",
        "../outside",
        "a/b",
        r"a\b",
        "C:outside",
        "a:secret",
        "NUL.txt",
        "com1",
        "LPT9.txt",
        "trailing.",
        "a b",
        ".hidden",
    ] {
        assert!(asset_name(name).is_err(), "{name}");
    }
    for name in [
        "wasmtime.dll",
        "node.wasm",
        "request.json",
        "canary-worker.exe",
    ] {
        assert!(asset_name(name).is_ok(), "{name}");
    }
}

mod pipe;
#[cfg(target_arch = "x86_64")]
mod resource_transport;
#[cfg(target_arch = "x86_64")]
pub use pipe::{
    DriverPolicy, DriverStop, RetainedBinding, RetainedClient, RetainedInput, RetainedReservation,
    RetainedWorkerRequest, execute_retained,
};
#[cfg(target_arch = "x86_64")]
pub use pipe::{
    PreparationExit, PreparationOutcome, PreparationRunning, PreparationWorkerRequest,
    execute_preparation,
};
#[cfg(target_arch = "x86_64")]
pub use pipe::{ResourceWorkerRequest, execute_resources};
mod worker_trace;
pub use pipe::{EndReason, ExitReceipt, Outcome, RunControl, Running, execute};

#[cfg(target_arch = "x86_64")]
pub mod runtime;

#[cfg(target_arch = "x86_64")]
pub mod adapter;

#[cfg(target_arch = "x86_64")]
pub mod package;
