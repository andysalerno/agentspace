//! Adapters projecting the [`ConfigDocument`] to and from record types.
//!
//! These project the authoritative [`ConfigDocument`] to and from the
//! WebUI-facing record types. The document is the single source of truth; these
//! functions never persist configuration fields anywhere else. Runtime-only
//! attributes (timestamps, gateway status, agent workspace mounts) live in the
//! disposable runtime metadata owned by [`ConfigState`].

use crate::{
    config::{
        document::{Agent, AgentCliConfig, Connection, Gateway, KernelConfig, Skill, env_to_text},
        state::{ConfigState, GatewayRuntime},
        value::ConfigValue,
    },
    errors::StoreError,
    models::{
        AgentCliRecord, AgentRecord, ConnectionRecord, GatewayRecord, HarnessName,
        KernelConfigRecord,
    },
};

fn literal_or_empty(value: &ConfigValue<String>) -> String {
    match value {
        ConfigValue::Literal(literal) => literal.clone(),
        ConfigValue::Secret(_) => String::new(),
    }
}

fn text_to_env(env_vars: &str) -> Option<String> {
    if env_vars.trim().is_empty() {
        None
    } else {
        Some(env_vars.to_owned())
    }
}

/// Merge a legacy flat env string onto an existing structured/text env,
/// preserving the structured form (including any `secretRef` leaves) when the
/// legacy projection is unchanged. The legacy record cannot represent secret or
/// structured env, so an unchanged flattened value must not clobber it.
fn merge_env(
    existing_env: Option<&std::collections::BTreeMap<String, ConfigValue<String>>>,
    existing_env_text: Option<&str>,
    incoming_text: &str,
) -> (
    Option<std::collections::BTreeMap<String, ConfigValue<String>>>,
    Option<String>,
) {
    let current_flat = env_to_text(existing_env, existing_env_text);
    if current_flat == incoming_text {
        (
            existing_env.cloned(),
            existing_env_text.map(ToOwned::to_owned),
        )
    } else {
        (None, text_to_env(incoming_text))
    }
}

/// Merge a legacy literal into a required secret-backed value. An empty literal
/// preserves an existing `secretRef`.
fn merge_required_value(existing: &ConfigValue<String>, incoming: &str) -> ConfigValue<String> {
    match existing {
        ConfigValue::Secret(name) if incoming.is_empty() => ConfigValue::Secret(name.clone()),
        _ => ConfigValue::Literal(incoming.to_owned()),
    }
}

/// Merge legacy literal gateway secrets onto an existing secret map, preserving
/// `secretRef` entries the legacy request cannot represent.
fn merge_gateway_secrets(
    existing: &std::collections::BTreeMap<String, ConfigValue<String>>,
    incoming: &std::collections::BTreeMap<String, String>,
) -> std::collections::BTreeMap<String, ConfigValue<String>> {
    let existing_literals: std::collections::BTreeMap<String, String> = existing
        .iter()
        .filter_map(|(key, value)| {
            value
                .as_literal()
                .map(|literal| (key.clone(), literal.clone()))
        })
        .collect();
    if &existing_literals == incoming {
        return existing.clone();
    }
    let mut merged = std::collections::BTreeMap::new();
    for (key, value) in existing {
        if value.is_secret() && !incoming.contains_key(key) {
            merged.insert(key.clone(), value.clone());
        }
    }
    for (key, value) in incoming {
        merged.insert(key.clone(), ConfigValue::Literal(value.clone()));
    }
    merged
}

fn agent_key(id: &str) -> String {
    format!("agent/{id}")
}

fn connection_key(id: &str) -> String {
    format!("connection/{id}")
}

fn gateway_key(id: &str) -> String {
    format!("gateway/{id}")
}

fn kernel_key(harness: HarnessName) -> String {
    format!("kernelConfig/{}", harness.as_str())
}

// ----- agents -----

fn record_to_agent(record: &AgentRecord) -> Agent {
    Agent {
        id: record.agent_id.clone(),
        name: record.name.clone(),
        harness: record.harness,
        connection: record.connection_id.clone(),
        cli: record.cli.as_ref().map(|cli| AgentCliConfig {
            harness: cli.harness,
            connection: cli.connection_id.clone(),
        }),
        system_prompt: ConfigValue::Literal(record.system_prompt.clone()),
        skills: record.skills.clone(),
        env: None,
        env_text: text_to_env(&record.env_vars),
    }
}

