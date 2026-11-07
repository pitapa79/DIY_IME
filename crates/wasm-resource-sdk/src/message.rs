use crate::{Error, Resource, ResourceDescriptor};
use serde::{
    Deserialize, Serialize,
    de::{MapAccess, Visitor},
};
use std::{
    collections::{BTreeMap, BTreeSet},
    fmt,
};

/// Logical references and guest-visible claims; the native connection owns the
/// actual resources. Duplicate names and duplicate IDs are not aliases.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize)]
#[serde(transparent)]
pub struct ResourceMap(pub BTreeMap<String, ResourceDescriptor>);
impl<'de> Deserialize<'de> for ResourceMap {
    fn deserialize<D: serde::Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        struct Unique;
        impl<'de> Visitor<'de> for Unique {
            type Value = ResourceMap;
            fn expecting(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
                f.write_str("unique logical resource references")
            }
            fn visit_map<A: MapAccess<'de>>(self, mut map: A) -> Result<Self::Value, A::Error> {
                let mut values = BTreeMap::new();
                while let Some((name, value)) = map.next_entry::<String, ResourceDescriptor>()? {
                    if values.insert(name, value).is_some() {
                        return Err(serde::de::Error::custom("duplicate resource reference"));
                    }
                }
                Ok(ResourceMap(values))
            }
        }
        d.deserialize_map(Unique)
    }
}
impl ResourceMap {
    pub fn validate(&self) -> Result<(), Error> {
        if self.0.is_empty() {
            return Err(Error::InvalidMessage);
        }
        let mut ids = BTreeSet::new();
        for (name, descriptor) in &self.0 {
            if name.is_empty() || name.len() > 4096 || !ids.insert(descriptor.id) {
                return Err(Error::InvalidMessage);
            }
            Resource::try_from(*descriptor)?;
        }
        Ok(())
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct NodeIdentity {
    pub kind: String,
    pub version: u32,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub enum Mode {
    Compose,
    Finalize,
}
fn present_mode<'de, D: serde::Deserializer<'de>>(d: D) -> Result<Option<Mode>, D::Error> {
    Mode::deserialize(d).map(Some)
}

#[derive(Clone, Debug, PartialEq, Eq, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct ArtifactDescriptor {
    pub resource: ResourceDescriptor,
    pub format: String,
    pub version: u32,
}
fn valid_format(format: &str, version: u32) -> bool {
    version > 0
        && !format.is_empty()
        && format.len() <= 128
        && format
            .bytes()
            .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b"._-".contains(&b))
}
impl ArtifactDescriptor {
    pub fn validate(&self) -> Result<(), Error> {
        if self.resource.length == 0 || !valid_format(&self.format, self.version) {
            return Err(Error::InvalidMessage);
        }
        Resource::try_from(self.resource)?;
        Ok(())
    }
}

/// These extra operations do not replace the existing v1 describe/validate/
/// prepare/execute wire. Only an explicitly verified compatible route selects them.
#[derive(Clone, Debug, PartialEq, Deserialize, Serialize)]
#[serde(tag = "method", rename_all = "camelCase", deny_unknown_fields)]
pub enum Operation<C, I> {
    PrepareArtifact {
        config: C,
        resources: ResourceMap,
    },
    ExecutePrepared {
        config: C,
        artifact: ArtifactDescriptor,
        inputs: I,
        #[serde(
            default,
            skip_serializing_if = "Option::is_none",
            deserialize_with = "present_mode"
        )]
        mode: Option<Mode>,
    },
}
#[derive(Clone, Debug, PartialEq, Deserialize, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Request<C, I> {
    pub api_version: u32,
    pub node: NodeIdentity,
    pub operation: Operation<C, I>,
}
impl<C, I> Request<C, I> {
    pub fn validate(&self) -> Result<(), Error> {
        if self.api_version != 1
            || self.node.kind.is_empty()
            || self.node.kind.len() > 256
            || self.node.version == 0
        {
            return Err(Error::InvalidMessage);
        }
        match &self.operation {
            Operation::PrepareArtifact { resources, .. } => resources.validate(),
            Operation::ExecutePrepared { artifact, .. } => artifact.validate(),
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Deserialize, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct PreparationAcknowledgement {
    pub valid: bool,
    pub prepared: bool,
    pub artifact_format: String,
    pub artifact_version: u32,
    pub artifact_bytes: u64,
}
impl PreparationAcknowledgement {
    /// Call only after the Node has validated all originals and built all bytes.
    pub fn new(format: String, version: u32, bytes: u64) -> Result<Self, Error> {
        let result = Self {
            valid: true,
            prepared: true,
            artifact_format: format,
            artifact_version: version,
            artifact_bytes: bytes,
        };
        result.validate()?;
        Ok(result)
    }
    pub fn validate(&self) -> Result<(), Error> {
        if !self.valid
            || !self.prepared
            || self.artifact_bytes == 0
            || self.artifact_bytes > u32::MAX as u64
            || !valid_format(&self.artifact_format, self.artifact_version)
        {
            Err(Error::InvalidMessage)
        } else {
            Ok(())
        }
    }
}
