use crate::node::{CoreError, MAX_INPUT_REVISION};
use serde::{Deserialize, Deserializer, Serialize, Serializer, de};
use std::{fmt, str::FromStr, time::Duration};

pub const MAX_SAFE_INTEGER: u64 = 9_007_199_254_740_991;

/// Atomic Core view, not the protocol-v3 wire state (which also owns tokens).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SessionControlState {
    pub revision: u64,
    pub active_plan_version: Option<DecimalId>,
    pub has_prepared: bool,
    pub can_rollback: bool,
    pub latest_input_revision: Option<u64>,
    pub delivery_generation: DecimalId,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct DecimalId(u64);
impl DecimalId {
    pub fn new(value: u64) -> Result<Self, CoreError> {
        if value == 0 {
            Err(CoreError::new(
                "invalid-request",
                "identifier must be positive",
            ))
        } else {
            Ok(Self(value))
        }
    }
    pub fn get(self) -> u64 {
        self.0
    }
    pub fn checked_next(self) -> Result<Self, CoreError> {
        self.0
            .checked_add(1)
            .map(Self)
            .ok_or_else(|| CoreError::new("id-exhausted", "identifier exhausted"))
    }
}
impl fmt::Display for DecimalId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        self.0.fmt(f)
    }
}
impl FromStr for DecimalId {
    type Err = CoreError;
    fn from_str(value: &str) -> Result<Self, Self::Err> {
        if value.is_empty() || value.starts_with('0') || !value.bytes().all(|b| b.is_ascii_digit())
        {
            return Err(CoreError::new(
                "invalid-request",
                "expected canonical positive decimal identifier",
            ));
        }
        value
            .parse::<u64>()
            .map_err(|_| CoreError::new("invalid-request", "identifier exceeds u64"))
            .and_then(Self::new)
    }
}
impl Serialize for DecimalId {
    fn serialize<S: Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
        s.serialize_str(&self.to_string())
    }
}
impl<'de> Deserialize<'de> for DecimalId {
    fn deserialize<D: Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        String::deserialize(d)?.parse().map_err(de::Error::custom)
    }
}

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize)]
#[serde(transparent)]
pub struct SessionEpoch(String);
impl SessionEpoch {
    /// The trusted host generates randomness; validation does not generate an epoch.
    pub fn new(value: impl Into<String>) -> Result<Self, CoreError> {
        let value = value.into();
        if value.len() != 32
            || !value
                .bytes()
                .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
        {
            return Err(CoreError::new(
                "invalid-request",
                "epoch must be 32 lowercase hex digits",
            ));
        }
        Ok(Self(value))
    }
    pub fn as_str(&self) -> &str {
        &self.0
    }
}
impl<'de> Deserialize<'de> for SessionEpoch {
    fn deserialize<D: Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        Self::new(String::deserialize(d)?).map_err(de::Error::custom)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize)]
