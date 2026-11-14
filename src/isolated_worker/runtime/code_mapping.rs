//! Private immutable code transport, not authority to load package artifacts.
use std::{
    fs::File,
    os::windows::io::{AsRawHandle, BorrowedHandle, FromRawHandle, OwnedHandle},
};
use windows_sys::Win32::{
    Foundation::DuplicateHandle,
    System::{
        Memory::{
            CreateFileMappingW, FILE_MAP_READ, MEMORY_MAPPED_VIEW_ADDRESS, MapViewOfFile,
            PAGE_READONLY, UnmapViewOfFile,
        },
        Threading::GetCurrentProcess,
    },
};
const MAX_CODE: usize = 8 * 1024 * 1024;

pub(in crate::isolated_worker) struct CodeView {
    view: MEMORY_MAPPED_VIEW_ADDRESS,
    length: usize,
}
impl CodeView {
    /// # Safety
    /// Only the trusted Parent's late immutable-code capability may supply this
    /// value. Native process lifetime owns the remote handle, even on errors.
    pub(super) unsafe fn from_parent_handle(value: u64, length: usize) -> Result<Self, String> {
        let value = usize::try_from(value).map_err(|_| "code handle overflow")?;
        let raw = value as *mut std::ffi::c_void;
        let mut flags = 0;
        if value == 0
            || value == usize::MAX
            || unsafe { windows_sys::Win32::Foundation::GetHandleInformation(raw, &mut flags) } == 0
        {
            return Err("invalid late code handle".into());
        }
        let handle = unsafe { BorrowedHandle::borrow_raw(raw) };
        unsafe { Self::map_locked(handle, length) }
    }
    /// # Safety
    /// The mapping must be read-only and its backing bytes continuously immutable
    /// for this view's lifetime. A handle/size/hash from a package is NOT proof.
    pub(in crate::isolated_worker) unsafe fn map_locked(
        handle: BorrowedHandle<'_>,
        length: usize,
    ) -> Result<Self, String> {
        if !(1..=MAX_CODE).contains(&length) {
            return Err("mapped code length exceeds bound".into());
        }
        let view = unsafe { MapViewOfFile(handle.as_raw_handle(), FILE_MAP_READ, 0, 0, length) };
        if view.Value.is_null() {
            return Err("read-only code view unavailable".into());
        }
        Ok(Self { view, length })
    }
    pub(in crate::isolated_worker) fn bytes(&self) -> &[u8] {
        // SAFETY: constructor's immutable backing contract; view owns its kernel
        // reference and remains mapped until after the Module/Store is dropped.
        unsafe { std::slice::from_raw_parts(self.view.Value.cast(), self.length) }
    }
}
impl Drop for CodeView {
    fn drop(&mut self) {
        unsafe {
            UnmapViewOfFile(self.view);
        }
    }
}

pub(in crate::isolated_worker) struct CodeMapping {
    view: CodeView,
    mapping: OwnedHandle,
    _file: File,
    digest: String,
}
// SAFETY: only the private locked-file constructor creates this owner. The
// maximum section protection is PAGE_READONLY, no writable view is ever made,
// and the immutable file pin outlives its view and every borrowed slice.
unsafe impl Send for CodeMapping {}
unsafe impl Sync for CodeMapping {}
impl CodeMapping {
    /// # Safety
    /// `file` must be the private immutable code snapshot, opened read-only with
    /// deny-write/delete sharing and never previously exposed for mutation.
    /// This checks bytes, NOT compiler provenance. LateJitCode separately proves
    /// its own settled compiler/source/policy before any handoff.
    pub(in crate::isolated_worker) unsafe fn capture_locked(
        file: &File,
        length: usize,
        digest: &str,
    ) -> Result<Self, String> {
        let file = file.try_clone().map_err(|_| "code file pin failed")?;
        let metadata = file
            .metadata()
            .map_err(|_| "code file metadata unavailable")?;
        if !metadata.is_file()
            || metadata.len() != length as u64
            || !(1..=MAX_CODE).contains(&length)
        {
            return Err("mapped code file length changed".into());
        }
        let handle = unsafe {
            CreateFileMappingW(
                file.as_raw_handle(),
                std::ptr::null(),
                PAGE_READONLY,
                0,
                0,
                std::ptr::null(),
            )
        };
        if handle.is_null() {
            return Err("read-only code mapping unavailable".into());
        }
        let mapping = unsafe { OwnedHandle::from_raw_handle(handle) };
        let view = unsafe { CodeView::map_locked(mapping.as_handle(), length)? };
        let actual = crate::isolated_worker::adapter::sha256(view.bytes())
            .map_err(|_| "mapped code digest unavailable")?;
        if actual != digest {
            return Err("mapped code bytes changed".into());
        }
        Ok(Self {
            view,
            mapping,
            _file: file,
            digest: digest.to_owned(),
        })
    }
    pub(in crate::isolated_worker) fn length(&self) -> usize {
        self.view.length
    }
    pub(in crate::isolated_worker) fn matches(&self, length: usize, digest: &str) -> bool {
        self.length() == length && self.digest == digest
    }
    pub(in crate::isolated_worker) fn duplicate_into(
        &self,
        process: BorrowedHandle<'_>,
    ) -> Result<u64, String> {
        let mut remote = std::ptr::null_mut();
        // Only the original, already sandboxed process handle is supplied by
        // Running's IO owner. No PID lookup, inheritance, write or execute right.
        if unsafe {
            DuplicateHandle(
                GetCurrentProcess(),
                self.mapping.as_raw_handle(),
                process.as_raw_handle(),
                &mut remote,
                FILE_MAP_READ,
                0,
                0,
            )
        } == 0
        {
            return Err("late read-only code handoff failed".into());
        }
        // Target owns this handle; its actual process exit closes it even when
        // the subsequent pipe write is cancelled/truncated. Parent retains the
        // mapping/file/budget until Running has joined that original process/IO.
        Ok(remote as usize as u64)
    }
}
use std::os::windows::io::AsHandle;

