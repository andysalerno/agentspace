use std::collections::BTreeMap;

use crate::{
    config::{
        document::env_to_text,
        secrets::{SecretStore, SecretStoreError},
        state::ConfigState,
        value::ConfigValue,
    },
    models::HarnessName,
};

/// A secret reference that must be resolved but has no value set.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct MissingSecret {
    pub name: String,
    pub field: String,
}

/// Failure while lazily resolving secret-backed configuration.
#[derive(Debug)]
pub enum ResolveError {
    /// One or more referenced secrets have no value set.
    Missing(Vec<MissingSecret>),
    /// The secret store failed to decrypt or read a value.
    Store(SecretStoreError),
}

impl From<SecretStoreError> for ResolveError {
    fn from(error: SecretStoreError) -> Self {
        Self::Store(error)
    }
}

/// The lazily resolved endpoint for a connection.
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct ResolvedConnection {
    pub url: Option<String>,
    pub api_key: Option<String>,
}

/// Resolve a connection URL and API key, accumulating any missing secrets into
/// `missing` (so a caller can report every missing field at once).
///
/// # Errors
/// Returns [`ResolveError::Store`] if the secret store fails to read a value.
pub fn resolve_connection(
    config: &ConfigState,
    connection_id: &str,
    missing: &mut Vec<MissingSecret>,
) -> Result<ResolvedConnection, ResolveError> {
    let document = config.active();
    let Some(connection) = document.connection(connection_id) else {
        return Ok(ResolvedConnection::default());
    };
    let secrets = config.secrets();
    let url = resolve_one(
        &secrets,
        &connection.url,
        &format!("connections/{connection_id}/url"),
        missing,
    )?;
    let api_key = match &connection.api_key {
        Some(api_key) => resolve_one(
            &secrets,
            api_key,
            &format!("connections/{connection_id}/apiKey"),
            missing,
        )?,
        None => None,
    };
    Ok(ResolvedConnection { url, api_key })
}

/// Resolve the effective environment for an agent, accumulating missing secrets.
///
/// # Errors
/// Returns [`ResolveError::Store`] if the secret store fails to read a value.
pub fn resolve_agent_env(
    config: &ConfigState,
    agent_id: &str,
    missing: &mut Vec<MissingSecret>,
) -> Result<BTreeMap<String, String>, ResolveError> {
    let document = config.active();
    let Some(agent) = document.spec.agents.iter().find(|item| item.id == agent_id) else {
        return Ok(BTreeMap::new());
    };
    let secrets = config.secrets();
    let mut env =
        crate::models::parse_env_vars(&env_to_text(agent.env.as_ref(), agent.env_text.as_deref()));
    if let Some(structured) = &agent.env {
        for (key, value) in structured {
            let field = format!("agents/{agent_id}/env/{key}");
            if let Some(resolved) = resolve_one(&secrets, value, &field, missing)? {
                env.insert(key.clone(), resolved);
            }
        }
    }
    Ok(env)
}

/// Resolve the effective environment for a kernel harness, accumulating missing
/// secrets.
///
/// # Errors
/// Returns [`ResolveError::Store`] if the secret store fails to read a value.
pub fn resolve_kernel_env(
    config: &ConfigState,
    harness: HarnessName,
    missing: &mut Vec<MissingSecret>,
) -> Result<BTreeMap<String, String>, ResolveError> {
    let document = config.active();
    let Some(kernel) = document
        .spec
        .kernel_configs
        .iter()
        .find(|item| item.harness == harness)
    else {
        return Ok(BTreeMap::new());
    };
    let secrets = config.secrets();
    let mut env = crate::models::parse_env_vars(&env_to_text(
        kernel.env.as_ref(),
        kernel.env_text.as_deref(),
    ));
    if let Some(structured) = &kernel.env {
        for (key, value) in structured {
            let field = format!("kernelConfigs/{}/env/{key}", harness.as_str());
            if let Some(resolved) = resolve_one(&secrets, value, &field, missing)? {
                env.insert(key.clone(), resolved);
            }
        }
    }
    Ok(env)
}

/// Resolve an agent's system prompt, accumulating a missing secret if the prompt
/// is a `secretRef` with no value set.
///
/// # Errors
/// Returns [`ResolveError::Store`] if the secret store fails to read a value.
pub fn resolve_agent_system_prompt(
    config: &ConfigState,
    agent_id: &str,
    missing: &mut Vec<MissingSecret>,
) -> Result<Option<String>, ResolveError> {
    let document = config.active();
    let Some(agent) = document.spec.agents.iter().find(|item| item.id == agent_id) else {
        return Ok(None);
    };
    let secrets = config.secrets();
    let field = format!("agents/{agent_id}/systemPrompt");
    resolve_one(&secrets, &agent.system_prompt, &field, missing)
}

/// The lazily resolved effective Git Agent configuration.
///
/// Every secret-backed leaf has been resolved to its literal value. This is the
/// only form exposed to the Git Agent; it is never serialized into config
/// exports and its secret-derived values must never be logged.
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct ResolvedGitAgent {
    pub enabled: bool,
    pub default_branch: Option<String>,
    pub allowed_ref_prefixes: Vec<String>,
    pub allowed_refs: Vec<String>,
    pub remote_url: Option<String>,
    pub patch_url: Option<String>,
    pub review_agent: String,
    pub validation_command: Option<String>,
}