fn agent_to_record(config: &ConfigState, agent: &Agent) -> AgentRecord {
    let (created_at, updated_at) = config.timestamps(&agent_key(&agent.id));
    AgentRecord {
        agent_id: agent.id.clone(),
        name: agent.name.clone(),
        harness: agent.harness,
        system_prompt: literal_or_empty(&agent.system_prompt),
        skills: agent.skills.clone(),
        env_vars: env_to_text(agent.env.as_ref(), agent.env_text.as_deref()),
        connection_id: agent.connection.clone(),
        cli: agent.cli.as_ref().map(|cli| AgentCliRecord {
            harness: cli.harness,
            connection_id: cli.connection.clone(),
        }),
        workspace_mounts: config.agent_mounts(&agent.id),
        created_at,
        updated_at,
    }
}

pub fn list_agents(config: &ConfigState) -> Result<Vec<AgentRecord>, StoreError> {
    let mut records: Vec<AgentRecord> = config
        .active()
        .spec
        .agents
        .iter()
        .map(|agent| agent_to_record(config, agent))
        .collect();
    records.sort_by(|left, right| left.agent_id.cmp(&right.agent_id));
    Ok(records)
}

pub fn get_agent(config: &ConfigState, agent_id: &str) -> Result<Option<AgentRecord>, StoreError> {
    let document = config.active();
    if let Some(agent) = document
        .spec
        .agents
        .iter()
        .find(|agent| agent.id == agent_id)
    {
        return Ok(Some(agent_to_record(config, agent)));
    }
    Ok(None)
}

pub fn insert_agent(config: &ConfigState, record: &AgentRecord) -> Result<(), StoreError> {
    let id = record.agent_id.clone();
    let mounts = record.workspace_mounts.clone();
    let agent = record_to_agent(record);
    config.mutate(move |document| {
        if document.spec.agents.iter().any(|item| item.id == agent.id) {
            return Err(StoreError::AgentAlreadyExists { agent_id: agent.id });
        }
        document.spec.agents.push(agent);
        Ok(())
    })?;
    config.mark_created(&agent_key(&id));
    config.set_agent_mounts(&id, mounts);
    Ok(())
}

pub fn update_agent(config: &ConfigState, record: &AgentRecord) -> Result<(), StoreError> {
    let id = record.agent_id.clone();
    let mounts = record.workspace_mounts.clone();
    let record = record.clone();
    config.mutate(move |document| {
        let slot = document
            .spec
            .agents
            .iter_mut()
            .find(|item| item.id == record.agent_id)
            .ok_or_else(|| StoreError::AgentNotFound {
                agent_id: record.agent_id.clone(),
            })?;
        patch_agent(slot, &record);
        Ok(())
    })?;
    config.mark_updated(&agent_key(&id));
    config.set_agent_mounts(&id, mounts);
    Ok(())
}

pub fn upsert_agent(config: &ConfigState, record: &AgentRecord) -> Result<(), StoreError> {
    let id = record.agent_id.clone();
    let mounts = record.workspace_mounts.clone();
    let existed = config.active().spec.agents.iter().any(|item| item.id == id);
    let record = record.clone();
    config.mutate(move |document| {
        if let Some(slot) = document
            .spec
            .agents
            .iter_mut()
            .find(|item| item.id == record.agent_id)
        {
            patch_agent(slot, &record);
        } else {
            document.spec.agents.push(record_to_agent(&record));
        }
        Ok(())
    })?;
    if existed {
        config.mark_updated(&agent_key(&id));
    } else {
        config.mark_created(&agent_key(&id));
    }
    config.set_agent_mounts(&id, mounts);
    Ok(())
}

/// Patch the representable scalar fields of an agent from a legacy record,
/// preserving structured/secret env that the record cannot express.
fn patch_agent(slot: &mut Agent, record: &AgentRecord) {
    slot.name.clone_from(&record.name);
    slot.harness = record.harness;
    slot.system_prompt = merge_required_value(&slot.system_prompt, &record.system_prompt);
    slot.skills.clone_from(&record.skills);
    slot.connection.clone_from(&record.connection_id);
    slot.cli = record.cli.as_ref().map(|cli| AgentCliConfig {
        harness: cli.harness,
        connection: cli.connection_id.clone(),
    });
    let (env, env_text) = merge_env(
        slot.env.as_ref(),
        slot.env_text.as_deref(),
        &record.env_vars,
    );
    slot.env = env;
    slot.env_text = env_text;
}

