//! Fixed messages for the synthetic-input Stage B desktop tests.

macro_rules! trace {
    ($message:literal) => {
        #[cfg(feature = "stage-b-diagnostics")]
        unsafe {
            windows::Win32::System::Diagnostics::Debug::OutputDebugStringW(windows_core::w!(
                $message
            ));
        }
    };
}

pub(crate) use trace;
