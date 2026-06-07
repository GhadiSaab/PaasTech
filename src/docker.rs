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
    connection_profiles: Option<HashMap<String, ConnectionProfileConfig>>,
    config_file: Option<ConfigFileSpec>,
}

#[derive(Deserialize)]
struct ConnectionProfileConfig {
    url_scheme: String,
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

pub fn connection_env_vars_for_service(
    name: &str,
    service_host: &str,
    service_port: u16,
    service_env: &HashMap<String, String>,
) -> Result<Vec<(String, String)>, String> {
    let host = service_host.to_string();
    let port_str = service_port.to_string();
    match name {
        "postgres" => {
            let Some(connection_profile) = service_env.get("PAASTECH_CONNECTION_PROFILE") else {
                let available = connection_profiles_for_service(name).join(", ");
                return Err(format!(
                    "Resource type 'postgres' requires a connection profile. Specify one of: {}",
                    available
                ));
            };
            validate_connection_profile_for_service(name, Some(connection_profile))?;
            let scheme = REGISTRY
                .get(name)
                .and_then(|config| config.connection_profiles.as_ref())
                .and_then(|profiles| profiles.get(connection_profile))
                .expect("validated connection profile")
                .url_scheme
                .as_str();
            let user = service_env
                .get("POSTGRES_USER")
                .map_or("postgres", String::as_str);
            let password = service_env
                .get("POSTGRES_PASSWORD")
                .map_or("postgres", String::as_str);
            let db = service_env
                .get("POSTGRES_DB")
                .map_or("postgres", String::as_str);
            let database_url = format!(
                "{}://{}:{}@{}:{}/{}",
                scheme, user, password, service_host, service_port, db
            );
            Ok(vec![
                ("POSTGRES_HOST".into(), host.clone()),
                ("POSTGRES_PORT".into(), port_str),
                ("DATABASE_URL".into(), database_url),
            ])
        }
        "redis" => Ok(vec![
            ("REDIS_HOST".into(), host.clone()),
            ("REDIS_PORT".into(), port_str),
            (
                "REDIS_URL".into(),
                format!("redis://{}:{}/0", service_host, service_port),
            ),
        ]),
        "s3" => Ok(vec![
            ("S3_HOST".into(), host.clone()),
            ("S3_PORT".into(), port_str),
            (
                "S3_ENDPOINT_URL".into(),
                format!("http://{}:{}", service_host, service_port),
            ),
        ]),
        _ => Ok(vec![
            (format!("{}_HOST", name.to_uppercase()), host),
            (format!("{}_PORT", name.to_uppercase()), port_str),
        ]),
    }
}

pub fn service_port_for_service(name: &str) -> u16 {
    REGISTRY.get(name).expect("Unknown service").port
}

pub fn default_version_for_service(_name: &str) -> &'static str {
    "latest"
}

pub fn connection_profiles_for_service(name: &str) -> Vec<&'static str> {
    let mut profiles: Vec<&'static str> = REGISTRY
        .get(name)
        .expect("Unknown service")
        .connection_profiles
        .as_ref()
        .map(|profiles| profiles.keys().map(|s| s.as_str()).collect())
        .unwrap_or_default();
    profiles.sort();
    profiles
}

pub fn validate_connection_profile_for_service(
    name: &str,
    connection_profile: Option<&str>,
) -> Result<(), String> {
    let config = REGISTRY.get(name).expect("Unknown service");
    let Some(profiles) = &config.connection_profiles else {
        if let Some(profile) = connection_profile {
            return Err(format!(
                "Resource type '{}' does not support connection profile '{}'",
                name, profile
            ));
        }
        return Ok(());
    };

    let Some(profile) = connection_profile else {
        let available = connection_profiles_for_service(name).join(", ");
        return Err(format!(
            "Resource type '{}' requires a connection profile. Specify one of: {}",
            name, available
        ));
    };

    if !profiles.contains_key(profile) {
        let available = connection_profiles_for_service(name).join(", ");
        return Err(format!(
            "Invalid connection profile '{}' for resource type '{}'. Specify one of: {}",
            profile, name, available
        ));
    }

    Ok(())
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn postgres_attachment_uses_requested_database_url_profile() {
        let mut env = HashMap::from([
            ("POSTGRES_USER".to_string(), "postgres".to_string()),
            ("POSTGRES_PASSWORD".to_string(), "postgres".to_string()),
            ("POSTGRES_DB".to_string(), "postgres".to_string()),
        ]);
        env.insert(
            "PAASTECH_CONNECTION_PROFILE".to_string(),
            "sync".to_string(),
        );

        let vars: HashMap<_, _> =
            connection_env_vars_for_service("postgres", "pg-service", 5432, &env)
                .expect("sync profile should be valid")
                .into_iter()
                .collect();

        assert_eq!(
            vars.get("DATABASE_URL").map(String::as_str),
            Some("postgresql://postgres:postgres@pg-service:5432/postgres")
        );
        assert!(!vars.contains_key("DATABASE_ASYNC_URL"));
        assert!(!vars.contains_key("DATABASE_SYNC_URL"));

        env.insert(
            "PAASTECH_CONNECTION_PROFILE".to_string(),
            "asyncpg".to_string(),
        );
        let vars: HashMap<_, _> =
            connection_env_vars_for_service("postgres", "pg-service", 5432, &env)
                .expect("asyncpg profile should be valid")
                .into_iter()
                .collect();
        assert_eq!(
            vars.get("DATABASE_URL").map(String::as_str),
            Some("postgresql+asyncpg://postgres:postgres@pg-service:5432/postgres")
        );
    }

    #[test]
    fn postgres_attachment_requires_connection_profile() {
        let env = HashMap::from([
            ("POSTGRES_USER".to_string(), "postgres".to_string()),
            ("POSTGRES_PASSWORD".to_string(), "postgres".to_string()),
            ("POSTGRES_DB".to_string(), "postgres".to_string()),
        ]);

        let err = connection_env_vars_for_service("postgres", "pg-service", 5432, &env)
            .expect_err("missing profile should fail");

        assert!(err.contains("requires a connection profile"));
    }

    #[test]
    fn redis_attachment_uses_unauthenticated_default_url() {
        let env = HashMap::new();

        let vars: HashMap<_, _> =
            connection_env_vars_for_service("redis", "redis-service", 6379, &env)
                .expect("redis should not require a connection profile")
                .into_iter()
                .collect();

        assert_eq!(
            vars.get("REDIS_URL").map(String::as_str),
            Some("redis://redis-service:6379/0")
        );
    }
}
