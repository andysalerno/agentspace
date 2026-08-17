use std::path::PathBuf;

use clap::{Args, Subcommand};
use serde::Serialize;

use crate::environment::{self, AGENT_ID};

use super::{
    client::SkillsClient,
    error::SkillsError,
    fs::collect_skill_directory,
    http_client::HttpSkillsClient,
    model::{CreateSkillRequest, Skill, SkillSource, SkillVersionSummary, UpdateSkillRequest},
};

#[derive(Args, Debug)]
pub struct SkillsArgs {
    /// Use this skills API URI instead of the environment configuration.
    #[arg(long, global = true)]
    pub uri: Option<String>,

    #[command(subcommand)]
    pub command: SkillsCommand,
}

#[derive(Debug, Subcommand)]
pub enum SkillsCommand {
    /// List available skills.
    Ls,
    /// Show a skill and all of its files.
    Get { skill_id: String },
    /// Create or replace a user skill from a local directory.
    Sync { directory: PathBuf },
    /// List saved versions of a user skill.
    Versions { skill_id: String },
    /// Restore a saved version, recording the restoration as a new version.
    Rollback { skill_id: String, version: u64 },
    /// Delete a user skill.
    Rm { skill_id: String },
}

pub async fn run(args: SkillsArgs, json: bool) -> Result<(), SkillsError> {
    let uri = environment::skills_api_uri(args.uri.as_deref())?;
    let client = HttpSkillsClient::new(&uri)?;
    execute(args.command, &client, json).await
}

pub async fn execute(
    command: SkillsCommand,
    client: &dyn SkillsClient,
    json: bool,
) -> Result<(), SkillsError> {
    match command {
        SkillsCommand::Ls => {
            let skills = client.list_skills().await?;
            print_value(&skills, json, |skills| {
                for skill in skills {
                    println!(
                        "{}\t{}\t{} files",
                        skill.skill_id, skill.source, skill.file_count
                    );
                }
            })
        }
        SkillsCommand::Get { skill_id } => {
            let skill = client.get_skill(&skill_id).await?;
            print_value(&skill, json, print_skill)
        }
        SkillsCommand::Sync { directory } => {
            let (skill_id, files) = collect_skill_directory(&directory)?;
            let existing = match client.get_skill(&skill_id).await {
                Ok(skill) => Some(skill),
                Err(error) if error.is_not_found() => None,
                Err(error) => return Err(error),
            };
            let (action, skill) = match existing {
                None => (
                    "created",
                    client
                        .create_skill(CreateSkillRequest {
                            skill_id,
                            files,
                            creator_agent_id: environment::optional(AGENT_ID),
                        })
                        .await?,
                ),
                Some(skill) if skill.source == SkillSource::User => (
                    "updated",
                    client
                        .update_skill(&skill.skill_id, UpdateSkillRequest { files })
                        .await?,
                ),
                Some(skill) => {
                    return Err(SkillsError::BuiltinReadOnly {
                        skill_id: skill.skill_id,
                    });
                }
            };
            let output = SyncOutput {
                action,
                skill: &skill,
            };
            print_value(&output, json, |_| {
                println!("{action} {} ({} files)", skill.skill_id, skill.files.len());
            })
        }
        SkillsCommand::Versions { skill_id } => {
            let versions = client.list_versions(&skill_id).await?;
            print_value(&versions, json, |versions| print_versions(versions))
        }
        SkillsCommand::Rollback { skill_id, version } => {
            let skill = client.rollback(&skill_id, version).await?;
            print_value(&skill, json, |_| {
                println!(
                    "rolled back {} to version {version} ({} files)",
                    skill.skill_id,
                    skill.files.len()
                );
            })
        }
        SkillsCommand::Rm { skill_id } => {
            client.delete_skill(&skill_id).await?;
            let output = serde_json::json!({ "removed": skill_id });
            print_value(&output, json, |_| {
                println!("removed {skill_id}");
            })
        }
    }
}

#[derive(Serialize)]
struct SyncOutput<'a> {
    action: &'a str,
    skill: &'a Skill,
}

fn print_skill(skill: &Skill) {
    println!("{} ({})", skill.skill_id, skill.source);
    for (path, content) in &skill.files {
        println!("\n--- {path} ---");
        print!("{content}");
        if !content.ends_with('\n') {
            println!();
        }
    }
}

fn print_versions(versions: &[SkillVersionSummary]) {
    for version in versions {
        println!(
            "{}\t{}\t{} files",
            version.version, version.created_at, version.file_count
        );
    }
}

