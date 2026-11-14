//! Explicit Host-only experiment. No default eligibility or security-policy adoption.
use super::*;

pub(in crate::isolated_worker) type Stages = Vec<(String, WasmNode, Value)>;

pub(in crate::isolated_worker) struct Owners {
    owners: Vec<retained::Owner>,
    sizes: Vec<usize>,
    experimental: bool,
}

impl Owners {
    pub(in crate::isolated_worker) fn evaluation_group_size() -> Result<Option<usize>, CoreError> {
        match std::env::var_os("DIY_EXPERIMENT_REVIEWED_PRIMITIVE_RETENTION").as_deref() {
            None => Ok(None),
            Some(s) if s == std::ffi::OsStr::new("0") => Ok(None),
            Some(s) if s == std::ffi::OsStr::new("separate") => Ok(Some(1)),
            Some(s) if s == std::ffi::OsStr::new("grouped") => Ok(Some(4)),
            _ => Err(fail(
                "retained-evaluation",
                "invalid explicit evaluation mode",
            )),
        }
    }

    pub(in crate::isolated_worker) fn fixed(owner: retained::Owner) -> Self {
        Self {
            owners: vec![owner],
            sizes: vec![4],
            experimental: false,
        }
    }

    pub(in crate::isolated_worker) fn install_evaluation(
        host: ExecutionHost,
        controller: GuardedController,
        groups: Vec<Stages>,
    ) -> Result<Self, CoreError> {
        if groups.is_empty()
            || groups.len() > 20
            || host.policy().max_physical_jobs < groups.len() as u64 + 1
            || groups.iter().map(Vec::len).sum::<usize>() > 20
        {
            return Err(fail(
                "retained-evaluation",
                "insufficient bounded lifetime/input capacity",
            ));
        }
        let mut result = Self {
            owners: Vec::new(),
            sizes: Vec::new(),
            experimental: true,
        };
        for stages in groups {
            result.sizes.push(stages.len());
            let owner =
                retained::Owner::install_evaluation(host.clone(), controller.clone(), stages)?
                    .ok_or_else(|| {
                        fail(
                            "retained-evaluation",
                            "unsupported retained evaluation policy",
                        )
                    })?;
            result.owners.push(owner);
        }
        Ok(result)
    }

    pub(in crate::isolated_worker) fn physical_jobs(&self) -> u64 {
        self.owners.iter().map(|o| o.physical_jobs()).sum()
    }
    pub(in crate::isolated_worker) fn start(&self) {
        for owner in &self.owners {
            owner.start();
        }
    }
    pub(in crate::isolated_worker) fn witness_terminal(&self, c: &GuardedController) {
        for owner in &self.owners {
            owner.witness_terminal(c);
        }
    }
    pub(in crate::isolated_worker) fn ready(&self) -> bool {
        self.owners.iter().all(|o| o.ready())
    }
    pub(in crate::isolated_worker) fn process_id(&self) -> Option<u64> {
        if self.owners.len() == 1 {
            self.owners[0].process_id()
        } else {
            None
        }
    }
    pub(in crate::isolated_worker) fn generation(&self) -> u64 {
        self.owners.iter().map(|o| o.generation()).sum()
    }
    pub(in crate::isolated_worker) fn decoded_replies(&self) -> u64 {
        self.owners.iter().map(|o| o.decoded_replies()).sum()
    }
    pub(in crate::isolated_worker) fn active_input(&self) -> bool {
        self.owners.iter().any(|o| o.active_input())
    }
    pub(in crate::isolated_worker) fn group_sizes(&self) -> &[usize] {
        &self.sizes
    }
    pub(in crate::isolated_worker) fn experimental(&self) -> bool {
        self.experimental
    }
    pub(in crate::isolated_worker) fn process_ids(&self) -> Vec<u64> {
        self.owners.iter().filter_map(|o| o.process_id()).collect()
    }
}

impl Drop for Owners {
    fn drop(&mut self) {
        // Cancel every group first, then each original owner joins and cleans up.
        for owner in &self.owners {
            owner.close();
        }
    }
}

impl WasmNode {
    pub(in crate::isolated_worker) fn retention_evaluation_eligible(&self) -> bool {
        // A classification, package name or guest self-report grants nothing.
        // These exact reviewed official bytes are already pinned by distribution.
        self.artifact.0.identity.rsplit(':').next()
            == Some("105c0ceb2854dba27a747eae5faa6e393bb99a88ce5ab3048a468fa9add88134")
            && matches!(
                self.definition.kind.as_str(),
                "experimental.packet-passthrough"
                    | "experimental.text-trim"
                    | "experimental.text-case"
                    | "demo.identity"
                    | "text.romaji-to-hiragana"
                    | "experimental.text-concat"
            )
            && self.definition.version == 1
            && self.network.is_none()
            && self.dictionary.is_none()
            && self.prepared_dictionary.get().is_none()
    }
}
