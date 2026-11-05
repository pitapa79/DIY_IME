//! Test-only physical child observation and exclusive dictionary sharing fixture.
#[cfg(all(windows, target_arch = "x86_64"))]
mod probe {
    use std::{
        env,
        fs::OpenOptions,
        io::{self, Read, Write},
        os::windows::fs::OpenOptionsExt,
    };
    use windows_sys::Win32::System::Threading::{
        OpenProcess, PROCESS_SYNCHRONIZE, WaitForSingleObject,
    };
    use windows_sys::Win32::{
        Foundation::{CloseHandle, INVALID_HANDLE_VALUE, WAIT_OBJECT_0, WAIT_TIMEOUT},
        System::Diagnostics::ToolHelp::{
            CreateToolhelp32Snapshot, PROCESSENTRY32W, Process32FirstW, Process32NextW,
            TH32CS_SNAPPROCESS,
        },
    };
    fn live(pid: u32) -> io::Result<bool> {
        let process = unsafe { OpenProcess(PROCESS_SYNCHRONIZE, 0, pid) };
        if process.is_null() {
            let error = io::Error::last_os_error();
            return if error.raw_os_error() == Some(87) {
                Ok(false)
            } else {
                Err(error)
            };
        }
        let state = unsafe { WaitForSingleObject(process, 0) };
        let error = io::Error::last_os_error();
        unsafe {
            CloseHandle(process);
        }
        match state {
            WAIT_OBJECT_0 => Ok(false),
            WAIT_TIMEOUT => Ok(true),
            _ => Err(error),
        }
    }
    fn children(parent: u32) -> io::Result<Vec<u32>> {
        let snapshot = unsafe { CreateToolhelp32Snapshot(TH32CS_SNAPPROCESS, 0) };
        if snapshot == INVALID_HANDLE_VALUE {
            return Err(io::Error::last_os_error());
        }
        let mut entry: PROCESSENTRY32W = unsafe { std::mem::zeroed() };
        entry.dwSize = std::mem::size_of::<PROCESSENTRY32W>() as u32;
        let mut found = Vec::new();
        let mut valid = unsafe { Process32FirstW(snapshot, &mut entry) };
        if valid == 0 {
            let error = io::Error::last_os_error();
            unsafe {
                CloseHandle(snapshot);
            }
            return Err(error);
        }
        while valid != 0 {
            // Host console helpers are not guest processes. Observe the actual
            // staged Worker, including its suspended pre-verification lifetime.
            let name = String::from_utf16_lossy(
                &entry.szExeFile[..entry
                    .szExeFile
                    .iter()
                    .position(|c| *c == 0)
                    .unwrap_or(entry.szExeFile.len())],
            );
            if entry.th32ParentProcessID == parent && name.eq_ignore_ascii_case("worker.exe") {
                match live(entry.th32ProcessID) {
                    Ok(true) => found.push(entry.th32ProcessID),
                    Ok(false) => (),
                    Err(error) => {
                        unsafe {
                            CloseHandle(snapshot);
                        }
                        return Err(error);
                    }
                }
            }
            valid = unsafe { Process32NextW(snapshot, &mut entry) };
        }
        let end = io::Error::last_os_error();
        unsafe {
            CloseHandle(snapshot);
        }
        if end.raw_os_error() != Some(18) {
            return Err(end);
        }
        Ok(found)
    }
    pub fn run() -> Result<(), Box<dyn std::error::Error>> {
        let args: Vec<_> = env::args().skip(1).collect();
        match args.as_slice() {
            [method, parent] if method == "children" => {
                println!("{}", serde_json::to_string(&children(parent.parse()?)?)?);
            }
            [method, path] if method == "lock" => {
                let _file = OpenOptions::new().read(true).share_mode(0).open(path)?;
                println!("locked");
                io::stdout().flush()?;
                let mut byte = [0];
                let _ = io::stdin().read(&mut byte)?;
            }
            _ => return Err("expected children <owned-parent-pid> or lock <test-resource>".into()),
        }
        Ok(())
    }
}
fn main() {
    #[cfg(all(windows, target_arch = "x86_64"))]
    if let Err(error) = probe::run() {
        eprintln!("{error}");
        std::process::exit(1);
    }
    #[cfg(not(all(windows, target_arch = "x86_64")))]
    {
        eprintln!("Windows x64 required");
        std::process::exit(1);
    }
}
