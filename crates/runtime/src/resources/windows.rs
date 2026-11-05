use super::*;
use std::{
    ffi::c_void,
    fs::OpenOptions,
    os::windows::{
        fs::{MetadataExt, OpenOptionsExt},
        io::AsRawHandle,
    },
    path::{Component, PathBuf, Prefix},
};
#[link(name = "kernel32")]
unsafe extern "system" {
    fn GetFinalPathNameByHandleW(
        handle: *mut c_void,
        path: *mut u16,
        length: u32,
        flags: u32,
    ) -> u32;
    fn GetFileInformationByHandleEx(
        handle: *mut c_void,
        class: i32,
        information: *mut c_void,
        length: u32,
    ) -> i32;
}
#[repr(C)]
#[derive(Default)]
struct FileId {
    volume: u64,
    id: [u8; 16],
}
fn final_path(file: &File) -> Result<PathBuf, CoreError> {
    let mut buffer = vec![0u16; 32768];
    // SAFETY: File owns a live handle; the buffer is writable for length UTF-16 units.
    let count = unsafe {
        GetFinalPathNameByHandleW(
            file.as_raw_handle(),
            buffer.as_mut_ptr(),
            buffer.len() as u32,
            1,
        )
    };
    if count == 0 || count as usize >= buffer.len() {
        return Err(failure("resource-path", "cannot resolve opened resource"));
    }
    let value = String::from_utf16(&buffer[..count as usize])
        .map_err(|_| failure("resource-path", "resource path is not valid Unicode"))?;
    // GUID form cannot represent SMB paths. Reject any unexpected provider/namespace.
    if !value.starts_with(r"\\?\Volume{") {
        return Err(failure(
            "resource-path",
            "only local volume resources are supported",
        ));
    }
    Ok(PathBuf::from(value))
}
fn identity(file: &File) -> Result<(u64, [u8; 16]), CoreError> {
    let mut info = FileId::default();
    // SAFETY: FILE_ID_INFO has repr(C), the documented u64 + 128-bit layout.
    // FileIdInfo = 18; output is exactly size_of::<FileId>() writable bytes.
    let ok = unsafe {
        GetFileInformationByHandleEx(
            file.as_raw_handle(),
            18,
            (&mut info as *mut FileId).cast(),
            std::mem::size_of::<FileId>() as u32,
        )
    };
    if ok == 0 || info.id == [0; 16] {
        return Err(failure("resource-path", "cannot identify opened resource"));
    }
    Ok((info.volume, info.id))
}
pub struct Root {
    path: PathBuf,
    _directory: File,
    volume: u64,
}
impl Root {
    pub fn open(path: &Path) -> Result<Self, CoreError> {
        if !path.is_absolute()
            || !matches!(path.components().next(),Some(Component::Prefix(prefix)) if matches!(prefix.kind(),Prefix::Disk(_)))
        {
            return Err(failure(
                "resource-config",
                "resource root must be an absolute local drive path",
            ));
        }
        // Keep the directory handle alive and deny delete/rename of the chosen root.
        let directory = OpenOptions::new()
            .read(true)
            .share_mode(3)
            .custom_flags(0x02000000)
            .open(path)
            .map_err(|_| failure("resource-config", "cannot open resource root"))?;
        if !directory
            .metadata()
            .map_err(|_| failure("resource-config", "cannot inspect resource root"))?
            .is_dir()
        {
            return Err(failure(
                "resource-config",
                "resource root is not a directory",
            ));
        }
        let final_name = final_path(&directory)?;
        let (volume, _) = identity(&directory)?;
        Ok(Self {
            path: final_name,
            _directory: directory,
            volume,
        })
    }
    pub fn file(&self, reference: &str) -> Result<OpenedResource, CoreError> {
        // Constrain the logical name before any OS open: no devices, ADS, traversal,
        // URL, Win32 trimming aliases, wildcards, or platform-dependent separators.
        if reference.is_empty()
            || reference.len() > 4096
            || reference.contains([':', '\\', '\0'])
            || reference.starts_with('/')
        {
            return Err(failure(
                "resource-path",
                "expected a root-relative resource name using /",
            ));
        }
        for part in reference.split('/') {
            let base = part.split('.').next().unwrap_or("").to_ascii_uppercase();
            if part.is_empty()
                || part == "."
                || part == ".."
                || part.ends_with(['.', ' '])
                || part
                    .chars()
                    .any(|c| c.is_control() || matches!(c, '<' | '>' | '"' | '|' | '?' | '*'))
                || matches!(
                    base.as_str(),
                    "CON" | "PRN" | "AUX" | "NUL" | "CONIN$" | "CONOUT$"
                )
                || ((base.starts_with("COM") || base.starts_with("LPT"))
                    && matches!(
                        base.get(3..),
                        Some("1" | "2" | "3" | "4" | "5" | "6" | "7" | "8" | "9" | "¹" | "²" | "³")
                    ))
            {
                return Err(failure("resource-path", "invalid resource path component"));
            }
        }
        // Open each directory without following reparse points, keep its handle,
        // and deny rename/delete until reading ends. No path re-open after checking.
        let mut ancestors = Vec::new();
        let mut current = self.path.clone();
        let parts: Vec<_> = reference.split('/').collect();
        for part in &parts[..parts.len() - 1] {
            current.push(part);
            let directory = OpenOptions::new()
                .read(true)
                .share_mode(3)
                .custom_flags(0x02000000 | 0x00200000)
                .open(&current)
                .map_err(|_| failure("resource-path", "cannot open resource directory"))?;
            let meta = directory
                .metadata()
                .map_err(|_| failure("resource-path", "cannot inspect resource directory"))?;
            if !meta.is_dir() || meta.file_attributes() & 0x400 != 0 {
                return Err(failure(
                    "resource-path",
                    "reparse directories are not supported",
                ));
            }
            ancestors.push(directory);
        }
        current.push(parts.last().expect("nonempty validated name"));
        // Deny existing/new ordinary writers, write mappings, delete and rename.
        let file = OpenOptions::new()
            .read(true)
            .share_mode(1)
            .custom_flags(0x00200000)
            .open(current)
            .map_err(|error| {
                failure(
                    if error.raw_os_error() == Some(32) {
                        "resource-busy"
                    } else {
                        "resource-read"
                    },
                    "cannot open resource for stable reading",
                )
            })?;
        let metadata = file
            .metadata()
            .map_err(|_| failure("resource-read", "cannot inspect resource"))?;
        if metadata.file_attributes() & 0x400 != 0 {
            return Err(failure("resource-path", "reparse files are not supported"));
        }
        let path = final_path(&file)?;
        // Compare full path components of the opened object, never a string prefix.
        // Case-sensitive comparison is conservative even on case-sensitive directories.
        if path == self.path || !path.starts_with(&self.path) {
            return Err(failure(
                "resource-path",
                "opened resource lies outside permitted root",
            ));
        }
        let key = identity(&file)?;
        if key.0 != self.volume {
            return Err(failure("resource-path", "resource is on another volume"));
        }
        let meta = file
            .metadata()
            .map_err(|_| failure("resource-read", "cannot inspect resource"))?;
        if !meta.is_file() || meta.file_attributes() & (0x1000 | 0x40000 | 0x400000) != 0 {
            return Err(failure(
                "resource-path",
                "expected a local resident regular file",
            ));
        }
        Ok(OpenedResource {
            file,
            key,
            ancestors,
        })
    }
}
