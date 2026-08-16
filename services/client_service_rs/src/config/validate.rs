use std::collections::{BTreeMap, BTreeSet};

use crate::{
    config::{
        document::{ConfigDocument, DEFAULT_METADATA_NAME},
        error::{ConfigError, ValidationIssue},
        value::{ConfigValue, SecretName},
    },
    models::{validate_agent_id, validate_connection_id, validate_gateway_id, validate_skill_id},
};

/// A single secret reference discovered in the document, with its stable field
/// path (for example `connections/primary/apiKey`).
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct SecretReference {
    pub name: SecretName,
    pub field: String,
}

/// Collect every `secretRef` in the document with its stable field path.
#[must_use]
pub fn secret_references(document: &ConfigDocument) -> Vec<SecretReference> {
    let mut references = Vec::new();
    for connection in &document.spec.connections {
        collect_value(
            &mut references,
            &format!("connections/{}/url", connection.id),
            &connection.url,
        );
        if let Some(api_key) = &connection.api_key {
            collect_value(
                &mut references,
                &format!("connections/{}/apiKey", connection.id),
                api_key,
            );
        }
    }
    for kernel in &document.spec.kernel_configs {
        collect_env(
            &mut references,
            &format!("kernelConfigs/{}/env", kernel.harness.as_str()),
            kernel.env.as_ref(),
        );
    }
    for agent in &document.spec.agents {
        collect_value(
            &mut references,
            &format!("agents/{}/systemPrompt", agent.id),
            &agent.system_prompt,
        );
        collect_env(
            &mut references,
            &format!("agents/{}/env", agent.id),
            agent.env.as_ref(),
        );
    }
    for gateway in &document.spec.gateways {
        collect_env(
            &mut references,
            &format!("gateways/{}/env", gateway.id),
            gateway.env.as_ref(),
        );
        for (key, value) in &gateway.secrets {
            collect_value(
                &mut references,
                &format!("gateways/{}/secrets/{key}", gateway.id),
                value,
            );
        }
    }
    references
}

fn collect_value(references: &mut Vec<SecretReference>, field: &str, value: &ConfigValue<String>) {
    if let ConfigValue::Secret(name) = value {
        references.push(SecretReference {
            name: name.clone(),
            field: field.to_owned(),
        });
    }
}

fn collect_env(
    references: &mut Vec<SecretReference>,
    prefix: &str,
    env: Option<&BTreeMap<String, ConfigValue<String>>>,
) {
    let Some(env) = env else { return };
    for (key, value) in env {
        collect_value(references, &format!("{prefix}/{key}"), value);
    }
}

/// Validate the entire document graph: identity formats, referential
/// integrity, secret declarations, and gateway env/secret rules.
///
/// # Errors
/// Returns [`ConfigError::Validation`] with all discovered issues.
pub fn validate(
    document: &ConfigDocument,
    builtin_skill_ids: &BTreeSet<String>,
) -> Result<(), ConfigError> {
    validate_inner(document, builtin_skill_ids, true)
}

/// Validation for interactive CRUD mutations.
///
/// It enforces every rule except skill-reference resolution, because agents may
/// reference installation-owned builtin skills that are not part of the config
/// document.
///
/// # Errors
/// Returns [`ConfigError::Validation`] with all discovered issues.
pub fn validate_mutation(document: &ConfigDocument) -> Result<(), ConfigError> {
    validate_inner(document, &BTreeSet::new(), false)
}

