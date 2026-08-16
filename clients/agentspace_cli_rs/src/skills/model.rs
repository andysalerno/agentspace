use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct Skill {
    pub skill_id: String,
    pub files: BTreeMap<String, String>,
    pub source: SkillSource,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct SkillSummary {
    pub skill_id: String,
    pub source: SkillSource,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct SkillVersion {
    pub skill_id: String,
    pub version: u64,
    pub created_at: String,
    pub files: BTreeMap<String, String>,
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum SkillSource {
    Builtin,
    User,
}

impl std::fmt::Display for SkillSource {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str(match self {
            Self::Builtin => "builtin",
            Self::User => "user",
        })
    }
}

#[derive(Debug, Serialize)]
pub struct CreateSkillRequest {
    pub skill_id: String,
    pub files: BTreeMap<String, String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub creator_agent_id: Option<String>,
}

#[derive(Debug, Serialize)]
pub struct UpdateSkillRequest {
    pub files: BTreeMap<String, String>,
}
