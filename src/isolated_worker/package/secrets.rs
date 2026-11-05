//! Host-only, current-user DPAPI vault. No plaintext read/export wire operation.
use super::{
    catalog::{read_bounded, regular, replace, write_temp},
    format::digest,
    manager::nonce,
};
use crate::node::CoreError;
use serde::{Deserialize, Deserializer, Serialize};
use std::{
    collections::BTreeMap,
    fs::{self, File, OpenOptions},
    os::windows::fs::{MetadataExt, OpenOptionsExt},
    path::{Path, PathBuf},
    ptr,
};
use windows_sys::Win32::{
    Foundation::LocalFree,
    Security::Cryptography::{
        CRYPT_INTEGER_BLOB, CRYPTPROTECT_UI_FORBIDDEN, CryptProtectData, CryptUnprotectData,
    },
};
const MAX_SECRET: usize = 4096;
const MAX_CIPHER: usize = 16 * 1024;
const MAX_ENTRIES: usize = 64;
const MAX_STORE: usize = 8 * 1024 * 1024;
const MAGIC: &[u8; 8] = b"DIY2SEC1";
fn fail(code: &str, message: &str) -> CoreError {
    CoreError::new(code, message)
}
fn invalid() -> CoreError {
    fail(
        "secret-store",
        "invalid or unavailable encrypted Host secret store",
    )
}
fn id(value: &str) -> bool {
    !value.is_empty()
        && value.len() <= 128
        && !value.starts_with('.')
        && !value.ends_with('.')
        && value
            .bytes()
            .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || b"._-".contains(&c))
}
fn version(value: &str) -> bool {
    value.len() == 32
        && value
            .bytes()
            .all(|c| c.is_ascii_digit() || (b'a'..=b'f').contains(&c))
}
fn wipe(bytes: &mut [u8]) {
    for byte in bytes {
        unsafe {
            ptr::write_volatile(byte, 0);
        }
    }
    std::sync::atomic::compiler_fence(std::sync::atomic::Ordering::SeqCst);
}
/// Owns sensitive bytes without Debug/Serialize/Clone. Browser/JS GC is separate.
pub struct SecretValue(Vec<u8>);
impl Drop for SecretValue {
    fn drop(&mut self) {
        wipe(&mut self.0);
    }
}
impl SecretValue {
    pub(super) fn bytes(&self) -> &[u8] {
        &self.0
    }
    pub fn new(bytes: Vec<u8>) -> Result<Self, CoreError> {
        let value = Self(bytes);
        if value.0.is_empty() || value.0.len() > MAX_SECRET || value.0.contains(&0) {
            return Err(fail(
                "secret-value",
                "secret must be nonempty and within the Host size limit",
            ));
        }
        Ok(value)
    }
}
impl<'de> Deserialize<'de> for SecretValue {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let bytes = String::deserialize(deserializer)?.into_bytes();
        Self::new(bytes).map_err(|_| serde::de::Error::custom("invalid Host secret value"))
    }
}
/// Request frame memory is cleared after strict decoding, also on errors.
pub(super) struct SensitiveFrame(pub Vec<u8>);
impl Drop for SensitiveFrame {
    fn drop(&mut self) {
        wipe(&mut self.0);
    }
}
struct DpapiBuffer(CRYPT_INTEGER_BLOB);
impl Drop for DpapiBuffer {
    fn drop(&mut self) {
        if !self.0.pbData.is_null() {
            unsafe {
                wipe(std::slice::from_raw_parts_mut(
                    self.0.pbData,
                    self.0.cbData as usize,
                ));
                LocalFree(self.0.pbData.cast());
            }
        }
    }
}
fn crypt(bytes: &[u8], name: &str, generation: &str, encrypt: bool) -> Result<Vec<u8>, CoreError> {
    let entropy = format!("DIYIME2.HostSecrets.v1\0{name}\0{generation}").into_bytes();
    let input = CRYPT_INTEGER_BLOB {
        cbData: bytes.len() as u32,
        pbData: bytes.as_ptr().cast_mut(),
    };
    let salt = CRYPT_INTEGER_BLOB {
        cbData: entropy.len() as u32,
        pbData: entropy.as_ptr().cast_mut(),
    };
    let mut output = DpapiBuffer(CRYPT_INTEGER_BLOB {
        cbData: 0,
        pbData: ptr::null_mut(),
    });
    let result = unsafe {
        if encrypt {
            CryptProtectData(
                &input,
                ptr::null(),
                &salt,
                ptr::null(),
                ptr::null(),
                CRYPTPROTECT_UI_FORBIDDEN,
                &mut output.0,
            )
        } else {
            CryptUnprotectData(
                &input,
                ptr::null_mut(),
                &salt,
                ptr::null(),
                ptr::null(),
                CRYPTPROTECT_UI_FORBIDDEN,
                &mut output.0,
            )
        }
    };
    if result == 0
        || output.0.pbData.is_null()
        || output.0.cbData == 0
        || output.0.cbData as usize > MAX_CIPHER
    {
        return Err(fail(
            "secret-crypto",
            "current-user secret encryption or integrity check failed",
        ));
    }
    Ok(unsafe { std::slice::from_raw_parts(output.0.pbData, output.0.cbData as usize) }.to_vec())
}
#[derive(Clone, Deserialize, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct Entry {
    id: String,
    version: String,
    ciphertext: Vec<u8>,
}
#[derive(Deserialize, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct Stored {
    format_version: u32,
    entries: Vec<Entry>,
}
#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SecretMetadata {
    pub id: String,
    pub version: String,
}
fn decode(entry: &Entry) -> Result<SecretValue, CoreError> {
    let plain = SensitiveFrame(crypt(&entry.ciphertext, &entry.id, &entry.version, false)?);
    if plain.0.len() < 12 + 64 || &plain.0[..8] != MAGIC {
        return Err(invalid());
    }
    let length = u32::from_le_bytes(plain.0[8..12].try_into().unwrap()) as usize;
    if length == 0 || length > MAX_SECRET || plain.0.len() != 12 + length + 64 {
        return Err(invalid());
    }
    let bytes = &plain.0[12..12 + length];
    let checksum = digest(&plain.0[..12 + length])?;
    if checksum.as_bytes() != &plain.0[12 + length..] {
        return Err(invalid());
    }
    SecretValue::new(bytes.to_vec())
}
fn encode(name: &str, secret: &SecretValue) -> Result<Entry, CoreError> {
    let generation = nonce()?;
    let mut plain = SensitiveFrame(Vec::with_capacity(12 + secret.0.len() + 64));
    plain.0.extend_from_slice(MAGIC);
    plain
        .0
        .extend_from_slice(&(secret.0.len() as u32).to_le_bytes());
    plain.0.extend_from_slice(&secret.0);
    let checksum = digest(&plain.0)?;
    plain.0.extend_from_slice(checksum.as_bytes());
    let entry = Entry {
        id: name.into(),
        version: generation.clone(),
        ciphertext: crypt(&plain.0, name, &generation, true)?,
    };
    if decode(&entry)?.0 != secret.0 {
        return Err(invalid());
    }
    Ok(entry)
}
/// Immutable encrypted reference captured by trusted Host code. No wire constructor.
/// Its version/policy identify the exact captured value without exposing its hash.
pub(crate) struct SecretReference {
    entry: Entry,
    policy: String,
}
impl SecretReference {
    pub(super) fn metadata(&self) -> SecretMetadata {
        SecretMetadata {
            id: self.entry.id.clone(),
            version: self.entry.version.clone(),
        }
    }
    pub(super) fn policy(&self) -> &str {
        &self.policy
    }
    pub(super) fn unprotect(&self) -> Result<SecretValue, CoreError> {
        decode(&self.entry)
    }
}
pub struct SecretVault {
    root: PathBuf,
    _lock: File,
    entries: BTreeMap<String, Entry>,
}
impl SecretVault {
    /// Fixed Host-owned root, never a guest cache; one owner prevents stale writers.
    pub fn open(root: &Path) -> Result<Self, CoreError> {
        regular(root, true).map_err(|_| invalid())?;
        let root = root.canonicalize().map_err(|_| invalid())?;
        let lock = OpenOptions::new()
            .read(true)
            .write(true)
            .create(true)
            .truncate(false)
            .share_mode(0)
            .custom_flags(0x00200000)
            .open(root.join("secrets.lock"))
            .map_err(|_| {
                fail(
                    "secret-busy",
                    "encrypted Host secret store already owned/unavailable",
                )
            })?;
        let metadata = lock.metadata().map_err(|_| invalid())?;
        if !metadata.is_file() || metadata.file_attributes() & 0x400 != 0 {
            return Err(invalid());
        }
        let path = root.join("secrets.json");
        let stored = match fs::symlink_metadata(&path) {
            Ok(_) => serde_json::from_slice::<Stored>(
                &read_bounded(&path, MAX_STORE).map_err(|_| invalid())?,
            )
            .map_err(|_| invalid())?,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => Stored {
                format_version: 1,
                entries: vec![],
            },
            Err(_) => return Err(invalid()),
        };
        if stored.format_version != 1 || stored.entries.len() > MAX_ENTRIES {
            return Err(invalid());
        }
        let mut entries = BTreeMap::new();
        for entry in stored.entries {
            if !id(&entry.id)
                || !version(&entry.version)
                || entry.ciphertext.is_empty()
                || entry.ciphertext.len() > MAX_CIPHER
                || entries.contains_key(&entry.id)
            {
                return Err(invalid());
            }
            // DPAPI plus a strict decrypted envelope, not a particular OS error code.
            let _verified = decode(&entry)?;
            entries.insert(entry.id.clone(), entry);
        }
        Ok(Self {
            root,
            _lock: lock,
            entries,
        })
    }
    pub(crate) fn capture(&self, name: &str) -> Result<SecretReference, CoreError> {
        use std::os::windows::ffi::OsStrExt;
        let entry = self
            .entries
            .get(name)
            .ok_or_else(|| fail("secret-id", "unknown logical secret ID"))?
            .clone();
        let policy = digest(
            &serde_json::to_vec(&(
                "DIYIME2.HostSecretReference.v1",
                self.root.as_os_str().encode_wide().collect::<Vec<_>>(),
                &entry.id,
                &entry.version,
            ))
            .map_err(|_| invalid())?,
        )?;
        Ok(SecretReference { entry, policy })
    }
    pub fn list(&self) -> Vec<SecretMetadata> {
        self.entries
            .values()
            .map(|e| SecretMetadata {
                id: e.id.clone(),
                version: e.version.clone(),
            })
            .collect()
    }
    fn persist(&self, entries: &BTreeMap<String, Entry>) -> Result<(), CoreError> {
        let bytes = serde_json::to_vec(&Stored {
            format_version: 1,
            entries: entries.values().cloned().collect(),
        })
        .map_err(|_| invalid())?;
        if bytes.len() > MAX_STORE {
            return Err(fail("secret-limit", "encrypted Host store limit exceeded"));
        }
        regular(&self.root, true).map_err(|_| invalid())?;
        let temporary = write_temp(&self.root, &bytes).map_err(|_| invalid())?;
        if replace(&temporary, &self.root.join("secrets.json")).is_err() {
            let _ = fs::remove_file(temporary);
            return Err(invalid());
        }
        Ok(())
    }
    pub fn put(&mut self, name: &str, value: SecretValue) -> Result<SecretMetadata, CoreError> {
        if !id(name) {
            return Err(fail("secret-id", "invalid logical secret ID"));
        }
        if !self.entries.contains_key(name) && self.entries.len() >= MAX_ENTRIES {
            return Err(fail(
                "secret-limit",
                "encrypted Host secret count limit exceeded",
            ));
        }
        let entry = encode(name, &value)?;
        let metadata = SecretMetadata {
            id: entry.id.clone(),
            version: entry.version.clone(),
        };
        let mut next = self.entries.clone();
        next.insert(name.into(), entry);
        self.persist(&next)?;
        self.entries = next;
        Ok(metadata)
    }
    pub fn remove(&mut self, name: &str) -> Result<(), CoreError> {
        if !id(name) || !self.entries.contains_key(name) {
            return Err(fail("secret-id", "unknown logical secret ID"));
        }
        let mut next = self.entries.clone();
        next.remove(name);
        self.persist(&next)?;
        self.entries = next;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    struct Fixture(PathBuf);
    impl Fixture {
        fn new() -> Self {
            let root = std::env::temp_dir().join(format!("diyime-vault-{}", nonce().unwrap()));
            fs::create_dir(&root).unwrap();
            Self(root)
        }
        fn vault(&self) -> SecretVault {
            SecretVault::open(&self.0).unwrap()
        }
        fn read(&self) -> Vec<u8> {
            fs::read(self.0.join("secrets.json")).unwrap()
        }
    }
    impl Drop for Fixture {
        fn drop(&mut self) {
            for name in ["secrets.json", "secrets.lock"] {
                let _ = fs::remove_file(self.0.join(name));
            }
            let _ = fs::remove_dir(&self.0);
        }
    }
    fn value(s: &str) -> SecretValue {
        SecretValue::new(s.as_bytes().to_vec()).unwrap()
    }
    #[test]
    fn dpapi_roundtrip_reopen_rotation_delete_and_metadata_never_export_plaintext() {
        let fixture = Fixture::new();
        let mut vault = fixture.vault();
        assert!(vault.list().is_empty());
        let first = vault
            .put("service-key", value("synthetic-private-secret-秘密"))
            .unwrap();
        assert_eq!(
            decode(&vault.entries["service-key"]).unwrap().0,
            "synthetic-private-secret-秘密".as_bytes()
        );
        for bytes in [fixture.read(), serde_json::to_vec(&vault.list()).unwrap()] {
            assert!(!String::from_utf8_lossy(&bytes).contains("synthetic-private-secret"));
        }
        let changed = vault
            .put("service-key", value("second-private-secret"))
            .unwrap();
        assert_ne!(first.version, changed.version);
        drop(vault);
        let mut vault = fixture.vault();
        assert_eq!(vault.list()[0].version, changed.version);
        assert_eq!(
            decode(&vault.entries["service-key"]).unwrap().0,
            b"second-private-secret"
        );
        vault.remove("service-key").unwrap();
        assert!(vault.list().is_empty());
        drop(vault);
        assert!(fixture.vault().list().is_empty());
    }
    #[test]
    fn exclusive_ownership_and_failed_save_cannot_replace_the_current_secret() {
        let fixture = Fixture::new();
        let mut vault = fixture.vault();
        assert!(SecretVault::open(&fixture.0).is_err());
        let original = vault.put("key", value("original-private")).unwrap();
        let disk = fixture.read();
        let locked = OpenOptions::new()
            .read(true)
            .share_mode(0)
            .open(fixture.0.join("secrets.json"))
            .unwrap();
        assert!(vault.put("key", value("refused-private")).is_err());
        assert!(vault.remove("key").is_err());
        assert_eq!(vault.list()[0].version, original.version);
        assert_eq!(
            decode(&vault.entries["key"]).unwrap().0,
            b"original-private"
        );
        drop(locked);
        assert_eq!(fixture.read(), disk);
        vault.put("key", value("recovered-private")).unwrap();
    }
    #[test]
    fn invalid_values_ids_and_strict_store_limits_fail_without_mutation() {
        let fixture = Fixture::new();
        let mut vault = fixture.vault();
        for bytes in [vec![], vec![1; MAX_SECRET + 1], vec![0]] {
            assert!(SecretValue::new(bytes).is_err());
        }
        for name in [
            "",
            "../escape",
            ".hidden",
            "ending.",
            "Upper",
            "white space",
            "a:b",
        ] {
            assert!(vault.put(name, value("private")).is_err());
        }
        assert!(vault.list().is_empty());
        assert!(!fixture.0.join("secrets.json").exists());
        assert!(vault.remove("unknown").is_err());
        drop(vault);
        for stored in [
            r#"{"formatVersion":2,"entries":[]}"#,
            r#"{"formatVersion":1,"entries":[],"unknown":true}"#,
            r#"{"formatVersion":1,"formatVersion":1,"entries":[]}"#,
        ] {
            fs::write(fixture.0.join("secrets.json"), stored).unwrap();
            assert!(SecretVault::open(&fixture.0).is_err());
        }
        fs::write(fixture.0.join("secrets.json"), vec![b' '; MAX_STORE + 1]).unwrap();
        assert!(SecretVault::open(&fixture.0).is_err());
    }
    #[test]
    fn ciphertext_corruption_renamed_entries_and_invalid_decrypted_envelopes_fail_closed() {
        let fixture = Fixture::new();
        let mut vault = fixture.vault();
        vault.put("key", value("private-integrity-canary")).unwrap();
        let original = vault.entries["key"].clone();
        drop(vault);
        for mutate in 0..4 {
            let mut entry = original.clone();
            match mutate {
                0 => entry.ciphertext[0] ^= 1,
                1 => entry.id = "renamed".into(),
                2 => entry.version = "a".repeat(32),
                _ => entry.ciphertext.clear(),
            };
            fs::write(
                fixture.0.join("secrets.json"),
                serde_json::to_vec(&Stored {
                    format_version: 1,
                    entries: vec![entry],
                })
                .unwrap(),
            )
            .unwrap();
            assert!(SecretVault::open(&fixture.0).is_err());
        }
        for bytes in [b"wrong-envelope".as_slice(), b"DIY2SEC1\0\0\0\0".as_slice()] {
            let mut entry = original.clone();
            entry.ciphertext = crypt(bytes, &entry.id, &entry.version, true).unwrap();
            assert!(decode(&entry).is_err());
        }
        fs::write(
            fixture.0.join("secrets.json"),
            serde_json::to_vec(&Stored {
                format_version: 1,
                entries: vec![original.clone(), original],
            })
            .unwrap(),
        )
        .unwrap();
        assert!(SecretVault::open(&fixture.0).is_err());
    }
    #[test]
    fn entry_count_limit_accepts_replacement_and_missing_configuration_never_creates_files() {
        let fixture = Fixture::new();
        let mut vault = fixture.vault();
        for i in 0..MAX_ENTRIES {
            vault
                .put(&format!("key-{i}"), value("synthetic-limit-secret"))
                .unwrap();
        }
        assert_eq!(vault.list().len(), MAX_ENTRIES);
        let disk = fixture.read();
        assert!(vault.put("one-too-many", value("refused-private")).is_err());
        assert_eq!(fixture.read(), disk);
        vault.put("key-0", value("replacement-private")).unwrap();
        assert_eq!(vault.list().len(), MAX_ENTRIES);
        drop(vault);
        assert_eq!(fixture.vault().list().len(), MAX_ENTRIES);
        assert!(SecretVault::open(&fixture.0.join("missing")).is_err());
        assert!(!fixture.0.join("missing").exists());
    }
}