pub fn add_agent_skill(
    config: &ConfigState,
    agent_id: &str,
    skill_id: &str,
) -> Result<bool, StoreError> {
    let mut added = false;
    config.mutate(|document| {
        let agent = document
            .spec
            .agents
            .iter_mut()
            .find(|item| item.id == agent_id)
            .ok_or_else(|| StoreError::AgentNotFound {
                agent_id: agent_id.to_owned(),
            })?;
        if agent.skills.iter().any(|skill| skill == skill_id) {
            return Ok(());
        }
        agent.skills.push(skill_id.to_owned());
        added = true;
        Ok(())
    })?;
    if added {
        config.mark_updated(&agent_key(agent_id));
    }
    Ok(added)
}

pub fn delete_agent(config: &ConfigState, agent_id: &str) -> Result<bool, StoreError> {
    let mut removed = false;
    config.mutate(|document| {
        let before = document.spec.agents.len();
        document.spec.agents.retain(|item| item.id != agent_id);
        removed = document.spec.agents.len() != before;
        Ok(())
    })?;
    if removed {
        config.remove_meta(&agent_key(agent_id));
        config.set_agent_mounts(agent_id, Vec::new());
    }
    Ok(removed)
}

// ----- connections -----

fn record_to_connection(record: &ConnectionRecord) -> Connection {
    Connection {
        id: record.connection_id.clone(),
        name: record.name.clone(),
        url: ConfigValue::Literal(record.url.clone()),
        api_flavor: record.api_flavor,
        api_key: connection_api_key_value(record),
    }
}

/// Project a record's mutually exclusive literal/`secretRef` API key fields onto
/// the document value. A secret name always wins over a literal, so a record
/// that carries both cannot silently downgrade a reference to a literal. The
/// name is already validated by its type, so this projection is infallible.
fn connection_api_key_value(record: &ConnectionRecord) -> Option<ConfigValue<String>> {
    if let Some(name) = &record.api_key_secret {
        return Some(ConfigValue::Secret(name.clone()));
    }
    if record.api_key.is_empty() {
        None
    } else {
        Some(ConfigValue::Literal(record.api_key.clone()))
    }
}

fn connection_to_record(config: &ConfigState, connection: &Connection) -> ConnectionRecord {
    let (created_at, updated_at) = config.timestamps(&connection_key(&connection.id));
    ConnectionRecord {
        connection_id: connection.id.clone(),
        name: connection.name.clone(),
        url: literal_or_empty(&connection.url),
        api_flavor: connection.api_flavor,
        api_key: connection
            .api_key
            .as_ref()
            .map(literal_or_empty)
            .unwrap_or_default(),
        api_key_secret: connection
            .api_key
            .as_ref()
            .and_then(ConfigValue::secret_name)
            .cloned(),
        created_at,
        updated_at,
    }
}

pub fn list_connections(config: &ConfigState) -> Result<Vec<ConnectionRecord>, StoreError> {
    let mut records: Vec<ConnectionRecord> = config
        .active()
        .spec
        .connections
        .iter()
        .map(|connection| connection_to_record(config, connection))
        .collect();
    records.sort_by(|left, right| left.connection_id.cmp(&right.connection_id));
    Ok(records)
}

pub fn get_connection(
    config: &ConfigState,
    connection_id: &str,
) -> Result<Option<ConnectionRecord>, StoreError> {
    Ok(config
        .active()
        .spec
        .connections
        .iter()
        .find(|connection| connection.id == connection_id)
        .map(|connection| connection_to_record(config, connection)))
}

pub fn insert_connection(
    config: &ConfigState,
    record: &ConnectionRecord,
) -> Result<(), StoreError> {
    let id = record.connection_id.clone();
    let connection = record_to_connection(record);
    config.mutate(move |document| {
        if document
            .spec
            .connections
            .iter()
            .any(|item| item.id == connection.id)
        {
            return Err(StoreError::ConnectionAlreadyExists {
                connection_id: connection.id,
            });
        }
        document.spec.connections.push(connection);
        Ok(())
    })?;
    config.mark_created(&connection_key(&id));
    Ok(())
}

