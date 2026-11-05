//! 初期の Rust 組込ノード API。外部プラグイン用 ABI ではありません。
use std::{collections::BTreeMap, error::Error, fmt, sync::Arc};

use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

use crate::{Candidate, Packet};

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CoreError {
    pub code: String,
    pub message: String,
}

impl CoreError {
    pub fn new(code: impl Into<String>, message: impl Into<String>) -> Self {
        Self {
            code: code.into(),
            message: message.into(),
        }
    }
}
impl fmt::Display for CoreError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}: {}", self.code, self.message)
    }
}
impl Error for CoreError {}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum RootConstraint {
    Any,
    Text,
    Sequence,
    Choice,
}

impl RootConstraint {
    pub fn accepts(self, candidate: &Candidate) -> bool {
        matches!(
            (self, candidate),
            (Self::Any, _)
                | (Self::Text, Candidate::Text { .. })
                | (Self::Sequence, Candidate::Sequence { .. })
                | (Self::Choice, Candidate::Choice { .. })
        )
    }
}

/// 型識別子は文章候補以外を追加できる形で保持します。
/// 現行ランタイムが実装するのは text-packet / version 1 のみです。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct DataType {
    pub id: String,
    pub version: u32,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Port {
    pub id: String,
    pub data_type: DataType,
    pub language: Option<String>,
    pub root: RootConstraint,
}

