use std::{collections::BTreeMap, error::Error};

use agentspace_cli_rs::skills::{
    client::SkillsClient,
    http_client::HttpSkillsClient,
    model::{
        CreateSkillRequest, Skill, SkillSource, SkillSummary, SkillVersion, UpdateSkillRequest,
    },
};
use axum::{
    Json, Router,
    extract::Path,
    http::StatusCode,
    routing::{get, post},
};
use serde_json::{Value, json};
use tokio::net::TcpListener;

fn files(content: &str) -> BTreeMap<String, String> {
    BTreeMap::from([("SKILL.md".to_owned(), content.to_owned())])
}

fn skill(content: &str) -> Skill {
    Skill {
        skill_id: "weather-report".to_owned(),
        files: files(content),
        source: SkillSource::User,
    }
}

async fn start_server() -> Result<(HttpSkillsClient, tokio::task::JoinHandle<()>), Box<dyn Error>> {
    let app = Router::new()
        .route(
            "/skills",
            get(|| async {
                Json(vec![SkillSummary {
                    skill_id: "weather-report".to_owned(),
                    source: SkillSource::User,
                }])
            })
            .post(|Json(request): Json<Value>| async move {
                assert_eq!(request["creator_agent_id"], "weather-agent");
                Json(skill("# Created\n"))
            }),
        )
        .route(
            "/skills/{skill_id}",
            get(|Path(skill_id): Path<String>| async move {
                assert_eq!(skill_id, "weather-report");
                Json(skill("# Existing\n"))
            })
            .put(
                |Path(skill_id): Path<String>, Json(request): Json<Value>| async move {
                    assert_eq!(skill_id, "weather-report");
                    assert_eq!(request["files"]["SKILL.md"], "# Updated\n");
                    Json(skill("# Updated\n"))
                },
            )
            .delete(|Path(skill_id): Path<String>| async move {
                assert_eq!(skill_id, "weather-report");
                StatusCode::NO_CONTENT
            }),
        )
        .route(
            "/skills/{skill_id}/versions",
            get(|Path(skill_id): Path<String>| async move {
                Json(vec![SkillVersion {
                    skill_id,
                    version: 1,
                    created_at: "2026-08-16T00:00:00Z".to_owned(),
                    files: files("# Created\n"),
                }])
            }),
        )
        .route(
            "/skills/{skill_id}/versions/{version}/rollback",
            post(
                |Path((skill_id, version)): Path<(String, u64)>| async move {
                    assert_eq!(skill_id, "weather-report");
                    assert_eq!(version, 1);
                    Json(skill("# Created\n"))
                },
            ),
        )
        .route(
            "/skills/missing",
            get(|| async {
                (
                    StatusCode::NOT_FOUND,
                    Json(json!({ "detail": "skill not found: missing" })),
                )
            }),
        );
    let listener = TcpListener::bind("127.0.0.1:0").await?;
    let address = listener.local_addr()?;
    let handle = tokio::spawn(async move {
        let _result = axum::serve(listener, app).await;
    });
    Ok((
        HttpSkillsClient::new(&format!("http://{address}/skills"))?,
        handle,
    ))
}

#[tokio::test]
async fn http_client_covers_complete_skills_contract() -> Result<(), Box<dyn Error>> {
    let (client, server) = start_server().await?;

    assert_eq!(client.list_skills().await?.len(), 1);
    assert_eq!(
        client.get_skill("weather-report").await?.files,
        files("# Existing\n")
    );
    assert_eq!(
        client
            .create_skill(CreateSkillRequest {
                skill_id: "weather-report".to_owned(),
                files: files("# Created\n"),
                creator_agent_id: Some("weather-agent".to_owned()),
            })
            .await?
            .files,
        files("# Created\n")
    );
    assert_eq!(
        client
            .update_skill(
                "weather-report",
                UpdateSkillRequest {
                    files: files("# Updated\n"),
                },
            )
            .await?
            .files,
        files("# Updated\n")
    );
    assert_eq!(client.list_versions("weather-report").await?[0].version, 1);
    assert_eq!(
        client.rollback("weather-report", 1).await?.files,
        files("# Created\n")
    );
    client.delete_skill("weather-report").await?;

    server.abort();
    Ok(())
}

#[tokio::test]
async fn http_client_preserves_api_error_detail() -> Result<(), Box<dyn Error>> {
    let (client, server) = start_server().await?;

    let error = client
        .get_skill("missing")
        .await
        .map_or_else(|error| error, |_| panic!("missing skill must fail"));
    assert_eq!(
        error.to_string(),
        "API returned 404: skill not found: missing"
    );

    server.abort();
    Ok(())
}
