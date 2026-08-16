use async_trait::async_trait;

use crate::api::ApiClient;

use super::{
    client::SkillsClient,
    error::SkillsError,
    model::{CreateSkillRequest, Skill, SkillSummary, SkillVersion, UpdateSkillRequest},
};

#[derive(Clone, Debug)]
pub struct HttpSkillsClient {
    api: ApiClient,
}

impl HttpSkillsClient {
    pub fn new(base_url: &str) -> Result<Self, SkillsError> {
        Ok(Self {
            api: ApiClient::new(base_url)?,
        })
    }
}

#[async_trait]
impl SkillsClient for HttpSkillsClient {
    async fn list_skills(&self) -> Result<Vec<SkillSummary>, SkillsError> {
        self.api.get(&[]).await.map_err(Into::into)
    }

    async fn get_skill(&self, skill_id: &str) -> Result<Skill, SkillsError> {
        self.api.get(&[skill_id]).await.map_err(Into::into)
    }

    async fn create_skill(&self, request: CreateSkillRequest) -> Result<Skill, SkillsError> {
        self.api.post(&[], Some(&request)).await.map_err(Into::into)
    }

    async fn update_skill(
        &self,
        skill_id: &str,
        request: UpdateSkillRequest,
    ) -> Result<Skill, SkillsError> {
        self.api
            .put(&[skill_id], &request)
            .await
            .map_err(Into::into)
    }

    async fn list_versions(&self, skill_id: &str) -> Result<Vec<SkillVersion>, SkillsError> {
        self.api
            .get(&[skill_id, "versions"])
            .await
            .map_err(Into::into)
    }

    async fn rollback(&self, skill_id: &str, version: u64) -> Result<Skill, SkillsError> {
        self.api
            .post::<(), Skill>(
                &[skill_id, "versions", &version.to_string(), "rollback"],
                None,
            )
            .await
            .map_err(Into::into)
    }

    async fn delete_skill(&self, skill_id: &str) -> Result<(), SkillsError> {
        self.api.delete(&[skill_id]).await.map_err(Into::into)
    }
}