pub fn update_connection(
    config: &ConfigState,
    record: &ConnectionRecord,
) -> Result<(), StoreError> {
    let id = record.connection_id.clone();
    let record = record.clone();
    config.mutate(move |document| {
        let slot = document
            .spec
            .connections
            .iter_mut()
            .find(|item| item.id == record.connection_id)
            .ok_or_else(|| StoreError::ConnectionNotFound {
                connection_id: record.connection_id.clone(),
            })?;
        patch_connection(slot, &record);
        Ok(())
    })?;
    config.mark_updated(&connection_key(&id));
    Ok(())
}

pub fn upsert_connection(
    config: &ConfigState,
    record: &ConnectionRecord,
) -> Result<(), StoreError> {
    let id = record.connection_id.clone();
    let existed = config
        .active()
        .spec
        .connections
        .iter()
        .any(|item| item.id == id);
    let record = record.clone();
    config.mutate(move |document| {
        if let Some(slot) = document
            .spec
            .connections
            .iter_mut()
            .find(|item| item.id == record.connection_id)
        {
            patch_connection(slot, &record);
        } else {
            document
                .spec
                .connections
                .push(record_to_connection(&record));
        }
        Ok(())
    })?;
    if existed {
        config.mark_updated(&connection_key(&id));
    } else {
        config.mark_created(&connection_key(&id));
    }
    Ok(())
}

/// Patch the representable scalar fields of a connection from a record,
/// preserving a secret-backed URL the record cannot express. The API key is
/// fully representable (literal or `secretRef`), so it is replaced outright.
fn patch_connection(slot: &mut Connection, record: &ConnectionRecord) {
    slot.name.clone_from(&record.name);
    slot.api_flavor = record.api_flavor;
    slot.url = merge_required_value(&slot.url, &record.url);
    slot.api_key = connection_api_key_value(record);
}

pub fn delete_connection(config: &ConfigState, connection_id: &str) -> Result<bool, StoreError> {
    let mut removed = false;
    config.mutate(|document| {
        let before = document.spec.connections.len();
        document
            .spec
            .connections
            .retain(|item| item.id != connection_id);
        removed = document.spec.connections.len() != before;
        Ok(())
    })?;
    if removed {
        config.remove_meta(&connection_key(connection_id));
    }
    Ok(removed)
}

// ----- gateways -----

fn record_to_gateway(record: &GatewayRecord) -> Gateway {
    Gateway {
        id: record.gateway_id.clone(),
        name: record.name.clone(),
        gateway_type: record.gateway_type,
        agent: record.agent_id.clone(),
        enabled: record.enabled,
        env: None,
        env_text: text_to_env(&record.env_vars),
        secrets: record
            .secrets
            .iter()
            .map(|(key, value)| (key.clone(), ConfigValue::Literal(value.clone())))
            .collect(),
    }
}

fn gateway_to_record(config: &ConfigState, gateway: &Gateway) -> GatewayRecord {
    let (created_at, updated_at) = config.timestamps(&gateway_key(&gateway.id));
    let runtime = config.gateway_runtime(&gateway.id);
    GatewayRecord {
        gateway_id: gateway.id.clone(),
        name: gateway.name.clone(),
        gateway_type: gateway.gateway_type,
        agent_id: gateway.agent.clone(),
        enabled: gateway.enabled,
        env_vars: env_to_text(gateway.env.as_ref(), gateway.env_text.as_deref()),
        secrets: gateway
            .secrets
            .iter()
            .filter_map(|(key, value)| match value {
                ConfigValue::Literal(literal) => Some((key.clone(), literal.clone())),
                ConfigValue::Secret(_) => None,
            })
            .collect(),
        status: runtime.status,
        last_error: runtime.last_error,
        container_name: runtime.container_name,
        created_at,
        updated_at,
    }
}

pub fn list_gateways(config: &ConfigState) -> Result<Vec<GatewayRecord>, StoreError> {
    let mut records: Vec<GatewayRecord> = config
        .active()
        .spec
        .gateways
        .iter()
        .map(|gateway| gateway_to_record(config, gateway))
        .collect();
    records.sort_by(|left, right| left.gateway_id.cmp(&right.gateway_id));
    Ok(records)
}

pub fn get_gateway(
    config: &ConfigState,
    gateway_id: &str,
) -> Result<Option<GatewayRecord>, StoreError> {
    Ok(config
        .active()
        .spec
        .gateways
        .iter()
        .find(|gateway| gateway.id == gateway_id)
        .map(|gateway| gateway_to_record(config, gateway)))
}

