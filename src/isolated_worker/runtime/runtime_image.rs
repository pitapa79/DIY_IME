//! Read-only DLL verification without a private copy of the entire image.
use crate::isolated_worker::OwnedHandle;
use std::{
    fs::File,
    os::windows::{fs::OpenOptionsExt, io::AsRawHandle},
    path::Path,
};
use windows_sys::Win32::{
    Storage::FileSystem::{
        FILE_ID_INFO, FILE_SHARE_READ, FILE_TYPE_DISK, FileIdInfo, GetFileInformationByHandleEx,
        GetFileType,
    },
    System::Memory::{
        CreateFileMappingW, FILE_MAP_READ, MEMORY_MAPPED_VIEW_ADDRESS, MapViewOfFile,
        PAGE_READONLY, UnmapViewOfFile,
    },
};

// The pinned 50.0.1 image is about 21 MB. Refuse unbounded file mappings.
const MAX_RUNTIME_BYTES: u64 = 64 * 1024 * 1024;

fn unnamed_absolute_file(path: &Path) -> bool {
    use std::os::windows::ffi::OsStrExt;
    path.is_absolute()
        && path
            .file_name()
            .is_some_and(|name| !name.encode_wide().any(|c| c == b':' as u16))
}

/// Constructed only by the trusted Host after hashing the actual locked file.
/// The private file, unlike a hash string supplied by a package, is a capability
/// whose read-only/deny-write/delete ownership lasts through every child exit.
pub(in crate::isolated_worker) struct VerifiedRuntimeFile(File);
impl VerifiedRuntimeFile {
    pub(in crate::isolated_worker) fn open(path: &Path) -> Result<Self, String> {
        if !unnamed_absolute_file(path) {
            return Err("runtime proof requires an absolute unnamed file stream".into());
        }
        let image = ReadOnlyImage::open(path)?;
        super::verify_runtime_bytes(image.bytes())?;
        let file = image
            ._file
            .try_clone()
            .map_err(|_| "runtime file pin failed")?;
        Ok(Self(file))
    }

    pub(in crate::isolated_worker) fn inherit(&self) -> Result<OwnedHandle, String> {
        use windows_sys::Win32::{
            Foundation::{DUPLICATE_SAME_ACCESS, DuplicateHandle},
            System::Threading::GetCurrentProcess,
        };
        let mut handle = std::ptr::null_mut();
        // SAFETY: duplicate only this verified read-only file, without widening
        // its access. The launch's explicit handle list contains the duplicate.
        super::super::ok(
            unsafe {
                DuplicateHandle(
                    GetCurrentProcess(),
                    self.0.as_raw_handle(),
                    GetCurrentProcess(),
                    &mut handle,
                    0,
                    1,
                    DUPLICATE_SAME_ACCESS,
                )
            },
            "duplicate verified runtime file",
        )?;
        OwnedHandle::new(handle, "verified runtime file")
    }
}

fn identity(file: &File) -> Result<(u64, [u8; 16], u64), String> {
    let handle = file.as_raw_handle();
    let mut info: FILE_ID_INFO = unsafe { std::mem::zeroed() };
    // No pipe/device queries or path traversal. Unsupported file identity is
    // a refusal, not permission to trust a package's claimed hash/path/length.
    if unsafe { GetFileType(handle) } != FILE_TYPE_DISK
        || unsafe {
            GetFileInformationByHandleEx(
                handle,
                FileIdInfo,
                std::ptr::from_mut(&mut info).cast(),
                std::mem::size_of::<FILE_ID_INFO>() as u32,
            )
        } == 0
    {
        return Err("runtime file identity unavailable".into());
    }
    let metadata = file
        .metadata()
        .map_err(|_| "runtime file metadata unavailable")?;
    if !metadata.is_file()
        || !(1..=MAX_RUNTIME_BYTES).contains(&metadata.len())
        || info.FileId.Identifier == [0; 16]
    {
        return Err("runtime proof is not a bounded regular file".into());
    }
    Ok((
        info.VolumeSerialNumber,
        info.FileId.Identifier,
        metadata.len(),
    ))
}

