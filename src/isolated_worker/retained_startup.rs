//! Fixed private readiness frame. Measurements are diagnostic only.
use std::io::{self, Read, Write};

pub(super) const TIMED_FLAG: &str = "--retained-compiled-startup-timing-v1";
const READY: &[u8; 8] = b"DIYRDY01";
const TIMED: &[u8; 8] = b"DIYRDY02";

#[derive(Debug, PartialEq, Eq)]
pub(super) struct Timing {
    pub dll_verify_us: u64,
    pub dll_load_us: u64,
    pub module_read_us: u64,
    pub module_restore_us: u64,
}

pub(super) type Observer = Box<dyn FnOnce(Timing) + Send>;

pub(super) fn write_ready(
    out: &mut impl Write,
    modules: u32,
    timing: Option<Timing>,
) -> io::Result<()> {
    out.write_all(if timing.is_some() { TIMED } else { READY })?;
    out.write_all(&modules.to_le_bytes())?;
    if let Some(timing) = timing {
        for value in [
            timing.dll_verify_us,
            timing.dll_load_us,
            timing.module_read_us,
            timing.module_restore_us,
        ] {
            out.write_all(&value.to_le_bytes())?;
        }
    }
    out.flush()
}

pub(super) fn read_ready(
    input: &mut impl Read,
    timed: bool,
) -> Result<(u32, Option<Timing>), &'static str> {
    let mut header = [0; 12];
    input
        .read_exact(&mut header)
        .map_err(|_| "retained readiness unavailable")?;
    let count = u32::from_le_bytes(header[8..].try_into().unwrap());
    let expected = if timed { TIMED } else { READY };
    if &header[..8] != expected || !(1..=4).contains(&count) || (timed && count != 1) {
        return Err("invalid retained readiness");
    }
    let timing = if timed {
        let mut payload = [0; 32];
        input
            .read_exact(&mut payload)
            .map_err(|_| "retained startup timing unavailable")?;
        let mut values = payload
            .chunks_exact(8)
            .map(|b| u64::from_le_bytes(b.try_into().unwrap()));
        let timing = Timing {
            dll_verify_us: values.next().unwrap(),
            dll_load_us: values.next().unwrap(),
            module_read_us: values.next().unwrap(),
            module_restore_us: values.next().unwrap(),
        };
        Some(timing)
    } else {
        None
    };
    Ok((count, timing))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn original_readiness_is_preserved() {
        let mut bytes = vec![];
        write_ready(&mut bytes, 4, None).unwrap();
        assert_eq!(bytes, b"DIYRDY01\x04\0\0\0");
        assert_eq!(read_ready(&mut bytes.as_slice(), false), Ok((4, None)));
        assert!(read_ready(&mut bytes.as_slice(), true).is_err());
    }

    #[test]
    fn timed_readiness_is_explicit_singleton_and_requires_all_measurements() {
        let timing = Timing {
            dll_verify_us: 1,
            dll_load_us: 2,
            module_read_us: 3,
            module_restore_us: 4,
        };
        let mut bytes = vec![];
        write_ready(&mut bytes, 1, Some(timing)).unwrap();
        assert_eq!(
            read_ready(&mut bytes.as_slice(), true),
            Ok((
                1,
                Some(Timing {
                    dll_verify_us: 1,
                    dll_load_us: 2,
                    module_read_us: 3,
                    module_restore_us: 4
                })
            ))
        );
        assert!(read_ready(&mut bytes.as_slice(), false).is_err());
        for length in 0..bytes.len() {
            assert!(read_ready(&mut &bytes[..length], true).is_err());
        }
        bytes[8] = 2;
        assert!(read_ready(&mut bytes.as_slice(), true).is_err());
    }
}