pub fn insert_gateway(config: &ConfigState, record: &GatewayRecord) -> Result<(), StoreError> {
    let id = record.gateway_id.clone();
    let runtime = GatewayRuntime {
        status: record.status.clone(),
        last_error: record.last_error.clone(),
        container_name: record.container_name.clone(),
    };
    let gateway = record_to_gateway(record);
    config.mutate(move |document| {
        if document
            .spec
            .gateways
            .iter()
            .any(|item| item.id == gateway.id)
        {
            return Err(StoreError::GatewayAlreadyExists {
                gateway_id: gateway.id,
            });
        }
        document.spec.gateways.push(gateway);
        Ok(())
    })?;
    config.mark_created(&gateway_key(&id));
    config.set_gateway_runtime(&id, runtime);
    Ok(())
}

pub fn update_gateway(config: &ConfigState, record: &GatewayRecord) -> Result<(), StoreError> {
    let id = record.gateway_id.clone();
    let runtime = GatewayRuntime {
        status: record.status.clone(),
        last_error: record.last_error.clone(),
        container_name: record.container_name.clone(),
    };
    let record = record.clone();
    config.mutate(move |document| {
        let slot = document
            .spec
            .gateways
            .iter_mut()
            .find(|item| item.id == record.gateway_id)
            .ok_or_else(|| StoreError::GatewayNotFound {
                gateway_id: record.gateway_id.clone(),
            })?;
        patch_gateway(slot, &record);
        Ok(())
    })?;
    config.mark_updated(&gateway_key(&id));
    config.set_gateway_runtime(&id, runtime);
    Ok(())
}

pub fn upsert_gateway(config: &ConfigState, record: &GatewayRecord) -> Result<(), StoreError> {
    let id = record.gateway_id.clone();
    let runtime = GatewayRuntime {
        status: record.status.clone(),
        last_error: record.last_error.clone(),
        container_name: record.container_name.clone(),
    };
    let existed = config
        .active()
        .spec
        .gateways
        .iter()
        .any(|item| item.id == id);
    let record = record.clone();
    config.mutate(move |document| {
        if let Some(slot) = document
            .spec
            .gateways
            .iter_mut()
            .find(|item| item.id == record.gateway_id)
        {
            patch_gateway(slot, &record);
        } else {
            document.spec.gateways.push(record_to_gateway(&record));
        }
        Ok(())
    })?;
    if existed {
        config.mark_updated(&gateway_key(&id));
    } else {
        config.mark_created(&gateway_key(&id));
    }
    config.set_gateway_runtime(&id, runtime);
    Ok(())
}

/// Update only the observed runtime status of a gateway. This never touches the
/// desired [`ConfigDocument`], so gateway start/stop cannot rewrite authored
/// configuration or flatten secret-backed env.
pub fn set_gateway_runtime_status(config: &ConfigState, gateway_id: &str, runtime: GatewayRuntime) {
    config.set_gateway_runtime(gateway_id, runtime);
}

/// Patch the representable scalar fields of a gateway from a legacy record,
/// preserving structured/secret env and `secretRef` secrets the record cannot
/// express.
fn patch_gateway(slot: &mut Gateway, record: &GatewayRecord) {
    slot.name.clone_from(&record.name);
    slot.gateway_type = record.gateway_type;
    slot.agent.clone_from(&record.agent_id);
    slot.enabled = record.enabled;
    let (env, env_text) = merge_env(
        slot.env.as_ref(),
        slot.env_text.as_deref(),
        &record.env_vars,
    );
    slot.env = env;
    slot.env_text = env_text;
    slot.secrets = merge_gateway_secrets(&slot.secrets, &record.secrets);
}

pub fn delete_gateway(config: &ConfigState, gateway_id: &str) -> Result<bool, StoreError> {
    let mut removed = false;
    config.mutate(|document| {
        let before = document.spec.gateways.len();
        document.spec.gateways.retain(|item| item.id != gateway_id);
        removed = document.spec.gateways.len() != before;
        Ok(())
    })?;
    if removed {
        config.remove_meta(&gateway_key(gateway_id));
        config.remove_gateway_runtime(gateway_id);
    }
    Ok(removed)
}

// ----- kernel configs -----

