use actix_web::{Error, error};
use reqwest::Client;
use serde::Deserialize;
use uuid::Uuid;

pub const VALID_SERVICES: &[&str] = &["postgres", "redis", "s3"];

pub fn docker_image_for_service(name: &str) -> &'static str {
    match name {
        "postgres" => "library/postgres",
        "redis" => "library/redis",
        "s3" => "dxflrs/garage",
        _ => unreachable!(),
    }
}

pub fn container_image_for_service(name: &str) -> &'static str {
    match name {
        "postgres" => "postgres",
        "redis" => "redis",
        "s3" => "dxflrs/garage",
        _ => unreachable!(),
    }
}

pub fn default_env_vars_for_service(name: &str) -> Vec<(String, String)> {
    match name {
        "postgres" => vec![
            ("POSTGRES_USER".to_string(), "postgres".to_string()),
            ("POSTGRES_PASSWORD".to_string(), "postgres".to_string()),
            ("POSTGRES_DB".to_string(), "postgres".to_string()),
        ],
        "redis" => vec![("REDIS_PASSWORD".to_string(), "redis".to_string())],
        "s3" => vec![
            (
                "GARAGE_RPC_SECRET".to_string(),
                format!("{}{}", Uuid::new_v4().simple(), Uuid::new_v4().simple()),
            ),
            (
                "GARAGE_METADATA_DIR".to_string(),
                "/var/lib/garage/meta".to_string(),
            ),
            (
                "GARAGE_DATA_DIR".to_string(),
                "/var/lib/garage/data".to_string(),
            ),
        ],
        _ => unreachable!(),
    }
}

pub fn service_port_for_service(name: &str) -> u16 {
    match name {
        "postgres" => 5432,
        "redis" => 6379,
        "s3" => 3900,
        _ => unreachable!(),
    }
}

pub async fn validate_docker_tag(client: &Client, image: &str, tag: &str) -> Result<(), Error> {
    let url = format!(
        "https://hub.docker.com/v2/repositories/{}/tags/{}/",
        image, tag
    );
    let resp = client
        .get(&url)
        .send()
        .await
        .map_err(|_| error::ErrorInternalServerError("Failed to reach Docker Hub"))?;

    match resp.status().as_u16() {
        200 => Ok(()),
        404 => Err(error::ErrorBadRequest(format!(
            "Version '{}' does not exist for this service on Docker Hub",
            tag
        ))),
        _ => Err(error::ErrorInternalServerError(
            "Unexpected response from Docker Hub",
        )),
    }
}

pub async fn fetch_service_versions(
    client: &Client,
    service_name: &str,
) -> Result<Vec<String>, Error> {
    let docker_image = docker_image_for_service(service_name);
    let url = format!(
        "https://hub.docker.com/v2/repositories/{}/tags/?page_size=50&ordering=last_updated",
        docker_image
    );

    let resp = client
        .get(&url)
        .send()
        .await
        .map_err(|_| error::ErrorInternalServerError("Failed to reach Docker Hub"))?;

    if !resp.status().is_success() {
        return Err(error::ErrorInternalServerError(
            "Failed to fetch versions from Docker Hub",
        ));
    }

    let tags: DockerTagsResponse = resp
        .json()
        .await
        .map_err(|_| error::ErrorInternalServerError("Failed to parse Docker Hub response"))?;

    Ok(tags.results.into_iter().map(|t| t.name).collect())
}

#[derive(Deserialize)]
struct DockerTagsResponse {
    results: Vec<DockerTag>,
}

#[derive(Deserialize)]
struct DockerTag {
    name: String,
}