#[serde(transparent)]
pub struct Milliseconds(u64);
impl Milliseconds {
    pub fn new(value: u64) -> Result<Self, CoreError> {
        if value == 0 || value > MAX_SAFE_INTEGER {
            Err(CoreError::new(
                "invalid-deadline",
                "duration must be a positive safe integer",
            ))
        } else {
            Ok(Self(value))
        }
    }
    pub fn get(self) -> u64 {
        self.0
    }
    pub fn duration(self) -> Duration {
        Duration::from_millis(self.0)
    }
    pub fn checked_deadline(self, now: Duration) -> Result<Duration, CoreError> {
        now.checked_add(self.duration())
            .ok_or_else(|| CoreError::new("invalid-deadline", "deadline overflow"))
    }
}
impl<'de> Deserialize<'de> for Milliseconds {
    fn deserialize<D: Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        Self::new(u64::deserialize(d)?).map_err(de::Error::custom)
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields, try_from = "RunKeyWire")]
pub struct RunKey {
    pub session_epoch: SessionEpoch,
    pub plan_version: DecimalId,
    pub run_id: DecimalId,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub input_revision: Option<u64>,
    pub delivery_generation: DecimalId,
}
#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct RunKeyWire {
    session_epoch: SessionEpoch,
    plan_version: DecimalId,
    run_id: DecimalId,
    #[serde(default, deserialize_with = "crate::json::optional_non_null")]
    input_revision: Option<u64>,
    delivery_generation: DecimalId,
}
impl TryFrom<RunKeyWire> for RunKey {
    type Error = CoreError;
    fn try_from(w: RunKeyWire) -> Result<Self, Self::Error> {
        let key = Self {
            session_epoch: w.session_epoch,
            plan_version: w.plan_version,
            run_id: w.run_id,
            input_revision: w.input_revision,
            delivery_generation: w.delivery_generation,
        };
        key.validate()?;
        Ok(key)
    }
}
impl RunKey {
    pub fn validate(&self) -> Result<(), CoreError> {
        if self
            .input_revision
            .is_some_and(|v| !(1..=MAX_INPUT_REVISION).contains(&v))
        {
            Err(CoreError::new(
                "invalid-runtime-input",
                "invalid input revision",
            ))
        } else {
            Ok(())
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct JobBudget {
    pub node_id: String,
    #[serde(deserialize_with = "required_budget")]
    pub budget_ms: Option<Milliseconds>,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct RunBudgets {
    #[serde(default)]
    pub run_budget_ms: Option<Milliseconds>,
    #[serde(default)]
    pub job_budgets: Vec<JobBudget>,
}
impl RunBudgets {
    /// Decode budget-only host values with the deadline error category, including
    /// JSON type errors. Outer protocol envelopes retain their own validation.
    pub fn from_value(value: serde_json::Value) -> Result<Self, CoreError> {
        let budgets: Self = serde_json::from_value(value)
            .map_err(|error| CoreError::new("invalid-deadline", error.to_string()))?;
        budgets.validate()?;
        Ok(budgets)
    }
    pub fn validate(&self) -> Result<(), CoreError> {
        let mut ids = std::collections::BTreeSet::new();
        if self
            .job_budgets
            .iter()
            .any(|b| b.node_id.trim().is_empty() || !ids.insert(&b.node_id))
        {
            return Err(CoreError::new(
                "invalid-deadline",
                "job budget node IDs must be nonempty and unique",
            ));
        }
        Ok(())
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", try_from = "PolicyWire")]
pub struct ResourcePolicy {
    pub max_active_runs: u64,
    pub max_physical_jobs: u64,
    pub max_observers: u64,
    pub max_queued_jobs: u64,
    pub max_queued_input_bytes: u64,
    pub max_retained_runs: u64,
    pub max_retained_bytes: u64,
    pub max_result_bytes_per_run: u64,
    pub retention_ms: Milliseconds,
    pub cancel_grace_ms: Milliseconds,
    pub max_dedupe_entries: u64,
    pub max_tombstones: u64,
    pub queue_wait_ms: Option<Milliseconds>,
}
fn required_budget<'de, D: Deserializer<'de>>(d: D) -> Result<Option<Milliseconds>, D::Error> {
    Option::deserialize(d)
}
#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct PolicyWire {
    max_active_runs: u64,
    max_physical_jobs: u64,
    max_observers: u64,
    max_queued_jobs: u64,
    max_queued_input_bytes: u64,
    max_retained_runs: u64,
    max_retained_bytes: u64,
    max_result_bytes_per_run: u64,
    retention_ms: Milliseconds,
    cancel_grace_ms: Milliseconds,
    max_dedupe_entries: u64,
    max_tombstones: u64,
    #[serde(deserialize_with = "required_budget")]
    queue_wait_ms: Option<Milliseconds>,
}
impl TryFrom<PolicyWire> for ResourcePolicy {
    type Error = CoreError;
    fn try_from(p: PolicyWire) -> Result<Self, Self::Error> {
        let result = Self {
            max_active_runs: p.max_active_runs,
            max_physical_jobs: p.max_physical_jobs,
            max_observers: p.max_observers,
            max_queued_jobs: p.max_queued_jobs,
            max_queued_input_bytes: p.max_queued_input_bytes,
            max_retained_runs: p.max_retained_runs,
            max_retained_bytes: p.max_retained_bytes,
            max_result_bytes_per_run: p.max_result_bytes_per_run,
            retention_ms: p.retention_ms,
            cancel_grace_ms: p.cancel_grace_ms,
            max_dedupe_entries: p.max_dedupe_entries,
            max_tombstones: p.max_tombstones,
            queue_wait_ms: p.queue_wait_ms,
        };
        result.validate()?;
        Ok(result)
    }
}
impl ResourcePolicy {
    pub fn from_value(value: serde_json::Value) -> Result<Self, CoreError> {
        serde_json::from_value(value)
            .map_err(|error| CoreError::new("invalid-resource-policy", error.to_string()))
    }
    pub fn validate(&self) -> Result<(), CoreError> {
        let positive = [
            self.max_active_runs,
            self.max_physical_jobs,
            self.max_observers,
            self.max_queued_input_bytes,
            self.max_retained_runs,
            self.max_retained_bytes,
            self.max_result_bytes_per_run,
            self.max_dedupe_entries,
        ];
        if positive.iter().any(|v| *v == 0 || *v > MAX_SAFE_INTEGER)
            || self.max_queued_jobs > MAX_SAFE_INTEGER
            || self.max_tombstones > MAX_SAFE_INTEGER
            || self.max_result_bytes_per_run > self.max_retained_bytes
        {
            Err(CoreError::new(
                "invalid-resource-policy",
                "resource limits must be safe integers and result reservation must fit retention budget",
            ))
        } else {
            Ok(())
        }
    }
}
