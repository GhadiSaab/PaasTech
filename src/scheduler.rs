use actix_web::rt::time::sleep;
use bollard::Docker;
use bollard::models::{
    ContainerCreateBody, EndpointSettings, HostConfig, NetworkCreateRequest, PortBinding,
};
use bollard::query_parameters::{
    CreateContainerOptionsBuilder, CreateImageOptionsBuilder, ListContainersOptionsBuilder,
    LogsOptionsBuilder, RemoveContainerOptionsBuilder, RestartContainerOptionsBuilder,
    StartContainerOptionsBuilder, StopContainerOptionsBuilder,
};
use futures_util::StreamExt;
use futures_util::TryStreamExt;
use serde::Serialize;
use sqlx::PgPool;
use std::collections::HashMap;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use crate::registry::Registry;

#[derive(Debug)]
pub enum DeployError {
    PortRequired(String),
    Other(String),
}

impl std::fmt::Display for DeployError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::PortRequired(message) | Self::Other(message) => f.write_str(message),
        }
    }
}

impl std::error::Error for DeployError {}

pub(crate) fn find_free_port() -> Result<u16, std::io::Error> {
    let listener = std::net::TcpListener::bind("0.0.0.0:0")?;
    Ok(listener.local_addr()?.port())
}

fn paas_net() -> HashMap<String, EndpointSettings> {
    let mut m = HashMap::new();
    m.insert("paas-net".to_string(), EndpointSettings::default());
    m
}

fn exposed_tcp_ports(exposed_ports: Option<Vec<String>>) -> Vec<u16> {
    let mut ports: Vec<u16> = exposed_ports
        .unwrap_or_default()
        .into_iter()
        .filter_map(|port| {
            let (port, protocol) = port.split_once('/')?;
            (protocol == "tcp").then(|| port.parse::<u16>().ok())?
        })
        .collect();
    ports.sort_unstable();
    ports.dedup();
    ports
}

fn image_with_default_tag(image: &str) -> String {
    let has_tag = match (image.rfind('/'), image.rfind(':')) {
        (_, None) => false,
        (Some(slash), Some(colon)) => colon > slash,
        (None, Some(_)) => true,
    };

    if image.contains('@') || has_tag {
        image.to_string()
    } else {
        format!("{image}:latest")
    }
}

fn traefik_labels(app_name: &str, internal_port: u16) -> HashMap<String, String> {
    let version = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs()
        .to_string();
    let service_name = format!("{app_name}-{version}");
    let base_domain = std::env::var("BASE_DOMAIN").unwrap_or_else(|_| "localhost".to_string());

    let mut labels = HashMap::new();
    labels.insert("traefik.enable".to_string(), "true".to_string());
    labels.insert(
        format!("traefik.http.routers.{app_name}.rule"),
        format!("Host(`{app_name}.{base_domain}`)"),
    );
    labels.insert(
        format!("traefik.http.routers.{app_name}.service"),
        service_name.clone(),
    );
    labels.insert(
        format!("traefik.http.services.{service_name}.loadbalancer.server.port"),
        internal_port.to_string(),
    );
    labels
}

#[derive(Clone)]
pub struct Scheduler {
    docker: Docker,
    docker_host: String,
}

#[derive(Serialize)]
pub struct ContainerInfo {
    pub id: String,
    pub image: String,
    pub name: String,
}

#[allow(dead_code)]
impl Scheduler {
    pub fn new() -> Self {
        let docker_host = std::env::var("DOCKER_HOST").unwrap_or_default();

        let docker = if docker_host.is_empty() {
            Docker::connect_with_defaults()
        } else {
            Docker::connect_with_host(docker_host.as_str())
        }
        .expect("Failed to connect to Docker API. If the socket was wrong, plz check the .env");
        Self {
            docker,
            docker_host,
        }
    }

    pub fn docker_host(&self) -> &str {
        &self.docker_host
    }