#[cfg(test)]
mod tests {
    use super::*;
    use std::os::windows::fs::OpenOptionsExt;
    use windows_sys::Win32::{
        Storage::FileSystem::FILE_SHARE_READ, System::Memory::FILE_MAP_WRITE,
    };
    #[test]
    fn immutable_code_mapping_checks_bytes_and_never_grants_a_writable_view() {
        let root = std::env::temp_dir().join(format!(
            "diyime-code-map-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        std::fs::create_dir(&root).unwrap();
        let path = root.join("synthetic-not-deserialized");
        std::fs::write(&path, b"public-code").unwrap();
        let file = File::options()
            .read(true)
            .share_mode(FILE_SHARE_READ)
            .open(&path)
            .unwrap();
        let digest = crate::isolated_worker::adapter::sha256(b"public-code").unwrap();
        // Private synthetic bytes test transport/integrity only, never loading.
        let mapping = unsafe { CodeMapping::capture_locked(&file, 11, &digest).unwrap() };
        assert!(mapping.matches(11, &digest));
        assert!(!mapping.matches(10, &digest));
        assert!(!mapping.matches(11, &"0".repeat(64)));
        assert!(unsafe { CodeView::from_parent_handle(0, 11) }.is_err());
        assert!(unsafe { CodeView::from_parent_handle(u64::MAX, 11) }.is_err());
        assert!(
            unsafe { CodeView::from_parent_handle(file.as_raw_handle() as usize as u64, 11) }
                .is_err()
        );
        assert_eq!(
            file.metadata().unwrap().len(),
            11,
            "invalid capability closed an unrelated handle"
        );
        assert!(unsafe { CodeMapping::capture_locked(&file, 11, &"0".repeat(64)) }.is_err());
        assert!(unsafe { CodeMapping::capture_locked(&file, 10, &digest) }.is_err());
        assert!(File::options().write(true).open(&path).is_err());
        let process = unsafe { BorrowedHandle::borrow_raw(GetCurrentProcess()) };
        let raw = mapping.duplicate_into(process).unwrap() as usize as *mut std::ffi::c_void;
        let duplicate = unsafe { OwnedHandle::from_raw_handle(raw) };
        let borrowed = unsafe { CodeView::map_locked(duplicate.as_handle(), 11).unwrap() };
        assert_eq!(borrowed.bytes(), b"public-code");
        assert!(
            unsafe { MapViewOfFile(duplicate.as_raw_handle(), FILE_MAP_WRITE, 0, 0, 11) }
                .Value
                .is_null()
        );
        assert!(
            unsafe { MapViewOfFile(mapping.mapping.as_raw_handle(), FILE_MAP_WRITE, 0, 0, 11) }
                .Value
                .is_null()
        );
        assert!(unsafe { CodeView::map_locked(duplicate.as_handle(), MAX_CODE + 1) }.is_err());
        drop(mapping);
        drop(duplicate);
        assert!(
            File::options().write(true).open(&path).is_err(),
            "original file pin was not retained"
        );
        drop(borrowed);
        drop(file);
        std::fs::remove_file(&path).unwrap();
        std::fs::remove_dir(&root).unwrap();
    }
}
