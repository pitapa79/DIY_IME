//! OS randomness is isolated from graph execution and never changes OS settings.
use super::SessionEpoch;
use crate::node::CoreError;

pub(crate) fn generate() -> Result<SessionEpoch, CoreError> {
    let mut bytes = [0u8; 16];
    fill(&mut bytes)?;
    const HEX: &[u8; 16] = b"0123456789abcdef";
    let mut text = String::with_capacity(32);
    for byte in bytes {
        text.push(HEX[usize::from(byte >> 4)] as char);
        text.push(HEX[usize::from(byte & 15)] as char);
    }
    SessionEpoch::new(text)
}
#[cfg(windows)]
fn fill(bytes: &mut [u8; 16]) -> Result<(), CoreError> {
    #[link(name = "bcrypt")]
    unsafe extern "system" {
        #[link_name = "BCryptGenRandom"]
        fn bcrypt_gen_random(
            algorithm: *mut std::ffi::c_void,
            buffer: *mut u8,
            length: u32,
            flags: u32,
        ) -> i32;
    }
    // https://learn.microsoft.com/en-us/windows/win32/api/bcrypt/nf-bcrypt-bcryptgenrandom
    // SAFETY: the system-preferred provider requires a null algorithm handle.
    // The writable initialized buffer has exactly the supplied 16-byte length.
    let status = unsafe { bcrypt_gen_random(std::ptr::null_mut(), bytes.as_mut_ptr(), 16, 2) };
    if status == 0 {
        Ok(())
    } else {
        Err(CoreError::new(
            "epoch-unavailable",
            format!("OS random generation failed ({status:#x})"),
        ))
    }
}
#[cfg(unix)]
fn fill(bytes: &mut [u8; 16]) -> Result<(), CoreError> {
    use std::io::Read;
    std::fs::File::open("/dev/urandom")
        .and_then(|mut file| file.read_exact(bytes))
        .map_err(|error| CoreError::new("epoch-unavailable", error.to_string()))
}
#[cfg(not(any(windows, unix)))]
fn fill(_: &mut [u8; 16]) -> Result<(), CoreError> {
    Err(CoreError::new(
        "epoch-unavailable",
        "this host has no implemented OS random source",
    ))
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn os_random_epochs_are_canonical_and_independent() {
        let mut epochs = std::collections::BTreeSet::new();
        for _ in 0..64 {
            let epoch = generate().unwrap();
            assert_eq!(epoch.as_str().len(), 32);
            assert!(epochs.insert(epoch));
        }
    }
}
