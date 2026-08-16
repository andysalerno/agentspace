//! Environment-backed configuration shared by `AgentSpace` CLI capabilities.

use std::{
    env,
    error::Error,
    fmt::{self, Display, Formatter},
};

pub const AGENT_ID: &str = "AGENTSPACE_AGENT_ID";
pub const CLIENT_SERVICE_URL: &str = "AGENTSPACE_CLIENT_SERVICE_URL";
pub const SKILLS_API: &str = "AGENTSPACE_SKILLS_API";

#[derive(Debug, Eq, PartialEq)]
pub struct EnvironmentError {
    message: String,
}

impl EnvironmentError {
    fn missing_skills_api() -> Self {
        Self {
            message: format!(
                "skills API is not configured; pass --uri or set {SKILLS_API} or \
                 {CLIENT_SERVICE_URL}"
            ),
        }
    }
}

impl Display for EnvironmentError {
    fn fmt(&self, formatter: &mut Formatter<'_>) -> fmt::Result {
        formatter.write_str(&self.message)
    }
}

impl Error for EnvironmentError {}

#[must_use]
pub fn optional(name: &str) -> Option<String> {
    env::var(name).ok().filter(|value| !value.trim().is_empty())
}

pub fn skills_api_uri(explicit: Option<&str>) -> Result<String, EnvironmentError> {
    if let Some(uri) = explicit.filter(|uri| !uri.trim().is_empty()) {
        return Ok(uri.to_owned());
    }
    if let Some(uri) = optional(SKILLS_API) {
        return Ok(uri);
    }
    optional(CLIENT_SERVICE_URL)
        .map(|uri| format!("{}/skills", uri.trim_end_matches('/')))
        .ok_or_else(EnvironmentError::missing_skills_api)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn explicit_skills_uri_takes_precedence() {
        assert_eq!(
            skills_api_uri(Some("http://example.test/skills")),
            Ok("http://example.test/skills".to_owned())
        );
    }
}