fn validate_inner(
    document: &ConfigDocument,
    builtin_skill_ids: &BTreeSet<String>,
    check_skill_refs: bool,
) -> Result<(), ConfigError> {
    let mut issues = Vec::new();

    if document.metadata_name.trim().is_empty() {
        issues.push(
            ValidationIssue::new("invalid_metadata_name", "metadata.name must not be empty")
                .with_field("metadata/name"),
        );
    }

    let declared_secrets: BTreeSet<&str> = document
        .spec
        .secrets
        .iter()
        .map(|item| item.name.as_str())
        .collect();
    let connection_ids: BTreeSet<&str> = document
        .spec
        .connections
        .iter()
        .map(|item| item.id.as_str())
        .collect();
    let skill_ids: BTreeSet<&str> = document
        .spec
        .skills
        .iter()
        .map(|item| item.id.as_str())
        .collect();
    let agent_ids: BTreeSet<&str> = document
        .spec
        .agents
        .iter()
        .map(|item| item.id.as_str())
        .collect();

    validate_identities(&mut issues, document);
    validate_skill_builtin_collisions(&mut issues, document, builtin_skill_ids);
    validate_skills(&mut issues, document);
    validate_agents(
        &mut issues,
        document,
        &connection_ids,
        &skill_ids,
        builtin_skill_ids,
        check_skill_refs,
    );
    validate_kernels(&mut issues, document);
    validate_gateways(&mut issues, document, &agent_ids);
    validate_secret_refs(&mut issues, document, &declared_secrets);

    if issues.is_empty() {
        Ok(())
    } else {
        Err(ConfigError::Validation { issues })
    }
}

fn validate_identities(issues: &mut Vec<ValidationIssue>, document: &ConfigDocument) {
    for connection in &document.spec.connections {
        if validate_connection_id(&connection.id).is_err() {
            issues.push(
                ValidationIssue::new(
                    "invalid_connection_id",
                    format!("invalid connection id {:?}", connection.id),
                )
                .with_resource(format!("connection/{}", connection.id)),
            );
        }
    }
    for skill in &document.spec.skills {
        if validate_skill_id(&skill.id).is_err() {
            issues.push(
                ValidationIssue::new(
                    "invalid_skill_id",
                    format!("invalid skill id {:?}", skill.id),
                )
                .with_resource(format!("skill/{}", skill.id)),
            );
        }
    }
}

/// Validate the content of every user skill in the document: required
/// `SKILL.md`, safe relative file paths, well-formed `agentspace.json`, valid
/// non-reserved volume mount paths, and no mount-path collisions across skills.
fn validate_skills(issues: &mut Vec<ValidationIssue>, document: &ConfigDocument) {
    let mut mount_paths: BTreeMap<String, String> = BTreeMap::new();
    for skill in &document.spec.skills {
        let mut content_issues = Vec::new();
        crate::config::skill_validation::validate_skill_content(
            &mut content_issues,
            &skill.id,
            &skill.files,
            &mut mount_paths,
        );
        for content_issue in content_issues {
            let mut issue = ValidationIssue::new(content_issue.code, content_issue.message)
                .with_resource(format!("skill/{}", skill.id));
            if let Some(field) = content_issue.field {
                issue = issue.with_field(field);
            }
            issues.push(issue);
        }
    }
}

fn validate_skill_builtin_collisions(
    issues: &mut Vec<ValidationIssue>,
    document: &ConfigDocument,
    builtin_skill_ids: &BTreeSet<String>,
) {
    for skill in &document.spec.skills {
        if builtin_skill_ids.contains(&skill.id) {
            issues.push(
                ValidationIssue::new(
                    "builtin_skill_collision",
                    format!(
                        "user skill {:?} collides with an installation-owned builtin skill",
                        skill.id
                    ),
                )
                .with_resource(format!("skill/{}", skill.id))
                .with_field(format!("skills/{}/id", skill.id)),
            );
        }
    }
}

fn validate_agents(
    issues: &mut Vec<ValidationIssue>,
    document: &ConfigDocument,
    connection_ids: &BTreeSet<&str>,
    skill_ids: &BTreeSet<&str>,
    builtin_skill_ids: &BTreeSet<String>,
    check_skill_refs: bool,
) {
    for agent in &document.spec.agents {
        if validate_agent_id(&agent.id).is_err() {
            issues.push(
                ValidationIssue::new(
                    "invalid_agent_id",
                    format!("invalid agent id {:?}", agent.id),
                )
                .with_resource(format!("agent/{}", agent.id)),
            );
        }
        if let Some(connection) = &agent.connection
            && !connection_ids.contains(connection.as_str())
        {
            issues.push(
                ValidationIssue::new(
                    "unresolved_connection_reference",
                    format!(
                        "agent {:?} references unknown connection {connection:?}",
                        agent.id
                    ),
                )
                .with_resource(format!("agent/{}", agent.id))
                .with_field(format!("agents/{}/connection", agent.id)),
            );
        }
        if let Some(connection) = agent.cli.as_ref().and_then(|cli| cli.connection.as_ref())
            && !connection_ids.contains(connection.as_str())
        {
            issues.push(
                ValidationIssue::new(
                    "unresolved_connection_reference",
                    format!(
                        "agent {:?} CLI references unknown connection {connection:?}",
                        agent.id
                    ),
                )
                .with_resource(format!("agent/{}", agent.id))
                .with_field(format!("agents/{}/cli/connection", agent.id)),
            );
        }
        for skill in &agent.skills {
            if check_skill_refs
                && !skill_ids.contains(skill.as_str())
                && !builtin_skill_ids.contains(skill)
            {
                issues.push(
                    ValidationIssue::new(
                        "unresolved_skill_reference",
                        format!("agent {:?} references unknown skill {skill:?}", agent.id),
                    )
                    .with_resource(format!("agent/{}", agent.id))
                    .with_field(format!("agents/{}/skills", agent.id)),
                );
            }
        }
        validate_env_exclusive(
            issues,
            &format!("agent/{}", agent.id),
            agent.env.is_some(),
            agent.env_text.is_some(),
        );
    }
}