fn print_value<T: Serialize>(
    value: &T,
    json: bool,
    human: impl FnOnce(&T),
) -> Result<(), SkillsError> {
    if json {
        let text =
            serde_json::to_string_pretty(value).map_err(|source| SkillsError::Json { source })?;
        println!("{text}");
    } else {
        human(value);
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use std::{
        collections::BTreeMap,
        fs,
        sync::{Arc, Mutex},
    };

    use async_trait::async_trait;
    use tempfile::tempdir;

    use super::*;
    use crate::api::ApiError;
    use crate::skills::model::SkillSummary;
    use reqwest::StatusCode;

    #[derive(Clone)]
    struct StubClient {
        existing: Option<SkillSource>,
        fail_get: bool,
        calls: Arc<Mutex<Vec<&'static str>>>,
    }

    impl StubClient {
        fn new(existing: Option<SkillSource>) -> Self {
            Self {
                existing,
                fail_get: false,
                calls: Arc::new(Mutex::new(Vec::new())),
            }
        }

        fn failing() -> Self {
            Self {
                existing: None,
                fail_get: true,
                calls: Arc::new(Mutex::new(Vec::new())),
            }
        }

        fn record(&self, call: &'static str) {
            self.calls
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .push(call);
        }

        fn calls(&self) -> Vec<&'static str> {
            self.calls
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .clone()
        }
    }

    #[async_trait]
    impl SkillsClient for StubClient {
        async fn list_skills(&self) -> Result<Vec<SkillSummary>, SkillsError> {
            unreachable!("not used by sync")
        }

        async fn get_skill(&self, skill_id: &str) -> Result<Skill, SkillsError> {
            self.record("get");
            if self.fail_get {
                return Err(SkillsError::Api(ApiError::Unavailable {
                    message: "offline".to_owned(),
                }));
            }
            self.existing.map_or_else(
                || {
                    Err(SkillsError::Api(ApiError::Response {
                        status: StatusCode::NOT_FOUND,
                        detail: format!("skill not found: {skill_id}"),
                    }))
                },
                |source| {
                    Ok(Skill {
                        skill_id: skill_id.to_owned(),
                        files: BTreeMap::new(),
                        source,
                    })
                },
            )
        }

        async fn create_skill(&self, request: CreateSkillRequest) -> Result<Skill, SkillsError> {
            self.record("create");
            Ok(Skill {
                skill_id: request.skill_id,
                files: request.files,
                source: SkillSource::User,
            })
        }

        async fn update_skill(
            &self,
            skill_id: &str,
            request: UpdateSkillRequest,
        ) -> Result<Skill, SkillsError> {
            self.record("update");
            Ok(Skill {
                skill_id: skill_id.to_owned(),
                files: request.files,
                source: SkillSource::User,
            })
        }

        async fn list_versions(
            &self,
            _skill_id: &str,
        ) -> Result<Vec<SkillVersionSummary>, SkillsError> {
            unreachable!("not used by sync")
        }

        async fn rollback(&self, _skill_id: &str, _version: u64) -> Result<Skill, SkillsError> {
            unreachable!("not used by sync")
        }

        async fn delete_skill(&self, _skill_id: &str) -> Result<(), SkillsError> {
            unreachable!("not used by sync")
        }
    }

    fn skill_directory() -> (tempfile::TempDir, PathBuf) {
        let root = tempdir().unwrap_or_else(|error| panic!("tempdir: {error}"));
        let directory = root.path().join("weather-report");
        fs::create_dir(&directory).unwrap_or_else(|error| panic!("create skill: {error}"));
        fs::write(directory.join("SKILL.md"), "# Weather\n")
            .unwrap_or_else(|error| panic!("write skill: {error}"));
        (root, directory)
    }

    #[tokio::test]
    async fn sync_creates_missing_skill() {
        let (_root, directory) = skill_directory();
        let client = StubClient::new(None);

        execute(SkillsCommand::Sync { directory }, &client, false)
            .await
            .unwrap_or_else(|error| panic!("sync: {error}"));

        assert_eq!(client.calls(), vec!["get", "create"]);
    }

    #[tokio::test]
    async fn sync_updates_user_skill() {
        let (_root, directory) = skill_directory();
        let client = StubClient::new(Some(SkillSource::User));

        execute(SkillsCommand::Sync { directory }, &client, false)
            .await
            .unwrap_or_else(|error| panic!("sync: {error}"));

        assert_eq!(client.calls(), vec!["get", "update"]);
    }

    #[tokio::test]
    async fn sync_refuses_builtin_skill() {
        let (_root, directory) = skill_directory();
        let client = StubClient::new(Some(SkillSource::Builtin));

        assert!(matches!(
            execute(SkillsCommand::Sync { directory }, &client, false).await,
            Err(SkillsError::BuiltinReadOnly { .. })
        ));
        assert_eq!(client.calls(), vec!["get"]);
    }

    #[tokio::test]
    async fn sync_propagates_non_not_found_probe_errors() {
        let (_root, directory) = skill_directory();
        let client = StubClient::failing();

        assert!(matches!(
            execute(SkillsCommand::Sync { directory }, &client, false).await,
            Err(SkillsError::Api(ApiError::Unavailable { .. }))
        ));
        assert_eq!(client.calls(), vec!["get"]);
    }
}