/// Resolve the effective Git Agent configuration, resolving every
/// `secretRef`-backed leaf. Returns `Ok(None)` when no Git Agent is configured.
///
/// # Errors
/// Returns [`ResolveError::Missing`] listing every referenced secret that has no
/// value set (with its field path), or [`ResolveError::Store`] on a store read
/// failure.
pub fn resolve_git_agent(config: &ConfigState) -> Result<Option<ResolvedGitAgent>, ResolveError> {
    let document = config.active();
    let Some(git_agent) = document.spec.git_agent.as_ref() else {
        return Ok(None);
    };
    let secrets = config.secrets();
    let mut missing = Vec::new();

    let default_branch = resolve_one(
        &secrets,
        &git_agent.default_branch,
        "gitAgent/defaultBranch",
        &mut missing,
    )?;
    let remote_url = resolve_one(
        &secrets,
        &git_agent.remote_url,
        "gitAgent/remoteUrl",
        &mut missing,
    )?;
    let patch_url = resolve_one(
        &secrets,
        &git_agent.patch_url,
        "gitAgent/patchUrl",
        &mut missing,
    )?;
    let validation_command = resolve_one(
        &secrets,
        &git_agent.validation_command,
        "gitAgent/validationCommand",
        &mut missing,
    )?;
    let allowed_ref_prefixes = resolve_list(
        &secrets,
        &git_agent.allowed_ref_prefixes,
        "gitAgent/allowedRefPrefixes",
        &mut missing,
    )?;
    let allowed_refs = resolve_list(
        &secrets,
        &git_agent.allowed_refs,
        "gitAgent/allowedRefs",
        &mut missing,
    )?;

    if !missing.is_empty() {
        return Err(ResolveError::Missing(missing));
    }
    Ok(Some(ResolvedGitAgent {
        enabled: git_agent.enabled,
        default_branch: non_empty(default_branch),
        allowed_ref_prefixes,
        allowed_refs,
        remote_url: non_empty(remote_url),
        patch_url: non_empty(patch_url),
        review_agent: git_agent.review_agent.clone(),
        validation_command: non_empty(validation_command),
    }))
}

/// Drop an empty literal so an unset optional leaf resolves to `None` rather
/// than an empty string.
fn non_empty(value: Option<String>) -> Option<String> {
    value.filter(|text| !text.is_empty())
}

/// Resolve a list of `ConfigValue<String>` leaves, accumulating missing secrets
/// and dropping empty literal entries.
fn resolve_list(
    secrets: &SecretStore,
    values: &[ConfigValue<String>],
    prefix: &str,
    missing: &mut Vec<MissingSecret>,
) -> Result<Vec<String>, ResolveError> {
    let mut resolved = Vec::new();
    for (index, value) in values.iter().enumerate() {
        let field = format!("{prefix}/{index}");
        if let Some(item) = resolve_one(secrets, value, &field, missing)?
            && !item.is_empty()
        {
            resolved.push(item);
        }
    }
    Ok(resolved)
}

fn resolve_one(
    secrets: &SecretStore,
    value: &ConfigValue<String>,
    field: &str,
    missing: &mut Vec<MissingSecret>,
) -> Result<Option<String>, ResolveError> {
    match value {
        ConfigValue::Literal(literal) => Ok(Some(literal.clone())),
        ConfigValue::Secret(name) => secrets.resolve(name.as_str())?.map_or_else(
            || {
                missing.push(MissingSecret {
                    name: name.as_str().to_owned(),
                    field: field.to_owned(),
                });
                Ok(None)
            },
            |resolved| Ok(Some(resolved)),
        ),
    }
}

/// Resolve the complete effective environment for a gateway.
///
/// Merges literal env text, literal/secret env entries, and literal/secret
/// `secrets` entries. Returns [`ResolveError::Missing`] when a referenced
/// secret has no value set.
///
/// # Errors
/// Returns [`ResolveError`] on missing secret values or store failures.
pub fn resolve_gateway_env(
    config: &ConfigState,
    gateway_id: &str,
) -> Result<BTreeMap<String, String>, ResolveError> {
    let document = config.active();
    let Some(gateway) = document.gateway(gateway_id) else {
        return Ok(BTreeMap::new());
    };
    let secrets = config.secrets();
    let mut env = crate::models::parse_env_vars(&env_to_text(
        gateway.env.as_ref(),
        gateway.env_text.as_deref(),
    ));
    let mut missing = Vec::new();

    if let Some(structured) = &gateway.env {
        for (key, value) in structured {
            let field = format!("gateways/{gateway_id}/env/{key}");
            if let Some(resolved) = resolve_one(&secrets, value, &field, &mut missing)? {
                env.insert(key.clone(), resolved);
            }
        }
    }
    for (key, value) in &gateway.secrets {
        let field = format!("gateways/{gateway_id}/secrets/{key}");
        if let Some(resolved) = resolve_one(&secrets, value, &field, &mut missing)? {
            env.insert(key.clone(), resolved);
        }
    }

    if missing.is_empty() {
        Ok(env)
    } else {
        Err(ResolveError::Missing(missing))
    }
}