fn validate_kernels(issues: &mut Vec<ValidationIssue>, document: &ConfigDocument) {
    for kernel in &document.spec.kernel_configs {
        validate_env_exclusive(
            issues,
            &format!("kernelConfig/{}", kernel.harness.as_str()),
            kernel.env.is_some(),
            kernel.env_text.is_some(),
        );
    }
}

fn validate_gateways(
    issues: &mut Vec<ValidationIssue>,
    document: &ConfigDocument,
    agent_ids: &BTreeSet<&str>,
) {
    for gateway in &document.spec.gateways {
        if validate_gateway_id(&gateway.id).is_err() {
            issues.push(
                ValidationIssue::new(
                    "invalid_gateway_id",
                    format!("invalid gateway id {:?}", gateway.id),
                )
                .with_resource(format!("gateway/{}", gateway.id)),
            );
        }
        if !agent_ids.contains(gateway.agent.as_str()) {
            issues.push(
                ValidationIssue::new(
                    "unresolved_agent_reference",
                    format!(
                        "gateway {:?} references unknown agent {:?}",
                        gateway.id, gateway.agent
                    ),
                )
                .with_resource(format!("gateway/{}", gateway.id))
                .with_field(format!("gateways/{}/agent", gateway.id)),
            );
        }
        validate_env_exclusive(
            issues,
            &format!("gateway/{}", gateway.id),
            gateway.env.is_some(),
            gateway.env_text.is_some(),
        );
        if let Some(env) = &gateway.env {
            for key in env.keys() {
                if gateway.secrets.contains_key(key) {
                    issues.push(
                        ValidationIssue::new(
                            "duplicate_gateway_key",
                            format!(
                                "gateway {:?} key {key:?} appears in both env and secrets",
                                gateway.id
                            ),
                        )
                        .with_resource(format!("gateway/{}", gateway.id))
                        .with_field(format!("gateways/{}/env/{key}", gateway.id)),
                    );
                }
            }
        }
    }
}

fn validate_secret_refs(
    issues: &mut Vec<ValidationIssue>,
    document: &ConfigDocument,
    declared_secrets: &BTreeSet<&str>,
) {
    for reference in secret_references(document) {
        if !declared_secrets.contains(reference.name.as_str()) {
            issues.push(
                ValidationIssue::new(
                    "unresolved_secret_reference",
                    format!(
                        "field {} references undeclared secret {}",
                        reference.field, reference.name
                    ),
                )
                .with_field(reference.field),
            );
        }
    }
}

fn validate_env_exclusive(
    issues: &mut Vec<ValidationIssue>,
    resource: &str,
    has_env: bool,
    has_env_text: bool,
) {
    if has_env && has_env_text {
        issues.push(
            ValidationIssue::new(
                "env_conflict",
                format!("{resource} sets both env and envText, which are mutually exclusive"),
            )
            .with_resource(resource.to_owned()),
        );
    }
}

/// Return the given metadata name, or the default when it is blank.
#[must_use]
pub fn metadata_name_or_default(name: &str) -> String {
    if name.trim().is_empty() {
        DEFAULT_METADATA_NAME.to_owned()
    } else {
        name.to_owned()
    }
}
