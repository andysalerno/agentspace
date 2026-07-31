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
