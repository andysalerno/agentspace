use async_trait::async_trait;

use super::{
    error::SkillsError,
    model::{CreateSkillRequest, Skill, SkillSummary, SkillVersion, UpdateSkillRequest},
};

#[async_trait]
pub trait SkillsClient: Send + Sync {
    async fn list_skills(&self) -> Result<Vec<SkillSummary>, SkillsError>;
    async fn get_skill(&self, skill_id: &str) -> Result<Skill, SkillsError>;
    async fn create_skill(&self, request: CreateSkillRequest) -> Result<Skill, SkillsError>;
    async fn update_skill(
        &self,
        skill_id: &str,
        request: UpdateSkillRequest,
    ) -> Result<Skill, SkillsError>;
    async fn list_versions(&self, skill_id: &str) -> Result<Vec<SkillVersion>, SkillsError>;
    async fn rollback(&self, skill_id: &str, version: u64) -> Result<Skill, SkillsError>;
    async fn delete_skill(&self, skill_id: &str) -> Result<(), SkillsError>;
}
