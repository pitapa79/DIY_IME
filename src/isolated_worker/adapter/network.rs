//! Immutable Host network proofs; no IO occurs while constructing consent.
use super::{fail, sha256};
use crate::isolated_worker::package::{
    network::{HttpEndpointConfig, NetworkBindings, NetworkDescription, NetworkPlan},
    secrets::SecretVault,
};
use crate::{execution_control::CancellationToken, node::CoreError};
use serde::{Deserialize, Serialize};
use std::{
    collections::{BTreeMap, BTreeSet},
    sync::Arc,
    time::{Duration, Instant},
};

#[derive(Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct NetworkLimits {
    pub max_calls: u32,
    pub max_request_bytes: usize,
    pub max_response_bytes: usize,
    pub max_continuation_bytes: usize,
    pub budget_ms: u64,
}
impl NetworkLimits {
    fn validate(&self) -> Result<(), CoreError> {
        if self.max_calls == 0
            || self.max_request_bytes == 0
            || self.max_response_bytes == 0
            || self.max_continuation_bytes == 0
            || self.budget_ms == 0
            || Instant::now()
                .checked_add(Duration::from_millis(self.budget_ms))
                .is_none()
        {
            return Err(fail(
                "network-config",
                "invalid selected continuation limits",
            ));
        }
        Ok(())
    }
}
#[derive(Clone, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct NetworkConfiguration {
    pub bindings: BTreeMap<String, HttpEndpointConfig>,
    pub limits: NetworkLimits,
}
pub struct NetworkProvider {
    bindings: Arc<NetworkBindings>,
    limits: NetworkLimits,
}
impl NetworkProvider {
    pub fn open(config: NetworkConfiguration) -> Result<Arc<Self>, CoreError> {
        config.limits.validate()?;
        Ok(Arc::new(Self {
            bindings: NetworkBindings::open(config.bindings)?,
            limits: config.limits,
        }))
    }
    pub(in crate::isolated_worker) fn capture(
        self: &Arc<Self>,
        capabilities: &[(String, String)],
        vault: Option<&SecretVault>,
    ) -> Result<Option<NetworkPlans>, CoreError> {
        let network: BTreeSet<_> = capabilities
            .iter()
            .filter(|(id, _)| id == "network.request")
            .map(|(_, binding)| binding.clone())
            .collect();
        let secrets: BTreeSet<_> = capabilities
            .iter()
            .filter(|(id, _)| id == "secret.use")
            .map(|(_, binding)| binding.clone())
            .collect();
        if !secrets.is_subset(&network) {
            return Err(fail(
                "package-capability",
                "secret use requires the same network binding",
            ));
        }
        if network.is_empty() {
            return Ok(None);
        }
        let mut plans = BTreeMap::new();
        for binding in network {
            let plan = self.bindings.capture(&binding, vault)?;
            if plan.description().secret.is_some() != secrets.contains(&binding) {
                return Err(fail(
                    "package-capability",
                    "secret declaration differs from fixed endpoint authentication",
                ));
            }
            plans.insert(binding, Arc::new(plan));
        }
        let identity = sha256(
            &serde_json::to_vec(&(
                &self.limits,
                plans.values().map(|p| p.description()).collect::<Vec<_>>(),
            ))
            .map_err(|_| fail("network-config", "network proof encoding failed"))?,
        )?;
        Ok(Some(NetworkPlans {
            provider: self.clone(),
            plans,
            identity,
        }))
    }
}
#[derive(Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct NetworkUseDescription {
    #[serde(flatten)]
    pub endpoint: NetworkDescription,
    pub limits: NetworkLimits,
}
#[derive(Clone)]
pub(in crate::isolated_worker) struct NetworkPlans {
    provider: Arc<NetworkProvider>,
    plans: BTreeMap<String, Arc<NetworkPlan>>,
    pub identity: String,
}
impl NetworkPlans {
    pub fn descriptions(&self) -> Vec<NetworkUseDescription> {
        self.plans
            .values()
            .map(|p| NetworkUseDescription {
                endpoint: p.description(),
                limits: self.provider.limits.clone(),
            })
            .collect()
    }
    pub fn same_proof(&self, other: &Self) -> bool {
        Arc::ptr_eq(&self.provider, &other.provider) && self.identity == other.identity
    }
    pub(super) fn execution(&self) -> Result<NetworkExecution<'_>, CoreError> {
        Ok(NetworkExecution {
            plans: self,
            calls: 0,
            request_bytes: 0,
            response_bytes: 0,
            deadline: Instant::now()
                .checked_add(Duration::from_millis(self.provider.limits.budget_ms))
                .ok_or_else(|| fail("network-config", "continuation budget unavailable"))?,
        })
    }
}
pub(super) struct NetworkExecution<'a> {
    plans: &'a NetworkPlans,
    calls: u32,
    request_bytes: usize,
    response_bytes: usize,
    deadline: Instant,
}
impl NetworkExecution<'_> {
    pub fn check(&self, cancel: &CancellationToken) -> Result<(), CoreError> {
        if cancel.is_cancelled() {
            return Err(fail("worker-cancelled", "network invocation cancelled"));
        }
        if Instant::now() >= self.deadline {
            return Err(fail("network-deadline", "continuation budget exceeded"));
        }
        Ok(())
    }
    pub fn exchange(
        &mut self,
        call: super::wire::NetworkCall,
        cancel: &CancellationToken,
        check: &mut dyn FnMut() -> Result<(), CoreError>,
    ) -> Result<
        (
            serde_json::Value,
            crate::isolated_worker::package::network::HttpReply,
        ),
        CoreError,
    > {
        self.check(cancel)?;
        check()?;
        let limits = &self.plans.provider.limits;
        let continuation_bytes = serde_json::to_vec(&call.continuation)
            .map_err(|_| fail("worker-reply", "continuation encoding failed"))?
            .len();
        if self.calls >= limits.max_calls
            || continuation_bytes > limits.max_continuation_bytes
            || call.body.len() > limits.max_request_bytes.saturating_sub(self.request_bytes)
        {
            return Err(fail(
                "network-limit",
                "continuation exceeds selected aggregate limits",
            ));
        }
        let plan = self.plans.plans.get(&call.binding).ok_or_else(|| {
            fail(
                "package-capability",
                "network binding is not part of this verified proof",
            )
        })?;
        self.calls += 1;
        self.request_bytes += call.body.len();
        let remaining = limits
            .max_response_bytes
            .saturating_sub(self.response_bytes);
        let reply = plan.exchange_bounded(&call.body, remaining, &mut || {
            self.check(cancel)?;
            check()
        })?;
        self.check(cancel)?;
        check()?;
        self.response_bytes += reply.body.len();
        Ok((call.continuation, reply))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    fn config() -> NetworkConfiguration {
        serde_json::from_value(json!({"bindings":{"api":{"host":"127.0.0.1","port":9,"secure":false,"allowInsecureLoopback":true,"method":"POST","path":"/fixed","contentType":"application/octet-stream","requestBytes":4096,"responseBytes":4096,"headerBytes":4096,"timeouts":{"resolveMs":1000,"connectMs":1000,"sendMs":1000,"receiveMs":1000,"budgetMs":2000}}},"limits":{"maxCalls":2,"maxRequestBytes":4096,"maxResponseBytes":4096,"maxContinuationBytes":1024,"budgetMs":10000}})).unwrap()
    }
    #[test]
    fn fixed_provider_requires_matching_declared_bindings_and_no_ambient_secret_use() {
        let provider = NetworkProvider::open(config()).unwrap();
        assert!(provider.capture(&[], None).unwrap().is_none());
        let network = vec![("network.request".into(), "api".into())];
        let one = provider.capture(&network, None).unwrap().unwrap();
        let two = provider.capture(&network, None).unwrap().unwrap();
        assert!(one.same_proof(&two));
        assert!(one.descriptions()[0].endpoint.secret.is_none());
        assert!(
            !one.same_proof(
                &NetworkProvider::open(config())
                    .unwrap()
                    .capture(&network, None)
                    .unwrap()
                    .unwrap()
            )
        );
        for capabilities in [
            vec![("secret.use".into(), "api".into())],
            vec![("network.request".into(), "unknown".into())],
            vec![
                ("network.request".into(), "api".into()),
                ("secret.use".into(), "api".into()),
            ],
            vec![
                ("network.request".into(), "api".into()),
                ("secret.use".into(), "other".into()),
            ],
        ] {
            assert!(provider.capture(&capabilities, None).is_err());
        }
    }
    #[test]
    fn limits_and_selected_endpoint_are_part_of_the_execution_proof_and_invalid_limits_fail_early()
    {
        let capabilities = vec![("network.request".into(), "api".into())];
        let original = NetworkProvider::open(config())
            .unwrap()
            .capture(&capabilities, None)
            .unwrap()
            .unwrap()
            .identity;
        for index in 0..6 {
            let mut cfg = config();
            match index {
                0 => cfg.limits.max_calls += 1,
                1 => cfg.limits.max_request_bytes += 1,
                2 => cfg.limits.max_response_bytes += 1,
                3 => cfg.limits.max_continuation_bytes += 1,
                4 => cfg.limits.budget_ms += 1,
                _ => cfg.bindings.get_mut("api").unwrap().path = "/other".into(),
            };
            assert_ne!(
                original,
                NetworkProvider::open(cfg)
                    .unwrap()
                    .capture(&capabilities, None)
                    .unwrap()
                    .unwrap()
                    .identity
            );
        }
        for index in 0..5 {
            let mut cfg = config();
            match index {
                0 => cfg.limits.max_calls = 0,
                1 => cfg.limits.max_request_bytes = 0,
                2 => cfg.limits.max_response_bytes = 0,
                3 => cfg.limits.max_continuation_bytes = 0,
                _ => cfg.limits.budget_ms = 0,
            };
            assert!(NetworkProvider::open(cfg).is_err());
        }
        let provider = NetworkProvider::open(config()).unwrap();
        let plans = provider.capture(&capabilities, None).unwrap().unwrap();
        let mut execution = plans.execution().unwrap();
        let cancel = CancellationToken::default();
        cancel.cancel();
        assert!(
            execution
                .exchange(
                    super::super::wire::NetworkCall {
                        binding: "api".into(),
                        body: vec![],
                        continuation: json!(null)
                    },
                    &cancel,
                    &mut || panic!("cancelled invocation checked authority or started IO")
                )
                .is_err()
        );
    }
}
