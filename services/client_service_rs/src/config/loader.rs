use std::{collections::BTreeMap, collections::BTreeSet, str::FromStr};

use serde::Deserialize;

use crate::{
    config::{
        document::{
            API_VERSION, Agent, AggregateManifest, ConfigDocument, ConfigSpec, Connection,
            GIT_AGENT_METADATA_NAME, Gateway, GitAgentConfig, KIND_AGGREGATE, KernelConfig,
            Metadata, SecretDeclaration, Skill,
        },
        error::ConfigError,
        value::{ConfigValue, SecretName},
    },
    models::{ConnectionApiFlavor, GatewayType, HarnessName},
};

/// Parse plain YAML source (one or more `---`-separated documents) into a
/// single strict [`ConfigDocument`]. Unknown fields, unsupported versions,
/// duplicate identities, and invalid enums are rejected.
///
/// # Errors
/// Returns a [`ConfigError`] describing the first structural or schema failure.
pub fn load_yaml(source: &str) -> Result<ConfigDocument, ConfigError> {
    load_yaml_with_resolver(source, &|_path| Err(ConfigError::UnsupportedBundle))
}

/// A resolver that expands a path-based skill source into an inline file map.
pub type SkillSourceResolver<'a> =
    dyn Fn(&str) -> Result<BTreeMap<String, String>, ConfigError> + 'a;

/// Parse YAML using a resolver that expands path-based skill sources into inline
/// file maps. Plain YAML uses a resolver that rejects path-based sources.
///
/// # Errors
/// Returns a [`ConfigError`] describing the first structural or schema failure.
pub fn load_yaml_with_resolver(
    source: &str,
    resolve_skill_source: &SkillSourceResolver<'_>,
) -> Result<ConfigDocument, ConfigError> {
    let mut accumulator = DocumentAccumulator::default();
    accumulator.merge_source(source, resolve_skill_source)?;
    accumulator.finish()
}

/// Accumulates one or more parsed YAML sources into a single strict document.
///
/// Each source may use a distinct skill-source resolver, so a bundle can resolve
/// every YAML document's `spec.source.path` relative to that document's own
/// directory. Produces a strict [`ConfigDocument`] on [`DocumentAccumulator::finish`].
#[derive(Default)]
pub struct DocumentAccumulator {
    metadata_name: Option<String>,
    spec: ConfigSpec,
}

impl DocumentAccumulator {
    /// Parse and merge every `---`-separated document in `source`, expanding any
    /// path-based skill sources with `resolve_skill_source`.
    ///
    /// # Errors
    /// Returns a [`ConfigError`] describing the first structural or schema
    /// failure in `source`.
    pub fn merge_source(
        &mut self,
        source: &str,
        resolve_skill_source: &SkillSourceResolver<'_>,
    ) -> Result<(), ConfigError> {
        for document in serde_yaml_ng::Deserializer::from_str(source) {
            let value = serde_yaml_ng::Value::deserialize(document).map_err(|e| parse_error(&e))?;
            if value.is_null() {
                continue;
            }
            let kind = read_str_field(&value, "kind")?;
            let api_version = read_str_field(&value, "apiVersion")?;
            if api_version != API_VERSION {
                return Err(ConfigError::UnsupportedApiVersion { value: api_version });
            }
            merge_document(
                &kind,
                value,
                &mut self.metadata_name,
                &mut self.spec,
                resolve_skill_source,
            )?;
        }
        Ok(())
    }

    /// Finalize the accumulated documents into a strict [`ConfigDocument`],
    /// rejecting empty input and duplicate identities across all sources.
    ///
    /// # Errors
    /// Returns a [`ConfigError`] when no documents were provided or an identity
    /// is declared more than once.
    pub fn finish(self) -> Result<ConfigDocument, ConfigError> {
        if self.metadata_name.is_none() && spec_is_empty(&self.spec) {
            return Err(ConfigError::Parse {
                detail: "no configuration documents were provided".to_owned(),
            });
        }
        let document = ConfigDocument {
            metadata_name: self
                .metadata_name
                .unwrap_or_else(|| crate::config::document::DEFAULT_METADATA_NAME.to_owned()),
            spec: self.spec,
        };
        check_duplicate_ids(&document)?;
        Ok(document)
    }
}