    pub async fn stop(&self, app_name: &str) {
        if let Err(e) = self
            .docker
            .stop_container(
                app_name,
                Some(StopContainerOptionsBuilder::default().build()),
            )
            .await
        {
            eprintln!("docker: failed to stop {app_name}: {e}");
        }

        if let Err(e) = self
            .docker
            .remove_container(
                app_name,
                Some(RemoveContainerOptionsBuilder::default().build()),
            )
            .await
        {
            eprintln!("docker: failed to remove {app_name}: {e}");
        }
    }

    pub async fn inspect(&self, app_name: &str) -> String {
        match self.docker.inspect_container(app_name, None).await {
            Ok(info) => info
                .state
                .and_then(|s| s.status)
                .map(|s| s.to_string())
                .unwrap_or_else(|| "unknown".to_string()),
            Err(_) => "unknown".to_string(),
        }
    }

    pub async fn list(&self) -> Vec<ContainerInfo> {
        self.docker
            .list_containers(Some(
                ListContainersOptionsBuilder::default().all(true).build(),
            ))
            .await
            .unwrap()
            .into_iter()
            .map(|c| ContainerInfo {
                id: c.id.unwrap_or_default(),
                image: c.image.unwrap_or_default(),
                name: c
                    .names
                    .and_then(|names| names.into_iter().next())
                    .map(|n| n.trim_start_matches('/').to_string())
                    .unwrap_or_default(),
            })
            .collect()
    }

    pub async fn restart(&self, app_name: &str) {
        if let Err(e) = self
            .docker
            .restart_container(
                app_name,
                Some(RestartContainerOptionsBuilder::default().build()),
            )
            .await
        {
            eprintln!("docker: failed to restart {app_name}: {e}");
        }
    }

    pub async fn watch(&self, pool: &PgPool) {
        loop {
            sleep(Duration::from_secs(5)).await;

            let apps = match Registry::list(pool).await {
                Ok(apps) => apps,
                Err(e) => {
                    eprintln!("watch: failed to list apps from registry: {e}");
                    continue;
                }
            };

            for app in apps {
                let status = self.inspect(&app.name).await;

                if status == "exited" {
                    println!("watch: app {} crashed, restarting", app.name);

                    if let Err(e) = Registry::update_status(pool, &app.name, "crashed").await {
                        eprintln!("watch: failed to update status for {}: {e}", app.name);
                    }

                    self.restart(&app.name).await;

                    if let Err(e) = Registry::update_status(pool, &app.name, "running").await {
                        eprintln!("watch: failed to update status for {}: {e}", app.name);
                    }
                }
            }
        }
    }

    async fn image_exists_locally(&self, image: &str) -> bool {
        self.docker.inspect_image(image).await.is_ok()
    }

    async fn pull(&self, image: &str) -> Result<String, DeployError> {
        let image_with_tag = image_with_default_tag(image);

        if self.image_exists_locally(&image_with_tag).await {
            return Ok(image_with_tag);
        }

        let mut stream = self.docker.create_image(
            Some(
                CreateImageOptionsBuilder::default()
                    .from_image(&image_with_tag)
                    .build(),
            ),
            None,
            None,
        );
        while let Some(result) = stream.next().await {
            let info = result
                .map_err(|e| DeployError::Other(format!("Failed to pull image {image}: {e}")))?;
            if let Some(message) = info.error_detail.and_then(|error| error.message) {
                return Err(DeployError::Other(format!(
                    "Failed to pull image {image}: {message}"
                )));
            }
        }

        Ok(image_with_tag)
    }

    async fn resolve_internal_port(
        &self,
        image: &str,
        requested_port: Option<u16>,
    ) -> Result<u16, DeployError> {
        if let Some(port) = requested_port {
            return Ok(port);
        }

        let image_info = self
            .docker
            .inspect_image(image)
            .await
            .map_err(|e| DeployError::Other(format!("Failed to inspect image {image}: {e}")))?;
        let ports = exposed_tcp_ports(image_info.config.and_then(|config| config.exposed_ports));

        match ports.as_slice() {
            [port] => Ok(*port),
            [] => Err(DeployError::PortRequired(format!(
                "Image {image} does not expose a TCP port. Provide the internal application port."
            ))),
            _ => Err(DeployError::PortRequired(format!(
                "Image {image} exposes multiple TCP ports ({ports:?}). Choose the internal application port."
            ))),
        }
    }

