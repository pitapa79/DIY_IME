//! Read-only DLL verification without a private copy of the entire image.
use crate::isolated_worker::OwnedHandle;
use std::{
    fs::File,
    os::windows::{fs::OpenOptionsExt, io::AsRawHandle},
    path::Path,
};
use windows_sys::Win32::{
    Storage::FileSystem::FILE_SHARE_READ,
    System::Memory::{
        CreateFileMappingW, FILE_MAP_READ, MEMORY_MAPPED_VIEW_ADDRESS, MapViewOfFile,
        PAGE_READONLY, UnmapViewOfFile,
    },
};

// The pinned 50.0.1 image is about 21 MB. Refuse unbounded file mappings.
const MAX_RUNTIME_BYTES: u64 = 64 * 1024 * 1024;

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
