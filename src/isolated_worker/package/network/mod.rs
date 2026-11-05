//! Trusted Host-selected HTTP endpoints. Guest declarations never construct plans.
mod windows;
use super::{
    format::digest,
    secrets::{SecretMetadata, SecretReference, SecretVault},
};
use crate::node::CoreError;
use serde::{Deserialize, Serialize};
use std::{collections::BTreeMap, net::IpAddr, sync::Arc};
fn fail(code: &str, message: &str) -> CoreError {
    CoreError::new(code, message)
}
fn invalid() -> CoreError {
    fail("network-config", "invalid fixed Host network binding")
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
fn default_secure() -> bool {
    true
}
fn header_name(value: &str) -> bool {
    if value.is_empty()
        || value.len() > 128
        || !value
            .bytes()
            .all(|c| c.is_ascii_alphanumeric() || b"!#$%&'*+-.^_`|~".contains(&c))
    {
        return false;
    }
    let lower = value.to_ascii_lowercase();
    !lower.starts_with("proxy-")
        && !matches!(
            lower.as_str(),
            "host"
                | "connection"
                | "keep-alive"
                | "transfer-encoding"
                | "content-length"
                | "content-type"
                | "accept"
                | "cookie"
                | "set-cookie"
                | "trailer"
                | "te"
                | "upgrade"
                | "expect"
                | "origin"
                | "referer"
        )
}
fn printable(value: &str, maximum: usize) -> bool {
    value.len() <= maximum && value.bytes().all(|c| (b' '..=b'~').contains(&c))
}
#[derive(Clone, Serialize, Deserialize)]
#[serde(rename_all = "UPPERCASE")]
pub enum HttpMethod {
    Get,
    Post,
    Put,
    Patch,
    Delete,
}
impl HttpMethod {
    fn name(&self) -> &'static str {
        match self {
            Self::Get => "GET",
            Self::Post => "POST",
            Self::Put => "PUT",
            Self::Patch => "PATCH",
            Self::Delete => "DELETE",
        }
    }
}
#[derive(Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct HttpTimeouts {
    pub resolve_ms: u32,
    pub connect_ms: u32,
    pub send_ms: u32,
    pub receive_ms: u32,
    pub budget_ms: u64,
}
#[derive(Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct HttpAuthentication {
    pub secret: String,
    pub header: String,
    pub prefix: String,
}
#[derive(Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct HttpEndpointConfig {
    pub host: String,
    pub port: u16,
    #[serde(default = "default_secure")]
    pub secure: bool,
    #[serde(default)]
    pub allow_insecure_loopback: bool,
    pub method: HttpMethod,
    pub path: String,
    pub content_type: String,
    pub request_bytes: usize,
    pub response_bytes: usize,
    pub header_bytes: u32,
    pub timeouts: HttpTimeouts,
    #[serde(default)]
    pub authentication: Option<HttpAuthentication>,
}
impl HttpEndpointConfig {
    fn validate(&mut self) -> Result<(), CoreError> {
        self.host = self.host.to_ascii_lowercase();
        let literal = self.host.parse::<IpAddr>().ok();
        if let Some(address) = literal {
            if self.host != address.to_string() {
                return Err(invalid());
            }
        } else if self.host.is_empty()
            || self.host.len() > 253
            || self.host.bytes().all(|c| c.is_ascii_digit() || c == b'.')
            || !self.host.split('.').all(|label| {
                !label.is_empty()
                    && label.len() <= 63
                    && !label.starts_with('-')
                    && !label.ends_with('-')
                    && label
                        .bytes()
                        .all(|c| c.is_ascii_alphanumeric() || c == b'-')
            })
        {
            return Err(invalid());
        }
        if !(self.secure
            || self.allow_insecure_loopback && matches!(self.host.as_str(), "127.0.0.1" | "::1"))
        {
            return Err(invalid());
        }
        if self.port == 0
            || !self.path.starts_with('/')
            || self.path.starts_with("//")
            || self.path.len() > 8192
            || !self
                .path
                .bytes()
                .all(|c| (b'!'..=b'~').contains(&c) && c != b'\\' && c != b'#')
            || self.content_type.is_empty()
            || !printable(&self.content_type, 256)
            || self.request_bytes == 0
            || self.request_bytes > u32::MAX as usize
            || self.response_bytes == 0
            || self.header_bytes == 0
            || self.timeouts.budget_ms == 0
        {
            return Err(invalid());
        }
        for value in [
            self.timeouts.resolve_ms,
            self.timeouts.connect_ms,
            self.timeouts.send_ms,
            self.timeouts.receive_ms,
        ] {
            if value == 0 || value > i32::MAX as u32 {
                return Err(invalid());
            }
        }
        if let Some(auth) = &self.authentication
            && (!id(&auth.secret) || !header_name(&auth.header) || !printable(&auth.prefix, 256))
        {
            return Err(invalid());
        }
        Ok(())
    }
    fn destination(&self) -> String {
        let host = if self.host.contains(':') {
            format!("[{}]", self.host)
        } else {
            self.host.clone()
        };
        format!(
            "{}://{}:{}{}",
            if self.secure { "https" } else { "http" },
            host,
            self.port,
            self.path
        )
    }
}
#[derive(Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct NetworkDescription {
    pub binding: String,
    pub method: String,
    pub destination: String,
    pub policy_sha256: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub secret: Option<SecretMetadata>,
}
pub struct NetworkBindings {
    entries: BTreeMap<String, HttpEndpointConfig>,
}
impl NetworkBindings {
    /// Only the trusted launcher supplies this configuration, never a guest body.
    pub fn open(mut entries: BTreeMap<String, HttpEndpointConfig>) -> Result<Arc<Self>, CoreError> {
        for (name, config) in &mut entries {
            if !id(name) {
                return Err(invalid());
            }
            config.validate()?;
        }
        Ok(Arc::new(Self { entries }))
    }
    /// Capture the CURRENT encrypted vault version for a new Host-held plan.
    /// No decryption or network IO happens during this operation.
    pub fn capture(
        &self,
        name: &str,
        vault: Option<&SecretVault>,
    ) -> Result<NetworkPlan, CoreError> {
        let config = self
            .entries
            .get(name)
            .ok_or_else(|| fail("network-binding", "fixed Host binding is not configured"))?
            .clone();
        let secret = config
            .authentication
            .as_ref()
            .map(|auth| {
                vault
                    .ok_or_else(|| {
                        fail(
                            "secret-unavailable",
                            "fixed Host secret store is not configured",
                        )
                    })?
                    .capture(&auth.secret)
            })
            .transpose()?;
        let policy = digest(
            &serde_json::to_vec(&(
                "DIYIME2.NetworkBinding.v1",
                name,
                &config,
                secret.as_ref().map(|s| s.policy()),
            ))
            .map_err(|_| invalid())?,
        )?;
        Ok(NetworkPlan {
            binding: name.into(),
            config,
            secret,
            policy,
        })
    }
}
/// Not deserializable: only a verified fixed binding can produce this plan.
///
/// ```compile_fail
/// let _: diy_ime2_core::isolated_worker::package::network::NetworkPlan =
///     serde_json::from_str("{}").unwrap();
/// ```
pub struct NetworkPlan {
    binding: String,
    config: HttpEndpointConfig,
    secret: Option<SecretReference>,
    policy: String,
}
#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct HttpReply {
    pub status: u16,
    pub body: Vec<u8>,
}
impl NetworkPlan {
    pub(crate) fn exchange_bounded(
        &self,
        body: &[u8],
        remaining_response: usize,
        check: &mut dyn FnMut() -> Result<(), CoreError>,
    ) -> Result<HttpReply, CoreError> {
        let mut config = self.config.clone();
        config.response_bytes = config.response_bytes.min(remaining_response);
        windows::exchange(&config, self.secret.as_ref(), body, check)
    }
    pub fn description(&self) -> NetworkDescription {
        NetworkDescription {
            binding: self.binding.clone(),
            method: self.config.method.name().into(),
            destination: self.config.destination(),
            policy_sha256: self.policy.clone(),
            secret: self.secret.as_ref().map(|s| s.metadata()),
        }
    }
    /// Trusted Host integration supplies a live authority/cancellation check.
    /// Synchronous OS IO retains this owner until it returns and handles close.
    pub fn exchange(
        &self,
        body: &[u8],
        check: &mut dyn FnMut() -> Result<(), CoreError>,
    ) -> Result<HttpReply, CoreError> {
        windows::exchange(&self.config, self.secret.as_ref(), body, check)
    }
}

#[cfg(test)]
mod tests;