    async fn ensure_paas_net(&self) {
        if self.docker.inspect_network("paas-net", None).await.is_ok() {
            return;
        }
        let _ = self
            .docker
            .create_network(NetworkCreateRequest {
                name: "paas-net".to_string(),
                ..Default::default()
            })
            .await;
    }

    async fn create_and_start(
        &self,
        app_name: &str,
        image: &str,
        internal_port: u16,
        host_port: u16,
        labels: HashMap<String, String>,
    ) -> Result<String, Box<dyn std::error::Error + Send + Sync>> {
        self.ensure_paas_net().await;
        let port_key = format!("{}/tcp", internal_port);
        let mut port_bindings: HashMap<String, Option<Vec<PortBinding>>> = HashMap::new();
        port_bindings.insert(
            port_key.clone(),
            Some(vec![PortBinding {
                host_ip: Some("0.0.0.0".to_string()),
                host_port: Some(host_port.to_string()),
            }]),
        );

        self.docker
            .create_container(
                Some(
                    CreateContainerOptionsBuilder::default()
                        .name(app_name)
                        .build(),
                ),
                ContainerCreateBody {
                    image: Some(image.to_string()),
                    exposed_ports: Some(vec![port_key]),
                    host_config: Some(HostConfig {
                        port_bindings: Some(port_bindings),
                        ..Default::default()
                    }),
                    labels: Some(labels),
                    networking_config: Some(bollard::models::NetworkingConfig {
                        endpoints_config: Some(paas_net()),
                    }),
                    ..Default::default()
                },
            )
            .await?;

        self.docker
            .start_container(
                app_name,
                Some(StartContainerOptionsBuilder::default().build()),
            )
            .await?;

        let container_id = self
            .docker
            .inspect_container(app_name, None)
            .await
            .ok()
            .and_then(|info| info.id)
            .unwrap_or_default();

        Ok(container_id)
    }

    pub async fn start_service(
        &self,
        service_id: &str,
        image: &str,
        container_port: u16,
        existing_port: Option<u16>,
        env_vars: Vec<String>,
        binds: Vec<String>,
    ) -> Result<(String, u16), Box<dyn std::error::Error + Send + Sync>> {
        self.ensure_paas_net().await;
        self.docker
            .create_image(
                Some(
                    CreateImageOptionsBuilder::default()
                        .from_image(image)
                        .build(),
                ),
                None,
                None,
            )
            .try_collect::<Vec<_>>()
            .await?;

        let host_port = match existing_port {
            Some(p) => p,
            None => find_free_port()?,
        };

        let port_key = format!("{}/tcp", container_port);
        let mut port_bindings: HashMap<String, Option<Vec<PortBinding>>> = HashMap::new();
        port_bindings.insert(
            port_key.clone(),
            Some(vec![PortBinding {
                host_ip: Some("0.0.0.0".to_string()),
                host_port: Some(host_port.to_string()),
            }]),
        );

        self.docker
            .create_container(
                Some(
                    CreateContainerOptionsBuilder::default()
                        .name(service_id)
                        .build(),
                ),
                ContainerCreateBody {
                    image: Some(image.to_string()),
                    env: if env_vars.is_empty() {
                        None
                    } else {
                        Some(env_vars)
                    },
                    exposed_ports: Some(vec![port_key]),
                    host_config: Some(HostConfig {
                        port_bindings: Some(port_bindings),
                        binds: if binds.is_empty() { None } else { Some(binds) },
                        ..Default::default()
                    }),
                    networking_config: Some(bollard::models::NetworkingConfig {
                        endpoints_config: Some(paas_net()),
                    }),
                    ..Default::default()
                },
            )
            .await?;

        self.docker
            .start_container(
                service_id,
                Some(StartContainerOptionsBuilder::default().build()),
            )
            .await?;

        let container_id = self
            .docker
            .inspect_container(service_id, None)
            .await
            .ok()
            .and_then(|info| info.id)
            .unwrap_or_default();

        Ok((container_id, host_port))
    }

