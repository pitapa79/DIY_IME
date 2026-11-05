//! One thread owns synchronous WinHTTP calls and handle closure through completion.
use super::super::secrets::{SecretReference, SecretValue, SensitiveFrame};
use super::{HttpEndpointConfig, HttpMethod, HttpReply, fail};
use crate::node::CoreError;
use std::{
    ffi::c_void,
    mem::size_of,
    ptr, thread,
    time::{Duration, Instant},
};
use windows_sys::Win32::{Foundation::GetLastError, Networking::WinHttp::*};
#[cfg(test)]
thread_local! { static READ_MARKER: std::cell::RefCell<Option<std::sync::mpsc::Sender<()>>> = const { std::cell::RefCell::new(None) }; }
#[cfg(test)]
pub(super) fn observe_read(marker: std::sync::mpsc::Sender<()>) {
    READ_MARKER.with(|value| *value.borrow_mut() = Some(marker));
}
fn io_failure() -> CoreError {
    fail("network-io", "fixed endpoint HTTP operation failed")
}
struct Internet(*mut c_void);
impl Drop for Internet {
    fn drop(&mut self) {
        // No concurrent API call or callback uses this synchronous handle. A
        // failed close cannot be converted into a released operation owner.
        while unsafe { WinHttpCloseHandle(self.0) } == 0 {
            thread::sleep(Duration::from_millis(20));
        }
    }
}
fn handle(
    check: &mut dyn FnMut() -> Result<(), CoreError>,
    call: impl FnOnce() -> *mut c_void,
) -> Result<Internet, CoreError> {
    check()?;
    let raw = call();
    let owned = if raw.is_null() {
        None
    } else {
        Some(Internet(raw))
    };
    check()?;
    owned.ok_or_else(io_failure)
}
fn io(
    check: &mut dyn FnMut() -> Result<(), CoreError>,
    call: impl FnOnce() -> i32,
) -> Result<(), CoreError> {
    checked_io(check, call, |code| {
        fail(
            "network-io",
            &format!("fixed endpoint HTTP operation failed (OS {code})"),
        )
    })
}
fn checked_io(
    check: &mut dyn FnMut() -> Result<(), CoreError>,
    call: impl FnOnce() -> i32,
    failure: impl FnOnce(u32) -> CoreError,
) -> Result<(), CoreError> {
    check()?;
    let result = call();
    let code = unsafe { GetLastError() };
    check()?;
    if result == 0 {
        Err(failure(code))
    } else {
        Ok(())
    }
}
fn option<T>(
    h: &Internet,
    name: u32,
    value: &T,
    check: &mut dyn FnMut() -> Result<(), CoreError>,
) -> Result<(), CoreError> {
    checked_io(
        check,
        || unsafe {
            WinHttpSetOption(
                h.0,
                name,
                ptr::from_ref(value).cast(),
                size_of::<T>() as u32,
            )
        },
        |code| {
            if code == ERROR_WINHTTP_INVALID_OPTION {
                fail(
                    "network-platform",
                    &format!(
                        "required WinHTTP option {name} is unavailable on this Windows installation (OS {code})"
                    ),
                )
            } else {
                fail(
                    "network-io",
                    &format!("fixed endpoint HTTP operation failed (OS {code})"),
                )
            }
        },
    )
}
fn wide(value: &str) -> Vec<u16> {
    value.encode_utf16().chain(Some(0)).collect()
}
struct Header(Vec<u16>);
impl Drop for Header {
    fn drop(&mut self) {
        for unit in &mut self.0 {
            unsafe {
                ptr::write_volatile(unit, 0);
            }
        }
        std::sync::atomic::compiler_fence(std::sync::atomic::Ordering::SeqCst);
    }
}
fn headers(config: &HttpEndpointConfig, secret: Option<&SecretValue>) -> Result<Header, CoreError> {
    // Allocate the complete bounded header before copying any plaintext secret.
    // Reallocation after injection would leave an unwiped old allocation.
    let authentication_units = config.authentication.as_ref().map_or(0, |auth| {
        auth.header.len() + 2 + auth.prefix.len() + secret.map_or(0, |s| s.bytes().len()) + 2
    });
    let mut header = Header(Vec::with_capacity(
        "Content-Type: ".len() + config.content_type.len() + 2 + authentication_units + 1,
    ));
    header.0.extend("Content-Type: ".encode_utf16());
    header.0.extend(config.content_type.encode_utf16());
    header.0.extend("\r\n".encode_utf16());
    if let Some(auth) = &config.authentication {
        let secret = secret.ok_or_else(|| {
            fail(
                "network-secret",
                "fixed endpoint authentication unavailable",
            )
        })?;
        if !secret.bytes().iter().all(|c| (b' '..=b'~').contains(c)) {
            return Err(fail(
                "network-secret",
                "secret cannot be represented safely in the fixed header",
            ));
        }
        header.0.extend(auth.header.encode_utf16());
        header.0.extend(": ".encode_utf16());
        header.0.extend(auth.prefix.encode_utf16());
        header
            .0
            .extend(secret.bytes().iter().map(|c| u16::from(*c)));
        header.0.extend("\r\n".encode_utf16());
    }
    header.0.push(0);
    Ok(header)
}
fn content_length(
    request: &Internet,
    check: &mut dyn FnMut() -> Result<(), CoreError>,
) -> Result<Option<u64>, CoreError> {
    check()?;
    let mut length = 0u64;
    let mut bytes = size_of::<u64>() as u32;
    let result = unsafe {
        WinHttpQueryHeaders(
            request.0,
            WINHTTP_QUERY_CONTENT_LENGTH | WINHTTP_QUERY_FLAG_NUMBER64,
            ptr::null(),
            ptr::from_mut(&mut length).cast(),
            &mut bytes,
            ptr::null_mut(),
        )
    };
    let error = unsafe { GetLastError() };
    check()?;
    if result != 0 && bytes == size_of::<u64>() as u32 {
        Ok(Some(length))
    } else if result == 0 && error == ERROR_WINHTTP_HEADER_NOT_FOUND {
        Ok(None)
    } else {
        Err(io_failure())
    }
}
pub(super) fn exchange(
    config: &HttpEndpointConfig,
    reference: Option<&SecretReference>,
    body: &[u8],
    check: &mut dyn FnMut() -> Result<(), CoreError>,
) -> Result<HttpReply, CoreError> {
    if body.len() > config.request_bytes
        || matches!(config.method, HttpMethod::Get) && !body.is_empty()
    {
        return Err(fail(
            "network-limit",
            "request exceeds fixed endpoint policy",
        ));
    }
    let deadline = Instant::now()
        .checked_add(Duration::from_millis(config.timeouts.budget_ms))
        .ok_or_else(|| fail("network-config", "HTTP budget cannot be represented"))?;
    let mut live = || {
        check()?;
        if Instant::now() >= deadline {
            Err(fail(
                "network-deadline",
                "selected cooperative HTTP budget exceeded",
            ))
        } else {
            Ok(())
        }
    };
    live()?;
    // These outlive request handles, including all cancellation/error returns.
    let secret = reference.map(|r| r.unprotect()).transpose()?;
    live()?;
    let header = headers(config, secret.as_ref())?;
    let agent = wide("DIY_IME2/1");
    let session = handle(&mut live, || unsafe {
        WinHttpOpen(
            agent.as_ptr(),
            WINHTTP_ACCESS_TYPE_NO_PROXY,
            ptr::null(),
            ptr::null(),
            0,
        )
    })?;
    if config.secure {
        option(
            &session,
            WINHTTP_OPTION_SECURE_PROTOCOLS,
            &(WINHTTP_FLAG_SECURE_PROTOCOL_TLS1_2 | WINHTTP_FLAG_SECURE_PROTOCOL_TLS1_3),
            &mut live,
        )?;
    }
    option(
        &session,
        WINHTTP_OPTION_DISABLE_GLOBAL_POOLING,
        &1u32,
        &mut live,
    )?;
    option(
        &session,
        WINHTTP_OPTION_DISABLE_SECURE_PROTOCOL_FALLBACK,
        &1u32,
        &mut live,
    )?;
    option(
        &session,
        WINHTTP_OPTION_FAILED_CONNECTION_RETRIES,
        &WINHTTP_FAILED_CONNECTION_RETRIES {
            dwMaxRetries: 0,
            dwAllowedRetryConditions: 0,
        },
        &mut live,
    )?;
    io(&mut live, || unsafe {
        WinHttpSetTimeouts(
            session.0,
            config.timeouts.resolve_ms as i32,
            config.timeouts.connect_ms as i32,
            config.timeouts.send_ms as i32,
            config.timeouts.receive_ms as i32,
        )
    })?;
    let connect_host = if config.host.contains(':') {
        format!("[{}]", config.host)
    } else {
        config.host.clone()
    };
    let host = wide(&connect_host);
    let connection = handle(&mut live, || unsafe {
        WinHttpConnect(session.0, host.as_ptr(), config.port, 0)
    })?;
    let method = wide(config.method.name());
    let path = wide(&config.path);
    let version = wide("HTTP/1.1");
    let request = handle(&mut live, || unsafe {
        WinHttpOpenRequest(
            connection.0,
            method.as_ptr(),
            path.as_ptr(),
            version.as_ptr(),
            ptr::null(),
            ptr::null(),
            if config.secure {
                WINHTTP_FLAG_SECURE
            } else {
                0
            },
        )
    })?;
    option(
        &request,
        WINHTTP_OPTION_DISABLE_FEATURE,
        &(WINHTTP_DISABLE_AUTHENTICATION
            | WINHTTP_DISABLE_COOKIES
            | WINHTTP_DISABLE_REDIRECTS
            | WINHTTP_DISABLE_KEEP_ALIVE),
        &mut live,
    )?;
    option(
        &request,
        WINHTTP_OPTION_AUTOLOGON_POLICY,
        &WINHTTP_AUTOLOGON_SECURITY_LEVEL_HIGH,
        &mut live,
    )?;
    option(
        &request,
        WINHTTP_OPTION_MAX_RESPONSE_HEADER_SIZE,
        &config.header_bytes,
        &mut live,
    )?;
    // A certificate request is an error from WinHTTP. Do not select a client
    // certificate, consult credential stores or retry with one. The documented
    // certificate-context setter is a response to CLIENT_AUTH_CERT_NEEDED,
    // not part of this broker's initial handshake configuration.
    io(&mut live, || unsafe {
        WinHttpSendRequest(
            request.0,
            header.0.as_ptr(),
            (header.0.len() - 1) as u32,
            if body.is_empty() {
                ptr::null()
            } else {
                body.as_ptr().cast()
            },
            body.len() as u32,
            body.len() as u32,
            0,
        )
    })?;
    io(&mut live, || unsafe {
        WinHttpReceiveResponse(request.0, ptr::null_mut())
    })?;
    let mut status = 0u32;
    let mut size = size_of::<u32>() as u32;
    io(&mut live, || unsafe {
        WinHttpQueryHeaders(
            request.0,
            WINHTTP_QUERY_STATUS_CODE | WINHTTP_QUERY_FLAG_NUMBER,
            ptr::null(),
            ptr::from_mut(&mut status).cast(),
            &mut size,
            ptr::null_mut(),
        )
    })?;
    if !(100..=599).contains(&status) || size != size_of::<u32>() as u32 {
        return Err(io_failure());
    }
    let expected = content_length(&request, &mut live)?;
    if expected.is_some_and(|n| n > config.response_bytes as u64) {
        return Err(fail(
            "network-limit",
            "response exceeds fixed endpoint policy",
        ));
    }
    let mut result = SensitiveFrame(Vec::new());
    // Reserve before receiving any body bytes so growth cannot leave discarded
    // copies of a reflected secret or other private response in the allocator.
    result
        .0
        .try_reserve_exact(config.response_bytes)
        .map_err(|_| fail("network-limit", "fixed response allocation is unavailable"))?;
    let mut buffer = SensitiveFrame(vec![0u8; 16 * 1024]);
    loop {
        let maximum = (config.response_bytes - result.0.len())
            .saturating_add(1)
            .min(buffer.0.len());
        let mut read = 0u32;
        io(&mut live, || unsafe {
            #[cfg(test)]
            READ_MARKER.with(|value| {
                if let Some(marker) = value.borrow_mut().take() {
                    let _ = marker.send(());
                }
            });
            WinHttpReadData(
                request.0,
                buffer.0.as_mut_ptr().cast(),
                maximum as u32,
                &mut read,
            )
        })?;
        if read == 0 {
            break;
        }
        if read as usize > maximum
            || result.0.len().saturating_add(read as usize) > config.response_bytes
        {
            return Err(fail(
                "network-limit",
                "response exceeds fixed endpoint policy",
            ));
        }
        result.0.extend_from_slice(&buffer.0[..read as usize]);
    }
    if expected.is_some_and(|n| n != result.0.len() as u64) {
        return Err(io_failure());
    }
    drop(request);
    drop(connection);
    drop(session);
    live()?;
    if secret.as_ref().is_some_and(|value| {
        result
            .0
            .windows(value.bytes().len())
            .any(|window| window == value.bytes())
    }) {
        return Err(fail(
            "network-secret",
            "endpoint reflected a protected authentication value",
        ));
    }
    Ok(HttpReply {
        status: status as u16,
        body: std::mem::take(&mut result.0),
    })
}

