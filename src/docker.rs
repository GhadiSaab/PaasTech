use actix_web::{Error, error};
use reqwest::Client;
use serde::Deserialize;
use std::collections::HashMap;
use std::sync::LazyLock;
use uuid::Uuid;

#[derive(Deserialize)]
struct ServiceConfig {
    docker_image: String,
    container_image: String,
    port: u16,
    env_vars: Vec<EnvVarSpec>,
    config_file: Option<ConfigFileSpec>,
}

#[derive(Deserialize)]
#[serde(untagged)]
enum EnvVarSpec {
    Static { key: String, value: String },
    Generated { key: String, generate: String },
}

#[derive(Deserialize)]
struct ConfigFileSpec {
    template: String,
    container_path: String,
}

static REGISTRY: LazyLock<HashMap<String, ServiceConfig>> = LazyLock::new(|| {
    serde_json::from_str(include_str!("../services/services.json"))
        .expect("services/services.json is invalid")
});

fn config_template(service: &str, template: &str) -> &'static str {
    match (service, template) {
        ("s3", "garage.toml") => include_str!("../services/s3/garage.toml"),
        _ => panic!("No template '{}' for service '{}'", template, service),
    }
}

pub fn is_valid_service(name: &str) -> bool {
    REGISTRY.contains_key(name)
}

pub fn valid_services() -> Vec<&'static str> {
    let mut services: Vec<&'static str> = REGISTRY.keys().map(|s| s.as_str()).collect();
    services.sort();
    services
}

pub fn docker_image_for_service(name: &str) -> &'static str {
    REGISTRY
        .get(name)
        .expect("Unknown service")
        .docker_image
        .as_str()
}

pub fn container_image_for_service(name: &str) -> &'static str {
    REGISTRY
        .get(name)
        .expect("Unknown service")
        .container_image
        .as_str()
}

pub fn prepare_config_for_service(
    name: &str,
    service_id: &str,
) -> Result<Vec<String>, std::io::Error> {
    let config = REGISTRY.get(name).expect("Unknown service");
    let Some(ref cf) = config.config_file else {
        return Ok(vec![]);
    };
    let dir = format!("/tmp/paastech/services/{}", service_id);
    std::fs::create_dir_all(&dir)?;
    let config_path = format!("{}/{}", dir, cf.template);
    std::fs::write(&config_path, config_template(name, &cf.template))?;
    Ok(vec![format!("{}:{}:ro", config_path, cf.container_path)])
}

pub fn default_env_vars_for_service(name: &str) -> Vec<(String, String)> {
    REGISTRY
        .get(name)
        .expect("Unknown service")
        .env_vars
        .iter()
        .map(|spec| match spec {
            EnvVarSpec::Static { key, value } => (key.clone(), value.clone()),
            EnvVarSpec::Generated { key, generate } => {
                let value = match generate.as_str() {
                    "uuid_hex64" => {
                        format!("{}{}", Uuid::new_v4().simple(), Uuid::new_v4().simple())
                    }
                    _ => panic!("Unknown generator '{}'", generate),
                };
                (key.clone(), value)
            }
        })
        .collect()
}

pub fn service_port_for_service(name: &str) -> u16 {
    REGISTRY.get(name).expect("Unknown service").port
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
