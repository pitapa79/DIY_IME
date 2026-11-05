#![cfg_attr(windows, windows_subsystem = "windows")]

#[cfg(all(windows, target_arch = "x86_64"))]
mod application {
    use serde::Deserialize;
    use std::{
        ffi::OsString,
        io::{BufRead, BufReader, Read, Write},
        os::windows::{ffi::OsStrExt, io::AsRawHandle, process::CommandExt},
        path::{Path, PathBuf},
        process::{Child, Command, Stdio},
        time::Duration,
    };
    use windows_sys::Win32::{
        Foundation::{CloseHandle, HANDLE},
        System::JobObjects::{
            AssignProcessToJobObject, CreateJobObjectW, JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE,
            JOBOBJECT_BASIC_ACCOUNTING_INFORMATION, JOBOBJECT_EXTENDED_LIMIT_INFORMATION,
            JobObjectBasicAccountingInformation, JobObjectExtendedLimitInformation,
            QueryInformationJobObject, SetInformationJobObject, TerminateJobObject,
        },
    };
    const NO_WINDOW: u32 = 0x08000000;
    struct ApplicationJob(HANDLE);
    impl ApplicationJob {
        fn new() -> Result<Self, ()> {
            let handle = unsafe { CreateJobObjectW(std::ptr::null(), std::ptr::null()) };
            if handle.is_null() {
                return Err(());
            }
            let job = Self(handle);
            let mut info: JOBOBJECT_EXTENDED_LIMIT_INFORMATION = unsafe { std::mem::zeroed() };
            info.BasicLimitInformation.LimitFlags = JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE;
            if unsafe {
                SetInformationJobObject(
                    job.0,
                    JobObjectExtendedLimitInformation,
                    (&info as *const JOBOBJECT_EXTENDED_LIMIT_INFORMATION).cast(),
                    std::mem::size_of_val(&info) as u32,
                )
            } == 0
            {
                return Err(());
            }
            Ok(job)
        }
        fn assign(&self, child: &Child) -> Result<(), ()> {
            if unsafe { AssignProcessToJobObject(self.0, child.as_raw_handle()) } == 0 {
                Err(())
            } else {
                Ok(())
            }
        }
    }
    impl Drop for ApplicationJob {
        fn drop(&mut self) {
            // Fail closed on unexpected application/launcher exit. The inner Worker
            // keeps its own AppContainer/Job policy; this is application ownership.
            loop {
                unsafe {
                    TerminateJobObject(self.0, 1);
                }
                let mut info: JOBOBJECT_BASIC_ACCOUNTING_INFORMATION =
                    unsafe { std::mem::zeroed() };
                if unsafe {
                    QueryInformationJobObject(
                        self.0,
                        JobObjectBasicAccountingInformation,
                        (&mut info as *mut JOBOBJECT_BASIC_ACCOUNTING_INFORMATION).cast(),
                        std::mem::size_of_val(&info) as u32,
                        std::ptr::null_mut(),
                    )
                } != 0
                    && info.ActiveProcesses == 0
                {
                    break;
                }
                std::thread::sleep(Duration::from_millis(10));
            }
            unsafe {
                CloseHandle(self.0);
            }
        }
    }
    #[derive(Deserialize)]
    #[serde(deny_unknown_fields)]
    struct Ready {
        url: String,
    }
    fn local_url(url: &str) -> bool {
        url.strip_prefix("http://127.0.0.1:")
            .and_then(|s| s.strip_suffix('/'))
            .is_some_and(|port| {
                !port.is_empty()
                    && port.bytes().all(|c| c.is_ascii_digit())
                    && port.parse::<u16>().is_ok_and(|p| p != 0)
            })
    }
    fn browser(url: &str) -> Result<(), ()> {
        if !local_url(url) {
            return Err(());
        }
        let system = std::env::var_os("SystemRoot").ok_or(())?;
        Command::new(Path::new(&system).join("System32/rundll32.exe"))
            .args(["url.dll,FileProtocolHandler", url])
            .creation_flags(NO_WINDOW)
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
            .map(|_| ())
            .map_err(|_| ())
    }
    pub fn run() -> Result<(), ()> {
        let executable = std::env::current_exe().map_err(|_| ())?;
        let root = executable.parent().ok_or(())?;
        let mut data = PathBuf::from(std::env::var_os("LOCALAPPDATA").ok_or(())?).join("DIY_IME2");
        let mut no_browser = false;
        let mut port = "5175".to_string();
        let mut args = std::env::args_os().skip(1);
        while let Some(arg) = args.next() {
            match arg.to_str() {
                Some("--no-browser") => no_browser = true,
                Some("--data-root") => data = PathBuf::from(args.next().ok_or(())?),
                Some("--port") => {
                    port = args.next().and_then(|s| s.into_string().ok()).ok_or(())?;
                    if port.parse::<u16>().is_err() {
                        return Err(());
                    }
                }
                _ => return Err(()),
            }
        }
        if !data.is_absolute() {
            return Err(());
        }
        let job = ApplicationJob::new()?;
        let mut child = Command::new(root.join("runtime/node.exe"))
            .arg(root.join("runtime/server.mjs"))
            .arg(root)
            .arg(data)
            .arg("--launcher")
            .arg(port)
            .current_dir(root)
            .env_remove("NODE_OPTIONS")
            .env_remove("NODE_PATH")
            .creation_flags(NO_WINDOW)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .spawn()
            .map_err(|_| ())?;
        if job.assign(&child).is_err() {
            let _ = child.kill();
            let _ = child.wait();
            return Err(());
        }
        // The trusted server does not initialize/spawn a Host before this handshake,
        // so all application descendants belong to the job before they can start.
        let mut input = child.stdin.take().ok_or(())?;
        input.write_all(b"START\n").map_err(|_| ())?;
        let mut line = String::new();
        BufReader::new(child.stdout.take().ok_or(())?)
            .take(4097)
            .read_line(&mut line)
            .map_err(|_| ())?;
        if line.len() > 4096 {
            return Err(());
        }
        let ready: Ready = serde_json::from_str(&line).map_err(|_| ())?;
        if !local_url(&ready.url) {
            return Err(());
        }
        if no_browser {
            println!("{}", line.trim());
        } else {
            browser(&ready.url)?;
        }
        let status = child.wait().map_err(|_| ())?;
        drop(input);
        if status.success() { Ok(()) } else { Err(()) }
    }
    pub fn failure() {
        #[link(name = "user32")]
        unsafe extern "system" {
            fn MessageBoxW(
                window: HANDLE,
                text: *const u16,
                caption: *const u16,
                flags: u32,
            ) -> i32;
        }
        let text: Vec<u16> = OsString::from("アプリを起動・終了できませんでした。すでに開いている画面がないか確認し、もう一度起動してください。").encode_wide().chain(Some(0)).collect();
        let title: Vec<u16> = OsString::from("DIY_IME2")
            .encode_wide()
            .chain(Some(0))
            .collect();
        unsafe {
            MessageBoxW(std::ptr::null_mut(), text.as_ptr(), title.as_ptr(), 0x10);
        }
    }
    #[cfg(test)]
    mod tests {
        use super::*;
        #[test]
        fn browser_target_is_only_an_exact_local_application_url() {
            assert!(local_url("http://127.0.0.1:5175/"));
            for url in [
                "https://127.0.0.1:5175/",
                "http://localhost:5175/",
                "http://127.0.0.1:0/",
                "http://127.0.0.1:65536/",
                "http://127.0.0.1:5175/path",
                "http://127.0.0.1:5175/?run=1",
                "http://127.0.0.1:5175/;start",
            ] {
                assert!(!local_url(url));
            }
        }
    }
}
#[cfg(all(windows, target_arch = "x86_64"))]
fn main() {
    if application::run().is_err() {
        if std::env::args_os().any(|a| a == "--no-browser") {
            eprintln!("Application startup or shutdown failed");
        } else {
            application::failure();
        }
        std::process::exit(1);
    }
}
#[cfg(not(all(windows, target_arch = "x86_64")))]
fn main() {
    eprintln!("Windows x64 required");
    std::process::exit(1);
}
