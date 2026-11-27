use super::*;
use std::{
    cell::Cell,
    rc::Rc,
    time::{Duration, Instant},
};
use windows::Win32::{
    Foundation::{E_FAIL, HWND, LPARAM, WPARAM},
    System::Com::{
        CLSCTX_INPROC_SERVER, COINIT_APARTMENTTHREADED, CoCreateInstance, CoInitializeEx,
        CoUninitialize,
    },
    UI::{
        Input::KeyboardAndMouse::{EnableWindow, SetFocus},
        TextServices::{
            CLSID_TF_ThreadMgr, IS_PASSWORD, ITfEditSession, ITfEditSession_Impl, ITfThreadMgr,
            InputScope, TF_CONTEXT_EDIT_CONTEXT_FLAGS, TF_ES_READ, TF_ES_SYNC,
        },
        WindowsAndMessaging::{
            CreateWindowExW, DestroyWindow, DispatchMessageW, MSG, PM_REMOVE, PeekMessageW,
            SW_SHOW, SendMessageW, SetForegroundWindow, SetWindowLongW, ShowWindow,
            TranslateMessage, WINDOW_EX_STYLE, WS_OVERLAPPEDWINDOW, WS_VISIBLE,
        },
    },
};
use windows_core::{HRESULT, Result, VARIANT, implement, s, w};

fn set_input_scope(window: HWND, scope: InputScope) -> Result<()> {
    use windows::Win32::System::LibraryLoader::{GetModuleHandleW, GetProcAddress};
    let module = unsafe { GetModuleHandleW(w!("msctf.dll"))? };
    let address = unsafe { GetProcAddress(module, s!("SetInputScope")) }.ok_or(E_FAIL)?;
    let set: unsafe extern "system" fn(HWND, InputScope) -> HRESULT =
        unsafe { std::mem::transmute(address) };
    unsafe { set(window, scope).ok() }
}

struct Window(HWND);
impl Drop for Window {
    fn drop(&mut self) {
        let _ = unsafe { DestroyWindow(self.0) };
    }
}

#[implement(ITfEditSession)]
struct EligibilityProbe {
    context: ITfContext,
    result: Rc<Cell<Option<bool>>>,
}
impl ITfEditSession_Impl for EligibilityProbe_Impl {
    fn DoEditSession(&self, cookie: u32) -> Result<()> {
        self.result
            .set(Some(editable_selection(&self.context, cookie).is_some()));
        Ok(())
    }
}

fn eligible(context: &ITfContext, client: u32) -> Result<bool> {
    let result = Rc::new(Cell::new(None));
    let session: ITfEditSession = EligibilityProbe {
        context: context.clone(),
        result: result.clone(),
    }
    .into();
    unsafe {
        context
            .RequestEditSession(
                client,
                &session,
                TF_CONTEXT_EDIT_CONTEXT_FLAGS(TF_ES_SYNC.0 | TF_ES_READ.0),
            )?
            .ok()?;
    }
    result.get().ok_or(E_FAIL.into())
}