    pub async fn get_logs(
        &self,
        container_name: &str,
        tail: Option<usize>,
    ) -> Result<String, bollard::errors::Error> {
        let tail_str = tail
            .map(|n| n.to_string())
            .unwrap_or_else(|| "all".to_string());

        let mut stream = self.docker.logs(
            container_name,
            Some(
                LogsOptionsBuilder::default()
                    .stdout(true)
                    .stderr(true)
                    .tail(&tail_str)
                    .build(),
            ),
        );

        let mut output = String::new();
        while let Some(chunk) = stream.next().await {
            match chunk {
                Ok(log) => output.push_str(&log.to_string()),
                Err(e) => return Err(e),
            }
        }

        Ok(output)
    }

    pub async fn stop_service(&self, service_id: &str) -> Result<(), bollard::errors::Error> {
        self.docker
            .stop_container(
                service_id,
                Some(StopContainerOptionsBuilder::default().build()),
            )
            .await?;

        self.docker
            .remove_container(
                service_id,
                Some(RemoveContainerOptionsBuilder::default().build()),
            )
            .await?;

        Ok(())
    }

    pub async fn deploy(
        &self,
        pool: &PgPool,
        app_name: &str,
        image: &str,
        internal_port: u16,
    ) -> Result<(), DeployError> {
        let image = self.pull(image).await?;

        let host_port = find_free_port().map_err(|e| {
            DeployError::Other(format!("Failed to find free port for {app_name}: {e}"))
        })?;

        let labels = traefik_labels(app_name, internal_port);

        let container_id = self
            .create_and_start(app_name, &image, internal_port, host_port, labels)
            .await
            .map_err(|e| DeployError::Other(format!("Failed to create/start {app_name}: {e}")))?;

        Registry::save(
            pool,
            app_name,
            &image,
            &container_id,
            Some(internal_port as i32),
            host_port as i32,
            "running",
        )
        .await
        .map_err(|e| DeployError::Other(format!("Failed to save app {app_name}: {e}")))?;

        Ok(())
    }

    pub async fn redeploy(
        &self,
        pool: &PgPool,
        app_name: &str,
        image: &str,
        internal_port: u16,
        host_port: u16,
    ) -> Result<(), DeployError> {
        let image = self.pull(image).await?;

        let _ = self
            .docker
            .stop_container(
                app_name,
                Some(StopContainerOptionsBuilder::default().build()),
            )
            .await;

        let _ = self
            .docker
            .remove_container(
                app_name,
                Some(RemoveContainerOptionsBuilder::default().build()),
            )
            .await;

        let labels = traefik_labels(app_name, internal_port);

        let container_id = self
            .create_and_start(app_name, &image, internal_port, host_port, labels)
            .await
            .map_err(|e| DeployError::Other(format!("Failed to recreate {app_name}: {e}")))?;

        Registry::update_container_id(pool, app_name, &container_id)
            .await
            .map_err(|e| DeployError::Other(format!("Failed to update container_id: {e}")))?;

        Registry::update_status(pool, app_name, "running")
            .await
            .map_err(|e| {
                DeployError::Other(format!("Failed to update status for {app_name}: {e}"))
            })?;

        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::{exposed_tcp_ports, image_with_default_tag};

    #[test]
    fn exposed_tcp_ports_filters_protocols_and_duplicates() {
        assert_eq!(
            exposed_tcp_ports(Some(vec![
                "443/tcp".to_string(),
                "53/udp".to_string(),
                "80/tcp".to_string(),
                "80/tcp".to_string(),
            ])),
            vec![80, 443]
        );
    }

    #[test]
    fn image_with_default_tag_handles_registry_ports() {
        assert_eq!(image_with_default_tag("nginx"), "nginx:latest");
        assert_eq!(image_with_default_tag("nginx:alpine"), "nginx:alpine");
        assert_eq!(
            image_with_default_tag("localhost:5000/example"),
            "localhost:5000/example:latest"
        );
    }
}