fn merge_document(
    kind: &str,
    value: serde_yaml_ng::Value,
    metadata_name: &mut Option<String>,
    spec: &mut ConfigSpec,
    resolve_skill_source: &SkillSourceResolver<'_>,
) -> Result<(), ConfigError> {
    match kind {
        KIND_AGGREGATE => {
            let manifest: AggregateManifest = from_value(value)?;
            *metadata_name = Some(manifest.metadata.name);
            merge_spec(spec, manifest.spec)?;
        }
        "SecretDeclaration" => {
            let manifest: Standalone<SecretDeclarationSpec> = from_value(value)?;
            let name =
                SecretName::new(manifest.metadata.name).map_err(|error| ConfigError::Parse {
                    detail: error.to_string(),
                })?;
            spec.secrets.push(SecretDeclaration {
                name,
                description: manifest.spec.description,
            });
        }
        "KernelConfig" => {
            let manifest: Standalone<EnvSpec> = from_value(value)?;
            let harness = HarnessName::from_str(&manifest.metadata.name).map_err(|error| {
                ConfigError::Parse {
                    detail: error.to_string(),
                }
            })?;
            spec.kernel_configs.push(KernelConfig {
                harness,
                env: manifest.spec.env,
                env_text: manifest.spec.env_text,
            });
        }
        "Connection" => {
            let manifest: Standalone<ConnectionSpec> = from_value(value)?;
            spec.connections.push(Connection {
                id: manifest.metadata.name,
                name: manifest.spec.name,
                url: manifest.spec.url,
                api_flavor: manifest.spec.api_flavor,
                api_key: manifest.spec.api_key,
            });
        }
        "Skill" => {
            let manifest: Standalone<SkillSpec> = from_value(value)?;
            spec.skills
                .push(parse_skill(manifest, resolve_skill_source)?);
        }
        "Agent" => {
            let manifest: Standalone<AgentSpec> = from_value(value)?;
            spec.agents.push(Agent {
                id: manifest.metadata.name,
                name: manifest.spec.name,
                harness: manifest.spec.harness,
                connection: manifest.spec.connection,
                system_prompt: manifest.spec.system_prompt,
                skills: manifest.spec.skills,
                env: manifest.spec.env,
                env_text: manifest.spec.env_text,
            });
        }
        "Gateway" => {
            let manifest: Standalone<GatewaySpec> = from_value(value)?;
            spec.gateways.push(Gateway {
                id: manifest.metadata.name,
                name: manifest.spec.name,
                gateway_type: manifest.spec.gateway_type,
                agent: manifest.spec.agent,
                enabled: manifest.spec.enabled,
                env: manifest.spec.env,
                env_text: manifest.spec.env_text,
                secrets: manifest.spec.secrets,
            });
        }
        "GitAgentConfig" => {
            let manifest: Standalone<GitAgentConfig> = from_value(value)?;
            merge_git_agent(spec, manifest)?;
        }
        other => {
            return Err(ConfigError::UnsupportedKind {
                value: other.to_owned(),
            });
        }
    }
    Ok(())
}

fn parse_skill(
    manifest: Standalone<SkillSpec>,
    resolve_skill_source: &SkillSourceResolver<'_>,
) -> Result<Skill, ConfigError> {
    let files = match (manifest.spec.files, manifest.spec.source) {
        (Some(files), None) => files,
        (None, Some(source)) => resolve_skill_source(&source.path)?,
        (Some(_), Some(_)) => {
            return Err(ConfigError::Parse {
                detail: format!(
                    "Skill {:?} sets both spec.files and spec.source; exactly one is required",
                    manifest.metadata.name
                ),
            });
        }
        (None, None) => {
            return Err(ConfigError::Parse {
                detail: format!(
                    "Skill {:?} must set exactly one of spec.files or spec.source",
                    manifest.metadata.name
                ),
            });
        }
    };
    Ok(Skill {
        id: manifest.metadata.name,
        files,
    })
}

fn merge_git_agent(
    spec: &mut ConfigSpec,
    manifest: Standalone<GitAgentConfig>,
) -> Result<(), ConfigError> {
    if manifest.metadata.name != GIT_AGENT_METADATA_NAME {
        return Err(ConfigError::Parse {
            detail: format!("GitAgentConfig metadata.name must be {GIT_AGENT_METADATA_NAME:?}"),
        });
    }
    if spec.git_agent.is_some() {
        return Err(ConfigError::DuplicateResource {
            kind: "gitAgentConfig".to_owned(),
            id: GIT_AGENT_METADATA_NAME.to_owned(),
        });
    }
    spec.git_agent = Some(manifest.spec);
    Ok(())
}

