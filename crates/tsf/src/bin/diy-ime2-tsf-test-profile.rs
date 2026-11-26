#[cfg(all(windows, target_arch = "x86_64"))]
mod windows_probe {
    use diy_ime2_tsf::{JAPANESE_LANGID, TEST_PROFILE_GUID, TEST_SERVICE_CLSID};
    use windows::Win32::{
        Foundation::S_OK,
        System::Com::{
            CLSCTX_INPROC_SERVER, COINIT_APARTMENTTHREADED, CoCreateInstance, CoInitializeEx,
            CoUninitialize,
        },
        UI::{
            Input::KeyboardAndMouse::HKL,
            TextServices::{
                CLSID_TF_CategoryMgr, CLSID_TF_InputProcessorProfiles,
                GUID_TFCAT_DISPLAYATTRIBUTEPROVIDER, GUID_TFCAT_TIP_KEYBOARD, ITfCategoryMgr,
                ITfInputProcessorProfileMgr, ITfInputProcessorProfiles, TF_INPUTPROCESSORPROFILE,
                TF_PROFILETYPE_INPUTPROCESSOR,
            },
        },
    };
    use windows_core::{Interface, Result};

    struct Apartment;

    impl Apartment {
        fn initialize() -> Result<Self> {
            unsafe { CoInitializeEx(None, COINIT_APARTMENTTHREADED).ok()? };
            Ok(Self)
        }
    }

    impl Drop for Apartment {
        fn drop(&mut self) {
            unsafe { CoUninitialize() };
        }
    }

    fn registered(profiles: &ITfInputProcessorProfileMgr) -> bool {
        let mut info = TF_INPUTPROCESSORPROFILE::default();
        let result = unsafe {
            (profiles.vtable().GetProfile)(
                profiles.as_raw(),
                TF_PROFILETYPE_INPUTPROCESSOR,
                JAPANESE_LANGID,
                &TEST_SERVICE_CLSID,
                &TEST_PROFILE_GUID,
                HKL::default(),
                &mut info,
            )
        };
        result == S_OK && info.clsid == TEST_SERVICE_CLSID && info.guidProfile == TEST_PROFILE_GUID
    }

    fn check(result: Result<()>, operation: &str) -> std::result::Result<(), String> {
        result.map_err(|error| format!("{operation}: {error}"))
    }

    pub fn run(command: &str) -> std::result::Result<(), String> {
        let _apartment = Apartment::initialize().map_err(|error| error.to_string())?;
        let profiles: ITfInputProcessorProfileMgr = unsafe {
            CoCreateInstance(&CLSID_TF_InputProcessorProfiles, None, CLSCTX_INPROC_SERVER)
        }
        .map_err(|error| error.to_string())?;
        let user: ITfInputProcessorProfiles = profiles.cast().map_err(|error| error.to_string())?;
        match command {
            "register" => {
                if registered(&profiles) {
                    return Err("test profile already exists".into());
                }
                let category: ITfCategoryMgr =
                    unsafe { CoCreateInstance(&CLSID_TF_CategoryMgr, None, CLSCTX_INPROC_SERVER) }
                        .map_err(|error| error.to_string())?;
                let description: Vec<_> = "DIY IME Stage A (test)".encode_utf16().collect();
                check(
                    unsafe {
                        profiles.RegisterProfile(
                            &TEST_SERVICE_CLSID,
                            JAPANESE_LANGID,
                            &TEST_PROFILE_GUID,
                            &description,
                            &[],
                            0,
                            HKL::default(),
                            0,
                            false,
                            0,
                        )
                    },
                    "register TSF test profile",
                )?;
                let mut installed = Vec::new();
                for id in [GUID_TFCAT_TIP_KEYBOARD, GUID_TFCAT_DISPLAYATTRIBUTEPROVIDER] {
                    let result = unsafe {
                        category.RegisterCategory(&TEST_SERVICE_CLSID, &id, &TEST_SERVICE_CLSID)
                    };
                    if let Err(error) = result {
                        for registered in installed {
                            let _ = unsafe {
                                category.UnregisterCategory(
                                    &TEST_SERVICE_CLSID,
                                    &registered,
                                    &TEST_SERVICE_CLSID,
                                )
                            };
                        }
                        let _ = unsafe {
                            profiles.UnregisterProfile(
                                &TEST_SERVICE_CLSID,
                                JAPANESE_LANGID,
                                &TEST_PROFILE_GUID,
                                0,
                            )
                        };
                        return Err(format!("register TSF test category: {error}"));
                    }
                    installed.push(id);
                }
                if !registered(&profiles) {
                    return Err("TSF did not return the installed profile".into());
                }
            }
            "remove" => {
                let category: ITfCategoryMgr =
                    unsafe { CoCreateInstance(&CLSID_TF_CategoryMgr, None, CLSCTX_INPROC_SERVER) }
                        .map_err(|error| error.to_string())?;
                if registered(&profiles) {
                    let mut categories = Ok(());
                    for id in [GUID_TFCAT_DISPLAYATTRIBUTEPROVIDER, GUID_TFCAT_TIP_KEYBOARD] {
                        let outcome = check(
                            unsafe {
                                category.UnregisterCategory(
                                    &TEST_SERVICE_CLSID,
                                    &id,
                                    &TEST_SERVICE_CLSID,
                                )
                            },
                            "unregister TSF test category",
                        );
                        categories = categories.and(outcome);
                    }
                    check(
                        unsafe {
                            profiles.UnregisterProfile(
                                &TEST_SERVICE_CLSID,
                                JAPANESE_LANGID,
                                &TEST_PROFILE_GUID,
                                0,
                            )
                        },
                        "unregister TSF test profile",
                    )
                    .and(categories)?;
                }
                if registered(&profiles) {
                    return Err("TSF still returns the test profile".into());
                }
            }
            "enable" | "disable" => {
                if !registered(&profiles) {
                    return Err("test profile is absent".into());
                }
                check(
                    unsafe {
                        user.EnableLanguageProfile(
                            &TEST_SERVICE_CLSID,
                            JAPANESE_LANGID,
                            &TEST_PROFILE_GUID,
                            command == "enable",
                        )
                    },
                    "set current-user opt-in for the test profile",
                )?;
            }
            "status" => {
                let present = registered(&profiles);
                let enabled = present
                    && unsafe {
                        user.IsEnabledLanguageProfile(
                            &TEST_SERVICE_CLSID,
                            JAPANESE_LANGID,
                            &TEST_PROFILE_GUID,
                        )
                        .map_err(|error| error.to_string())?
                    }
                    .as_bool();
                println!("registered={present} enabled_for_current_user={enabled}");
            }
            _ => return Err("expected register, remove, enable, disable, or status".into()),
        }
        Ok(())
    }
}

#[cfg(all(windows, target_arch = "x86_64"))]
fn main() -> std::process::ExitCode {
    let mut arguments = std::env::args().skip(1);
    let command = arguments.next();
    let outcome = if arguments.next().is_none() {
        windows_probe::run(command.as_deref().unwrap_or_default())
    } else {
        Err("expected exactly one command".into())
    };
    match outcome {
        Ok(()) => std::process::ExitCode::SUCCESS,
        Err(error) => {
            eprintln!("{error}");
            std::process::ExitCode::FAILURE
        }
    }
}

#[cfg(not(all(windows, target_arch = "x86_64")))]
fn main() -> std::process::ExitCode {
    eprintln!("The stage A test profile requires x64 Windows.");
    std::process::ExitCode::FAILURE
}
