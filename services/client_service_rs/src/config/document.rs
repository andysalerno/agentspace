use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};

use crate::{
    config::value::{ConfigValue, SecretName},
    models::{ConnectionApiFlavor, GatewayType, HarnessName},
};

/// The only supported API version in this schema generation.
pub const API_VERSION: &str = "agentspace.dev/v1alpha1";
/// Aggregate document kind that can hold the entire system configuration.
pub const KIND_AGGREGATE: &str = "AgentSpaceConfig";
/// Default aggregate metadata name used when the UI regenerates the source.
pub const DEFAULT_METADATA_NAME: &str = "local";
/// Metadata block shared by every manifest form.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct Metadata {
    pub name: String,
}

/// A declared secret. YAML declares names and descriptions only, never values.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct SecretDeclaration {
    pub name: SecretName,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
}

/// Per-harness kernel defaults.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct KernelConfig {
    pub harness: HarnessName,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub env: Option<BTreeMap<String, ConfigValue<String>>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub env_text: Option<String>,
}

/// A model endpoint connection.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Connection {
    pub id: String,
    pub name: String,
    pub url: ConfigValue<String>,
    pub api_flavor: ConnectionApiFlavor,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub api_key: Option<ConfigValue<String>>,
}

/// A user skill with an inline file tree.
///
/// Path-based sources are config-set authoring syntax expanded by the loader;
/// the stored document always contains the resolved inline file map.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct Skill {
    pub id: String,
    pub files: BTreeMap<String, String>,
}

/// An agent definition. Workspace mounts are runtime state and excluded.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Agent {
    pub id: String,
    pub name: String,
    pub harness: HarnessName,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub connection: Option<String>,
    pub system_prompt: ConfigValue<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub skills: Vec<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub env: Option<BTreeMap<String, ConfigValue<String>>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub env_text: Option<String>,
}

/// A gateway definition. Runtime status/container fields are excluded.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Gateway {
    pub id: String,
    pub name: String,
    #[serde(rename = "type")]
    pub gateway_type: GatewayType,
    pub agent: String,
    pub enabled: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub env: Option<BTreeMap<String, ConfigValue<String>>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub env_text: Option<String>,
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub secrets: BTreeMap<String, ConfigValue<String>>,
}

/// The desired-state configuration payload.
#[derive(Clone, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ConfigSpec {
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub secrets: Vec<SecretDeclaration>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub kernel_configs: Vec<KernelConfig>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub connections: Vec<Connection>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub skills: Vec<Skill>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub agents: Vec<Agent>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub gateways: Vec<Gateway>,
}

/// The aggregate manifest form (`kind: AgentSpaceConfig`).
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct AggregateManifest {
    pub api_version: String,
    pub kind: String,
    pub metadata: Metadata,
    #[serde(default)]
    pub spec: ConfigSpec,
}

/// The strict, single-schema desired-state document.
///
/// This is the YAML schema, the active in-memory config, the validation input,
/// the UI mutation target, and the canonical serialization model. There are no
/// parallel persistence models.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ConfigDocument {
    pub metadata_name: String,
    pub spec: ConfigSpec,
}

impl Default for ConfigDocument {
    fn default() -> Self {
        Self {
            metadata_name: DEFAULT_METADATA_NAME.to_owned(),
            spec: ConfigSpec::default(),
        }
    }
}

impl ConfigDocument {
    #[must_use]
    pub fn into_aggregate(self) -> AggregateManifest {
        AggregateManifest {
            api_version: API_VERSION.to_owned(),
            kind: KIND_AGGREGATE.to_owned(),
            metadata: Metadata {
                name: self.metadata_name,
            },
            spec: self.spec,
        }
    }

    #[must_use]
    pub fn to_aggregate(&self) -> AggregateManifest {
        self.clone().into_aggregate()
    }

    #[must_use]
    pub fn connection(&self, id: &str) -> Option<&Connection> {
        self.spec.connections.iter().find(|item| item.id == id)
    }

    #[must_use]
    pub fn gateway(&self, id: &str) -> Option<&Gateway> {
        self.spec.gateways.iter().find(|item| item.id == id)
    }

    #[must_use]
    pub fn secret_declared(&self, name: &str) -> bool {
        self.spec
            .secrets
            .iter()
            .any(|item| item.name.as_str() == name)
    }
}

/// Convert the structured or raw env representation into normalized text.
///
/// The result is suitable for the existing `.env`-style record fields. Literal
/// entries render as `KEY=VALUE`; secret-referenced entries are omitted because
/// they must be resolved lazily at their point of use.
#[must_use]
pub fn env_to_text(
    env: Option<&BTreeMap<String, ConfigValue<String>>>,
    env_text: Option<&str>,
) -> String {
    if let Some(text) = env_text {
        return text.to_owned();
    }
    let Some(env) = env else {
        return String::new();
    };
    let mut lines = Vec::new();
    for (key, value) in env {
        if let ConfigValue::Literal(literal) = value {
            lines.push(format!("{key}={literal}"));
        }
    }
    lines.join("\n")
}