impl Port {
    pub fn packet(id: impl Into<String>) -> Self {
        Self {
            id: id.into(),
            data_type: DataType {
                id: "text-packet".into(),
                version: 1,
            },
            language: None,
            root: RootConstraint::Any,
        }
    }
    pub fn with_language(mut self, language: impl Into<String>) -> Self {
        self.language = Some(language.into());
        self
    }
    pub fn with_root(mut self, root: RootConstraint) -> Self {
        self.root = root;
        self
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct InputPort {
    pub port: Port,
    pub required: bool,
}

impl InputPort {
    pub fn required(port: Port) -> Self {
        Self {
            port,
            required: true,
        }
    }
    pub fn optional(port: Port) -> Self {
        Self {
            port,
            required: false,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct NodeDefinition {
    pub kind: String,
    pub version: u32,
    pub inputs: Vec<InputPort>,
    pub outputs: Vec<Port>,
    /// エディター向けの設定宣言。実行前の最終検査は Node::validate_config が行います。
    pub config_schema: Value,
    pub default_config: Value,
}

impl NodeDefinition {
    pub fn new(
        kind: impl Into<String>,
        version: u32,
        inputs: Vec<InputPort>,
        outputs: Vec<Port>,
    ) -> Self {
        Self {
            kind: kind.into(),
            version,
            inputs,
            outputs,
            config_schema: json!({"type":"object", "additionalProperties":false}),
            default_config: json!({}),
        }
    }
}

/// 分岐先では同じ Packet を読み取り専用で共有します。
#[derive(Debug, Clone, PartialEq)]
pub enum OutputValue {
    Data(Arc<Packet>),
    NoOutput,
}

pub type Inputs = BTreeMap<String, OutputValue>;
pub type Outputs = BTreeMap<String, OutputValue>;

pub const MAX_INPUT_REVISION: u64 = 9_007_199_254_740_991;
pub const MAX_INPUT_CODE_POINTS: usize = 4096;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum InputMode {
    Compose,
    Finalize,
}

/// 1回の実行へ渡す文章のスナップショット。計画やノード設定には含めない。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RuntimeInput {
    pub revision: u64,
    pub text: String,
    pub mode: InputMode,
}

impl RuntimeInput {
    pub fn validate(&self) -> Result<(), CoreError> {
        if !(1..=MAX_INPUT_REVISION).contains(&self.revision)
            || self.text.chars().count() > MAX_INPUT_CODE_POINTS
        {
            return Err(CoreError::new(
                "invalid-runtime-input",
                "revision must be a positive safe integer and text must contain at most 4096 code points",
            ));
        }
        Ok(())
    }
    pub fn from_value(value: Value) -> Result<Self, CoreError> {
        let input: Self = serde_json::from_value(value)
            .map_err(|error| CoreError::new("invalid-runtime-input", error.to_string()))?;
        input.validate()?;
        Ok(input)
    }
}

#[derive(Debug, Clone, Copy)]
pub struct RunContext<'a> {
    pub run_id: u64,
    pub plan_version: u64,
    pub node_id: &'a str,
    pub input: Option<&'a RuntimeInput>,
}

/// 準備時だけ渡されるHost資源の取得面。実行中には渡さない。
/// 実装は1 prepare単位で使い、同じ対象には固定した所有bytesを返す。
/// max_bytesを超えた場合は切り捨てずエラーにする。ファイルの解釈はNode側の責務。
pub trait PrepareResources {
    fn read(&mut self, reference: &str, max_bytes: usize) -> Result<Arc<[u8]>, CoreError>;
}

pub(crate) struct NoPrepareResources;
impl PrepareResources for NoPrepareResources {
    fn read(&mut self, _: &str, _: usize) -> Result<Arc<[u8]>, CoreError> {
        Err(CoreError::new(
            "resource-unavailable",
            "no resource source was explicitly supplied",
        ))
    }
}

/// 計画が所有する準備済みの実行。内部可変性を禁止するsandboxではない。
/// 設定と読み込んだ資源を所有し、宣言済みポートへの出力だけを返す。
pub trait PreparedNode: Send + Sync {
    fn execute(&self, context: RunContext<'_>, inputs: &Inputs) -> Result<Outputs, CoreError>;
}

/// execute は1実行の全入力が解決してから呼ばれます。
/// 宣言したすべての出力に Data または明示的な NoOutput を返します。
pub trait Node: Send + Sync {
    fn definition(&self) -> NodeDefinition;
    fn validate_config(&self, config: &Value) -> Result<(), CoreError> {
        if config == &json!({}) {
            Ok(())
        } else {
            Err(CoreError::new(
                "invalid-config",
                "this node accepts only an empty object",
            ))
        }
    }
    /// 構造検査後にノードインスタンスごとに1回呼ばれる。
    /// Noneは既存executeを使う。必要な資源を取得できない場合はErrを返す。
    fn prepare(
        &self,
        _config: &Value,
        _resources: &mut dyn PrepareResources,
    ) -> Result<Option<Arc<dyn PreparedNode>>, CoreError> {
        Ok(None)
    }
    fn execute(
        &self,
        context: RunContext<'_>,
        inputs: &Inputs,
        config: &Value,
    ) -> Result<Outputs, CoreError>;
}

#[derive(Clone)]
pub(crate) struct RegisteredNode {
    pub definition: NodeDefinition,
    pub implementation: RegisteredImplementation,
}

#[derive(Clone)]
pub(crate) enum RegisteredImplementation {
    Legacy(Arc<dyn Node>),
    ErrorAware {
        node: Arc<dyn crate::error_input::ErrorAwareNode>,
        error_inputs: Vec<String>,
    },
    Deferred {
        node: Arc<dyn crate::execution_control::DeferredNode>,
        error_inputs: Vec<String>,
    },
    Control {
        node: Arc<dyn crate::execution_control::ControlNode>,
        error_inputs: Vec<String>,
    },
}
impl RegisteredImplementation {
    pub(crate) fn validate_config(&self, config: &Value) -> Result<(), CoreError> {
        match self {
            Self::Legacy(node) => node.validate_config(config),
            Self::ErrorAware { node, .. } => node.validate_config(config),
            Self::Deferred { node, .. } => node.validate_config(config),
            Self::Control { node, .. } => node.validate_config(config),
        }
    }
    pub(crate) fn receives_error(&self, input: &str) -> bool {
        match self {
            Self::Legacy(_) => false,
            Self::ErrorAware { error_inputs, .. }
            | Self::Deferred { error_inputs, .. }
            | Self::Control { error_inputs, .. } => error_inputs.iter().any(|id| id == input),
        }
    }
}

#[derive(Default)]
pub struct Registry {
    pub(crate) nodes: BTreeMap<(String, u32), RegisteredNode>,
}

impl Registry {
    pub fn new() -> Self {
        Self::default()
    }
    pub fn definitions(&self) -> impl Iterator<Item = &NodeDefinition> {
        self.nodes
            .values()
            .filter(|node| matches!(node.implementation, RegisteredImplementation::Legacy(_)))
            .map(|node| &node.definition)
    }
    pub fn error_aware_definitions(
        &self,
    ) -> impl Iterator<Item = crate::error_input::ErrorAwareDefinition> {
        self.nodes
            .values()
            .filter_map(|registered| match &registered.implementation {
                RegisteredImplementation::Legacy(_)
                | RegisteredImplementation::Deferred { .. }
                | RegisteredImplementation::Control { .. } => None,
                RegisteredImplementation::ErrorAware { error_inputs, .. } => {
                    Some(crate::error_input::ErrorAwareDefinition {
                        version: 1,
                        definition: registered.definition.clone(),
                        error_inputs: error_inputs.clone(),
                    })
                }
            })
    }
    pub fn register_error_aware(
        &mut self,
        node: impl crate::error_input::ErrorAwareNode + 'static,
    ) -> Result<(), CoreError> {
        let definition = node.definition();
        let error_inputs = node.error_inputs();
        validate_error_inputs(&definition, &error_inputs)?;
        self.register_implementation(
            definition,
            RegisteredImplementation::ErrorAware {
                node: Arc::new(node),
                error_inputs,
            },
        )
    }
    pub fn register(&mut self, node: impl Node + 'static) -> Result<(), CoreError> {
        self.register_implementation(
            node.definition(),
            RegisteredImplementation::Legacy(Arc::new(node)),
        )
    }
    pub fn execution_definitions(
        &self,
    ) -> impl Iterator<Item = crate::execution_control::ExecutionDefinition> {
        use crate::execution_control::{ExecutionDefinition, ExecutionKind};
        self.nodes.values().map(|registered| {
            let (execution_kind, error_inputs) = match &registered.implementation {
                RegisteredImplementation::Legacy(_) => (ExecutionKind::Sync, vec![]),
                RegisteredImplementation::ErrorAware { error_inputs, .. } => {
                    (ExecutionKind::Sync, error_inputs.clone())
                }
                RegisteredImplementation::Deferred { error_inputs, .. } => {
                    (ExecutionKind::Deferred, error_inputs.clone())
                }
                RegisteredImplementation::Control { error_inputs, .. } => {
                    (ExecutionKind::Control, error_inputs.clone())
                }
            };
            ExecutionDefinition {
                version: 2,
                execution_kind,
                definition: registered.definition.clone(),
                error_inputs,
            }
        })
    }
    pub fn register_deferred(
        &mut self,
        node: impl crate::execution_control::DeferredNode + 'static,
    ) -> Result<(), CoreError> {
        let definition = node.definition();
        let error_inputs = node.error_inputs();
        validate_error_inputs(&definition, &error_inputs)?;
        self.register_implementation(
            definition,
            RegisteredImplementation::Deferred {
                node: Arc::new(node),
                error_inputs,
            },
        )
    }
    pub fn register_control(
        &mut self,
        node: impl crate::execution_control::ControlNode + 'static,
    ) -> Result<(), CoreError> {
        let definition = node.definition();
        let error_inputs = node.error_inputs();
        validate_error_inputs(&definition, &error_inputs)?;
        self.register_implementation(
            definition,
            RegisteredImplementation::Control {
                node: Arc::new(node),
                error_inputs,
            },
        )
    }
    fn register_implementation(
        &mut self,
        definition: NodeDefinition,
        implementation: RegisteredImplementation,
    ) -> Result<(), CoreError> {
        let key = (definition.kind.clone(), definition.version);
        if definition.kind.trim().is_empty() || definition.version == 0 {
            return Err(CoreError::new(
                "invalid-definition",
                "node kind and version are required",
            ));
        }
        if self.nodes.contains_key(&key) {
            return Err(CoreError::new(
                "duplicate-kind",
                format!("{}@{}", key.0, key.1),
            ));
        }
        for ports in [
            definition
                .inputs
                .iter()
                .map(|input| &input.port)
                .collect::<Vec<_>>(),
            definition.outputs.iter().collect(),
        ] {
            let mut ids = std::collections::BTreeSet::new();
            for port in ports {
                if port.id.trim().is_empty() || !ids.insert(&port.id) {
                    return Err(CoreError::new(
                        "invalid-definition",
                        "port IDs must be nonempty and unique within each direction",
                    ));
                }
                if port.data_type
                    != (DataType {
                        id: "text-packet".into(),
                        version: 1,
                    })
                {
                    return Err(CoreError::new(
                        "unsupported-data-type",
                        format!("{}:{}", port.data_type.id, port.data_type.version),
                    ));
                }
                if port
                    .language
                    .as_ref()
                    .is_some_and(|language| language.trim().is_empty())
                {
                    return Err(CoreError::new(
                        "invalid-definition",
                        "language must be nonempty or omitted",
                    ));
                }
            }
        }
        implementation.validate_config(&definition.default_config)?;
        self.nodes.insert(
            key,
            RegisteredNode {
                definition,
                implementation,
            },
        );
        Ok(())
    }
}

fn validate_error_inputs(definition: &NodeDefinition, ids: &[String]) -> Result<(), CoreError> {
    let mut seen = std::collections::BTreeSet::new();
    if ids
        .iter()
        .any(|id| !seen.insert(id) || !definition.inputs.iter().any(|input| &input.port.id == id))
    {
        return Err(CoreError::new(
            "invalid-definition",
            "error input IDs must exist and be unique",
        ));
    }
    Ok(())
}