#[cfg(test)]
mod platform_tests {
    use super::*;

    #[test]
    fn unsupported_required_option_fails_before_any_connection() {
        let session = handle(&mut || Ok(()), || unsafe {
            WinHttpOpen(
                ptr::null(),
                WINHTTP_ACCESS_TYPE_NO_PROXY,
                ptr::null(),
                ptr::null(),
                0,
            )
        })
        .unwrap();
        // An unknown option is refused by the real OS on both old and current Windows.
        let error = option(&session, u32::MAX, &1u32, &mut || Ok(())).unwrap_err();
        assert_eq!(error.code, "network-platform");
        assert_eq!(
            error.message,
            "required WinHTTP option 4294967295 is unavailable on this Windows installation (OS 12009)"
        );
        // A refused setting does not trigger a retry without it or create a connection.
        option(
            &session,
            WINHTTP_OPTION_DISABLE_SECURE_PROTOCOL_FALLBACK,
            &1u32,
            &mut || Ok(()),
        )
        .unwrap();
    }

    #[test]
    fn withdrawal_after_option_call_takes_precedence_over_platform_diagnostic() {
        let mut checks = 0;
        let error = checked_io(
            &mut || {
                checks += 1;
                if checks == 2 {
                    Err(fail("cancelled", "test withdrawal"))
                } else {
                    Ok(())
                }
            },
            || 0,
            |_| panic!("withdrawn operation must not publish an OS diagnostic"),
        )
        .unwrap_err();
        assert_eq!(error.code, "cancelled");
        assert_eq!(checks, 2);
    }
}