#[test]
#[ignore = "requires a foreground Windows desktop for native Edit eligibility checks"]
fn native_edit_attributes_preserve_protection() -> Result<()> {
    unsafe { CoInitializeEx(None, COINIT_APARTMENTTHREADED).ok()? };
    let outcome = (|| -> Result<()> {
        let manager: ITfThreadMgr =
            unsafe { CoCreateInstance(&CLSID_TF_ThreadMgr, None, CLSCTX_INPROC_SERVER)? };
        let client = unsafe { manager.Activate()? };
        let result = (|| -> Result<()> {
            let window = Window(unsafe {
                CreateWindowExW(
                    WINDOW_EX_STYLE(0),
                    w!("STATIC"),
                    w!("DIY IME native attribute regression"),
                    WS_OVERLAPPEDWINDOW,
                    0,
                    0,
                    480,
                    180,
                    None,
                    None,
                    None,
                    None,
                )?
            });
            let edit = Window(unsafe {
                CreateWindowExW(
                    WINDOW_EX_STYLE(0),
                    w!("EDIT"),
                    w!(""),
                    WS_CHILD | WS_VISIBLE,
                    10,
                    10,
                    400,
                    35,
                    window.0,
                    None,
                    None,
                    None,
                )?
            });
            unsafe {
                let _ = ShowWindow(window.0, SW_SHOW);
                let _ = SetForegroundWindow(window.0);
                let _ = SetFocus(edit.0);
            }
            let deadline = Instant::now() + Duration::from_secs(10);
            let context = loop {
                let mut message = MSG::default();
                unsafe {
                    while PeekMessageW(&mut message, None, 0, 0, PM_REMOVE).as_bool() {
                        let _ = TranslateMessage(&message);
                        DispatchMessageW(&message);
                    }
                }
                if let Ok(context) = unsafe { manager.GetFocus().and_then(|doc| doc.GetTop()) } {
                    break context;
                }
                assert!(
                    Instant::now() < deadline,
                    "native Edit must expose a TSF context"
                );
                std::thread::sleep(Duration::from_millis(10));
            };
            assert!(
                eligible(&context, client)?,
                "ordinary Edit with missing TSF values"
            );
            let style = unsafe { GetWindowLongW(edit.0, GWL_STYLE) };
            for denied in [ES_PASSWORD, ES_READONLY] {
                unsafe {
                    SetWindowLongW(edit.0, GWL_STYLE, style | denied);
                }
                assert!(
                    !eligible(&context, client)?,
                    "protected Edit style must deny input"
                );
                unsafe {
                    SetWindowLongW(edit.0, GWL_STYLE, style);
                }
                assert!(eligible(&context, client)?);
            }
            let compartments: ITfCompartmentMgr = context.cast()?;
            let disabled =
                unsafe { compartments.GetCompartment(&GUID_COMPARTMENT_KEYBOARD_DISABLED)? };
            unsafe {
                disabled.SetValue(client, &VARIANT::from(1_i32))?;
            }
            assert!(
                !eligible(&context, client)?,
                "explicit TSF disable must win"
            );
            unsafe {
                disabled.SetValue(client, &VARIANT::from(windows_core::BSTR::from("unknown")))?;
            }
            assert!(
                !eligible(&context, client)?,
                "unknown TSF type must deny input"
            );
            unsafe {
                disabled.SetValue(client, &VARIANT::from(0_i32))?;
            }
            assert!(eligible(&context, client)?);
            set_input_scope(edit.0, IS_PASSWORD)?;
            assert!(
                !eligible(&context, client)?,
                "explicit password InputScope must win"
            );
            set_input_scope(edit.0, IS_DEFAULT)?;
            assert!(eligible(&context, client)?);
            let empty = unsafe { compartments.GetCompartment(&GUID_COMPARTMENT_EMPTYCONTEXT)? };
            unsafe {
                empty.SetValue(client, &VARIANT::from(1_i32))?;
            }
            assert!(
                !eligible(&context, client)?,
                "empty TSF context must deny input"
            );
            unsafe {
                empty.SetValue(client, &VARIANT::from(windows_core::BSTR::from("unknown")))?;
            }
            assert!(
                !eligible(&context, client)?,
                "unknown empty-context type must deny input"
            );
            unsafe {
                empty.SetValue(client, &VARIANT::from(0_i32))?;
            }
            assert!(eligible(&context, client)?);
            const EM_SETPASSWORDCHAR: u32 = 0x00cc;
            unsafe {
                SendMessageW(
                    edit.0,
                    EM_SETPASSWORDCHAR,
                    WPARAM(usize::from(b'*')),
                    LPARAM(0),
                );
            }
            assert!(
                !eligible(&context, client)?,
                "dynamic password masking must deny input"
            );
            unsafe {
                SendMessageW(edit.0, EM_SETPASSWORDCHAR, WPARAM(0), LPARAM(0));
            }
            assert!(eligible(&context, client)?);
            unsafe {
                SetWindowLongW(edit.0, GWL_STYLE, style | ES_PASSWORD);
            }
            assert!(
                !eligible(&context, client)?,
                "native password style must override default InputScope"
            );
            unsafe {
                SetWindowLongW(edit.0, GWL_STYLE, style);
            }
            unsafe {
                let _ = SetFocus(window.0);
            }
            assert!(
                !eligible(&context, client)?,
                "unfocused native field must deny input"
            );
            unsafe {
                let _ = SetFocus(edit.0);
            }
            assert!(eligible(&context, client)?);
            unsafe {
                let _ = EnableWindow(edit.0, false);
            }
            assert!(
                !eligible(&context, client)?,
                "disabled native window must deny input"
            );
            Ok(())
        })();
        result.and(unsafe { manager.Deactivate() })
    })();
    unsafe {
        CoUninitialize();
    }
    outcome
}