/// The caller must already trust the Parent's provenance of `verified`.
/// Identity alone never proves that arbitrary caller-supplied bytes are a DLL.
pub(super) fn lock_matching_file(path: &Path, verified: &File) -> Result<File, String> {
    if !unnamed_absolute_file(path) {
        // NTFS streams share a file ID. The capability and DLL loader must
        // refer to the same unnamed stream, not just the same base file.
        return Err("runtime proof path must be absolute and use an unnamed file stream".into());
    }
    let expected = identity(verified)?;
    let file = File::options()
        .read(true)
        .share_mode(FILE_SHARE_READ)
        .open(path)
        .map_err(|_| "runtime proof path cannot be locked read-only")?;
    if identity(&file)? != expected {
        return Err("runtime proof refers to a different file".into());
    }
    Ok(file)
}

pub(super) struct ReadOnlyImage {
    view: MEMORY_MAPPED_VIEW_ADDRESS,
    _mapping: OwnedHandle,
    _file: File,
    length: usize,
}
impl ReadOnlyImage {
    pub(super) fn open(path: &Path) -> Result<Self, String> {
        let file = File::options()
            .read(true)
            .share_mode(FILE_SHARE_READ)
            .open(path)
            .map_err(|_| "runtime read-only file unavailable")?;
        let length = file
            .metadata()
            .map_err(|_| "runtime metadata unavailable")?
            .len();
        if !(1..=MAX_RUNTIME_BYTES).contains(&length) {
            return Err("runtime image exceeds verification bound".into());
        }
        // No name, inherited handle, writable view or executable mapping.
        // The file handle excludes new writes/deletes and remains owned until
        // the view has been unmapped and the mapping handle closed.
        let mapping = OwnedHandle::new(
            unsafe {
                CreateFileMappingW(
                    file.as_raw_handle(),
                    std::ptr::null(),
                    PAGE_READONLY,
                    0,
                    0,
                    std::ptr::null(),
                )
            },
            "runtime read-only mapping",
        )?;
        let length = usize::try_from(length).map_err(|_| "runtime image length overflow")?;
        let view = unsafe { MapViewOfFile(mapping.0, FILE_MAP_READ, 0, 0, length) };
        if view.Value.is_null() {
            return Err("runtime read-only view unavailable".into());
        }
        Ok(Self {
            view,
            _mapping: mapping,
            _file: file,
            length,
        })
    }
    pub(super) fn bytes(&self) -> &[u8] {
        // SAFETY: the complete nonempty file view is read-only and cannot
        // outlive this owner. Its original file and mapping remain open.
        unsafe { std::slice::from_raw_parts(self.view.Value.cast(), self.length) }
    }
}
impl Drop for ReadOnlyImage {
    fn drop(&mut self) {
        // SAFETY: this is the view returned by MapViewOfFile for this owner.
        unsafe {
            UnmapViewOfFile(self.view);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn proof_refuses_unpinned_bytes_and_matching_refuses_foreign_mutable_or_pipe_files() {
        use std::os::windows::io::FromRawHandle;
        let root =
            std::env::temp_dir().join(format!("diyime-runtime-proof-{}", std::process::id()));
        std::fs::create_dir(&root).unwrap();
        let path = root.join("runtime.dll");
        let foreign = root.join("foreign.dll");
        std::fs::write(
            &path,
            b"same contents do not establish compiler/runtime provenance",
        )
        .unwrap();
        std::fs::copy(&path, &foreign).unwrap();
        assert!(VerifiedRuntimeFile::open(&path).is_err());
        let file = File::options()
            .read(true)
            .share_mode(FILE_SHARE_READ)
            .open(&path)
            .unwrap();
        assert!(lock_matching_file(&foreign, &file).is_err());
        assert!(lock_matching_file(Path::new("runtime.dll"), &file).is_err());
        let alternate_stream = std::path::PathBuf::from(format!("{}:alternate", path.display()));
        assert!(
            lock_matching_file(&alternate_stream, &file)
                .unwrap_err()
                .contains("unnamed")
        );
        assert!(VerifiedRuntimeFile::open(&alternate_stream).is_err());
        let matched = lock_matching_file(&path, &file).unwrap();
        assert!(File::options().write(true).open(&path).is_err());
        assert!(std::fs::rename(&path, root.join("replaced.dll")).is_err());
        drop(matched);
        drop(file);
        let writable = File::options()
            .read(true)
            .write(true)
            .share_mode(FILE_SHARE_READ | windows_sys::Win32::Storage::FileSystem::FILE_SHARE_WRITE)
            .open(&path)
            .unwrap();
        assert!(lock_matching_file(&path, &writable).is_err());
        drop(writable);
        let mut read = std::ptr::null_mut();
        let mut write = std::ptr::null_mut();
        assert_ne!(
            unsafe {
                windows_sys::Win32::System::Pipes::CreatePipe(
                    &mut read,
                    &mut write,
                    std::ptr::null(),
                    0,
                )
            },
            0
        );
        let read = unsafe { File::from_raw_handle(read) };
        let write = OwnedHandle::new(write, "test pipe").unwrap();
        assert!(lock_matching_file(&path, &read).is_err());
        drop(read);
        drop(write);
        std::fs::remove_file(path).unwrap();
        std::fs::remove_file(foreign).unwrap();
        std::fs::remove_dir(root).unwrap();
    }

    #[test]
    #[ignore = "requires the pinned production DLL"]
    fn real_verified_runtime_file_pins_full_hash_identity_and_readonly_inheritance() {
        use std::{
            io::{Seek, Write},
            os::windows::io::FromRawHandle,
        };
        let source = std::env::var_os("DIY_RANGE_RUNTIME_DLL").unwrap();
        let root = std::env::temp_dir().join(format!(
            "diyime-pinned-runtime-proof-{}",
            std::process::id()
        ));
        std::fs::create_dir(&root).unwrap();
        let path = root.join("runtime.dll");
        std::fs::copy(source, &path).unwrap();
        let verified = VerifiedRuntimeFile::open(&path).unwrap();
        let inherited = verified.inherit().unwrap();
        let mut flags = 0;
        assert_ne!(
            unsafe {
                windows_sys::Win32::Foundation::GetHandleInformation(inherited.0, &mut flags)
            },
            0
        );
        assert_ne!(
            flags & windows_sys::Win32::Foundation::HANDLE_FLAG_INHERIT,
            0
        );
        let mut file = unsafe { File::from_raw_handle(inherited.0) };
        std::mem::forget(inherited);
        assert!(file.write_all(b"rejected write").is_err());
        let matched = lock_matching_file(&path, &file).unwrap();
        // Parent capability can be released only after the child inherited its
        // own pin. The child guard still prevents writes and path replacement.
        drop(verified);
        assert!(File::options().write(true).open(&path).is_err());
        assert!(std::fs::remove_file(&path).is_err());
        drop(matched);
        drop(file);
        let mut changed = File::options().write(true).open(&path).unwrap();
        changed.seek(std::io::SeekFrom::Start(0)).unwrap();
        changed.write_all(b"bad runtime").unwrap();
        drop(changed);
        assert!(VerifiedRuntimeFile::open(&path).is_err());
        std::fs::remove_file(path).unwrap();
        std::fs::remove_dir(root).unwrap();
    }

    #[test]
    fn mapping_pins_the_same_read_only_bytes_until_all_owners_close() {
        let root = std::env::temp_dir().join(format!("diyime-runtime-map-{}", std::process::id()));
        std::fs::create_dir(&root).unwrap();
        let path = root.join("runtime.dll");
        std::fs::write(&path, b"immutable runtime image").unwrap();
        let first = ReadOnlyImage::open(&path).unwrap();
        let second = ReadOnlyImage::open(&path).unwrap();
        assert_eq!(first.bytes(), b"immutable runtime image");
        assert_eq!(second.bytes(), first.bytes());
        assert!(File::options().write(true).open(&path).is_err());
        assert!(std::fs::rename(&path, root.join("replacement.dll")).is_err());
        drop(first);
        assert!(File::options().write(true).open(&path).is_err());
        drop(second);
        let writer = File::options().write(true).open(&path).unwrap();
        assert!(ReadOnlyImage::open(&path).is_err());
        drop(writer);
        std::fs::write(&path, b"changed image").unwrap();
        let changed = ReadOnlyImage::open(&path).unwrap();
        assert_eq!(changed.bytes(), b"changed image");
        assert!(super::super::verify_runtime_bytes(changed.bytes()).is_err());
        drop(changed);
        std::fs::write(&path, b"").unwrap();
        assert!(ReadOnlyImage::open(&path).is_err());
        std::fs::remove_file(&path).unwrap();
        std::fs::remove_dir(&root).unwrap();
    }
}