fn kernel_to_record(config: &ConfigState, kernel: &KernelConfig) -> KernelConfigRecord {
    let (_, updated_at) = config.timestamps(&kernel_key(kernel.harness));
    KernelConfigRecord {
        harness: kernel.harness,
        env_vars: env_to_text(kernel.env.as_ref(), kernel.env_text.as_deref()),
        updated_at,
    }
}

pub fn list_kernel_configs(config: &ConfigState) -> Result<Vec<KernelConfigRecord>, StoreError> {
    let mut records: Vec<KernelConfigRecord> = config
        .active()
        .spec
        .kernel_configs
        .iter()
        .map(|kernel| kernel_to_record(config, kernel))
        .collect();
    records.sort_by(|left, right| left.harness.as_str().cmp(right.harness.as_str()));
    Ok(records)
}

pub fn get_kernel_config(
    config: &ConfigState,
    harness: HarnessName,
) -> Result<Option<KernelConfigRecord>, StoreError> {
    Ok(config
        .active()
        .spec
        .kernel_configs
        .iter()
        .find(|kernel| kernel.harness == harness)
        .map(|kernel| kernel_to_record(config, kernel)))
}

pub fn upsert_kernel_config(
    config: &ConfigState,
    harness: HarnessName,
    env_vars: String,
) -> Result<KernelConfigRecord, StoreError> {
    config.mutate(|document| {
        if let Some(slot) = document
            .spec
            .kernel_configs
            .iter_mut()
            .find(|item| item.harness == harness)
        {
            let (env, env_text) = merge_env(slot.env.as_ref(), slot.env_text.as_deref(), &env_vars);
            slot.env = env;
            slot.env_text = env_text;
        } else {
            document.spec.kernel_configs.push(KernelConfig {
                harness,
                env: None,
                env_text: text_to_env(&env_vars),
            });
        }
        Ok(())
    })?;
    config.mark_updated(&kernel_key(harness));
    let (_, updated_at) = config.timestamps(&kernel_key(harness));
    Ok(KernelConfigRecord {
        harness,
        env_vars,
        updated_at,
    })
}

pub fn delete_kernel_config(
    config: &ConfigState,
    harness: HarnessName,
) -> Result<bool, StoreError> {
    let mut removed = false;
    config.mutate(|document| {
        let before = document.spec.kernel_configs.len();
        document
            .spec
            .kernel_configs
            .retain(|item| item.harness != harness);
        removed = document.spec.kernel_configs.len() != before;
        Ok(())
    })?;
    if removed {
        config.remove_meta(&kernel_key(harness));
    }
    Ok(removed)
}

// ----- user skills -----

/// List user-defined skills declared in the config document.
pub fn list_skills(config: &ConfigState) -> Result<Vec<Skill>, StoreError> {
    Ok(config.active().spec.skills.clone())
}

/// Return a single user skill by id, if declared in the document.
pub fn get_skill(config: &ConfigState, skill_id: &str) -> Result<Option<Skill>, StoreError> {
    Ok(config
        .active()
        .spec
        .skills
        .iter()
        .find(|skill| skill.id == skill_id)
        .cloned())
}

/// Return whether a user skill is declared in the document.
pub fn skill_exists(config: &ConfigState, skill_id: &str) -> Result<bool, StoreError> {
    Ok(config
        .active()
        .spec
        .skills
        .iter()
        .any(|skill| skill.id == skill_id))
}

/// Create or replace the inline file map of a user skill in the document.
pub fn upsert_skill(
    config: &ConfigState,
    skill_id: &str,
    files: std::collections::BTreeMap<String, String>,
) -> Result<(), StoreError> {
    let id = skill_id.to_owned();
    config.mutate(move |document| {
        if let Some(existing) = document.spec.skills.iter_mut().find(|skill| skill.id == id) {
            existing.files = files;
        } else {
            document.spec.skills.push(Skill { id, files });
        }
        Ok(())
    })
}

/// Remove a user skill from the document. Returns whether it existed.
pub fn delete_skill(config: &ConfigState, skill_id: &str) -> Result<bool, StoreError> {
    let existed = skill_exists(config, skill_id)?;
    if existed {
        let id = skill_id.to_owned();
        config.mutate(move |document| {
            document.spec.skills.retain(|skill| skill.id != id);
            Ok(())
        })?;
    }
    Ok(existed)
}