fn merge_spec(target: &mut ConfigSpec, source: ConfigSpec) -> Result<(), ConfigError> {
    target.secrets.extend(source.secrets);
    target.kernel_configs.extend(source.kernel_configs);
    target.connections.extend(source.connections);
    target.skills.extend(source.skills);
    target.agents.extend(source.agents);
    target.gateways.extend(source.gateways);
    if let Some(git_agent) = source.git_agent {
        if target.git_agent.is_some() {
            return Err(ConfigError::DuplicateResource {
                kind: "gitAgentConfig".to_owned(),
                id: GIT_AGENT_METADATA_NAME.to_owned(),
            });
        }
        target.git_agent = Some(git_agent);
    }
    Ok(())
}

fn check_duplicate_ids(document: &ConfigDocument) -> Result<(), ConfigError> {
    check_unique(
        "secret",
        document.spec.secrets.iter().map(|item| item.name.as_str()),
    )?;
    check_unique(
        "kernelConfig",
        document
            .spec
            .kernel_configs
            .iter()
            .map(|item| item.harness.as_str()),
    )?;
    check_unique(
        "connection",
        document
            .spec
            .connections
            .iter()
            .map(|item| item.id.as_str()),
    )?;
    check_unique(
        "skill",
        document.spec.skills.iter().map(|item| item.id.as_str()),
    )?;
    check_unique(
        "agent",
        document.spec.agents.iter().map(|item| item.id.as_str()),
    )?;
    check_unique(
        "gateway",
        document.spec.gateways.iter().map(|item| item.id.as_str()),
    )?;
    Ok(())
}

fn check_unique<'a>(kind: &str, ids: impl Iterator<Item = &'a str>) -> Result<(), ConfigError> {
    let mut seen = BTreeSet::new();
    for id in ids {
        if !seen.insert(id.to_owned()) {
            return Err(ConfigError::DuplicateResource {
                kind: kind.to_owned(),
                id: id.to_owned(),
            });
        }
    }
    Ok(())
}

const fn spec_is_empty(spec: &ConfigSpec) -> bool {
    spec.secrets.is_empty()
        && spec.kernel_configs.is_empty()
        && spec.connections.is_empty()
        && spec.skills.is_empty()
        && spec.agents.is_empty()
        && spec.gateways.is_empty()
        && spec.git_agent.is_none()
}

fn read_str_field(value: &serde_yaml_ng::Value, field: &str) -> Result<String, ConfigError> {
    value
        .get(field)
        .and_then(serde_yaml_ng::Value::as_str)
        .map(ToOwned::to_owned)
        .ok_or_else(|| ConfigError::Parse {
            detail: format!("document is missing required string field {field:?}"),
        })
}

fn from_value<T: serde::de::DeserializeOwned>(
    value: serde_yaml_ng::Value,
) -> Result<T, ConfigError> {
    serde_yaml_ng::from_value(value).map_err(|e| parse_error(&e))
}

fn parse_error(error: &serde_yaml_ng::Error) -> ConfigError {
    ConfigError::Parse {
        detail: error.to_string(),
    }
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct Standalone<S> {
    #[allow(dead_code)]
    api_version: String,
    #[allow(dead_code)]
    kind: String,
    metadata: Metadata,
    spec: S,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct SecretDeclarationSpec {
    #[serde(default)]
    description: Option<String>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct EnvSpec {
    #[serde(default)]
    env: Option<BTreeMap<String, ConfigValue<String>>>,
    #[serde(default)]
    env_text: Option<String>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct ConnectionSpec {
    name: String,
    url: ConfigValue<String>,
    api_flavor: ConnectionApiFlavor,
    #[serde(default)]
    api_key: Option<ConfigValue<String>>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct SkillSpec {
    #[serde(default)]
    files: Option<BTreeMap<String, String>>,
    #[serde(default)]
    source: Option<SkillSourceSpec>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct SkillSourceSpec {
    path: String,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct AgentSpec {
    name: String,
    harness: HarnessName,
    #[serde(default)]
    connection: Option<String>,
    system_prompt: ConfigValue<String>,
    #[serde(default)]
    skills: Vec<String>,
    #[serde(default)]
    env: Option<BTreeMap<String, ConfigValue<String>>>,
    #[serde(default)]
    env_text: Option<String>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct GatewaySpec {
    name: String,
    #[serde(rename = "type")]
    gateway_type: GatewayType,
    agent: String,
    enabled: bool,
    #[serde(default)]
    env: Option<BTreeMap<String, ConfigValue<String>>>,
    #[serde(default)]
    env_text: Option<String>,
    #[serde(default)]
    secrets: BTreeMap<String, ConfigValue<String>>,
}
