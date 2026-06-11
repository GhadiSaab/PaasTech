mod rolling;
mod watcher;

use actix_web::rt::time::sleep;
use bollard::Docker;
use bollard::models::{
    ContainerCreateBody, EndpointSettings, HostConfig, NetworkConnectRequest, NetworkCreateRequest,
    NetworkDisconnectRequest, PortBinding,
};
use bollard::query_parameters::{
    CreateContainerOptionsBuilder, CreateImageOptionsBuilder, ListContainersOptionsBuilder,
    LogsOptionsBuilder, RemoveContainerOptionsBuilder, RestartContainerOptionsBuilder,
    StartContainerOptionsBuilder, StopContainerOptionsBuilder,
};
use futures_util::StreamExt;
use futures_util::TryStreamExt;
use serde::Serialize;
use std::collections::HashMap;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use crate::engine::ProcessType;
use uuid::Uuid;

#[derive(Debug)]
pub enum DeployError {
    PortRequired(String),
    RolledBack(String),
    Other(String),
}

impl std::fmt::Display for DeployError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::PortRequired(message) | Self::RolledBack(message) | Self::Other(message) => {
                f.write_str(message)
            }
        }
    }
}

impl std::error::Error for DeployError {}

pub(crate) fn find_free_port() -> Result<u16, std::io::Error> {
    let listener = std::net::TcpListener::bind("0.0.0.0:0")?;
    Ok(listener.local_addr()?.port())
}

pub fn app_container_name(project_id: Uuid, app_name: &str) -> String {
    format!("paastech-{}-{}", project_id.simple(), app_name)
}

pub fn app_process_container_name(project_id: Uuid, app_name: &str, process_name: &str) -> String {
    format!(
        "{}-{}",
        app_container_name(project_id, app_name),
        process_name
    )
}

fn project_net(network_name: &str) -> HashMap<String, EndpointSettings> {
    let mut m = HashMap::new();
    m.insert(network_name.to_string(), EndpointSettings::default());
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

pub(super) fn build_traefik_labels(
    app_name: &str,
    process_name: Option<&str>,
    internal_port: u16,
    version: Option<&str>,
    base_domain: &str,
    public_host: Option<&str>,
) -> HashMap<String, String> {
    let service_base = process_name
        .map(|process_name| format!("{app_name}-{process_name}"))
        .unwrap_or_else(|| app_name.to_string());
    let service_name = match version {
        Some(v) => format!("{service_base}-{v}"),
        None => service_base.clone(),
    };
    let router_name = &service_name;
    let host = public_host
        .map(str::to_string)
        .unwrap_or_else(|| format!("{app_name}.{base_domain}"));

    let mut labels = HashMap::new();
    labels.insert("traefik.enable".to_string(), "true".to_string());
    labels.insert(
        format!("traefik.http.routers.{router_name}.rule"),
        format!("Host(`{host}`)"),
    );
    labels.insert(
        format!("traefik.http.routers.{router_name}.service"),
        service_name.clone(),
    );
    labels.insert(
        format!("traefik.http.services.{service_name}.loadbalancer.server.port"),
        internal_port.to_string(),
    );

    if base_domain != "localhost" {
        labels.insert(
            format!("traefik.http.routers.{router_name}.entrypoints"),
            "websecure".to_string(),
        );
        labels.insert(
            format!("traefik.http.routers.{router_name}.tls"),
            "true".to_string(),
        );
        labels.insert(
            format!("traefik.http.routers.{router_name}.tls.certresolver"),
            "letsencrypt".to_string(),
        );
        labels.insert(
            format!("traefik.http.routers.{router_name}.tls.domains[0].main"),
            base_domain.to_string(),
        );
        labels.insert(
            format!("traefik.http.routers.{router_name}.tls.domains[0].sans"),
            format!("*.{base_domain}"),
        );
    }

    labels
}

fn labels_for_project_network(
    mut labels: HashMap<String, String>,
    network_name: &str,
) -> HashMap<String, String> {
    labels.insert(
        "traefik.docker.network".to_string(),
        network_name.to_string(),
    );
    labels
}

fn traefik_labels(
    app_name: &str,
    process_name: Option<&str>,
    internal_port: u16,
    base_domain: &str,
    public_host: Option<&str>,
    replica_group: Option<&str>,
) -> HashMap<String, String> {
    if let Some(group) = replica_group {
        // All replicas share a fixed service name — Traefik load balances across them.
        return build_traefik_labels(
            app_name,
            Some(group),
            internal_port,
            None,
            base_domain,
            public_host,
        );
    }
    let version = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs()
        .to_string();
    build_traefik_labels(
        app_name,
        process_name,
        internal_port,
        Some(&version),
        base_domain,
        public_host,
    )
}

pub(super) fn resolve_domain(base_domain: Option<&str>) -> String {
    base_domain
        .map(|s| s.to_string())
        .or_else(|| std::env::var("BASE_DOMAIN").ok())
        .unwrap_or_else(|| "localhost".to_string())
}

pub(super) fn traefik_network_name() -> String {
    std::env::var("TRAEFIK_NETWORK").unwrap_or_else(|_| "paas-net".to_string())
}

#[derive(Clone)]
pub struct Scheduler {
    pub(super) docker: Docker,
    docker_host: String,
}

#[derive(Serialize)]
pub struct ContainerInfo {
    pub id: String,
    pub image: String,
    pub name: String,
}

pub struct ProcessStartResult {
    pub container_id: String,
    pub internal_port: Option<u16>,
    pub host_port: Option<u16>,
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

    pub async fn restart(&self, app_name: &str) -> Result<(), bollard::errors::Error> {
        self.docker
            .restart_container(
                app_name,
                Some(RestartContainerOptionsBuilder::default().build()),
            )
            .await
    }

    pub(super) async fn wait_until_running(&self, app_name: &str, timeout: Duration) -> bool {
        let deadline = std::time::Instant::now() + timeout;
        while std::time::Instant::now() < deadline {
            if self.inspect(app_name).await == "running" {
                return true;
            }
            sleep(Duration::from_millis(500)).await;
        }

        false
    }

    async fn image_exists_locally(&self, image: &str) -> bool {
        self.docker.inspect_image(image).await.is_ok()
    }

    pub(super) async fn pull(&self, image: &str) -> Result<String, DeployError> {
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

    pub(super) async fn resolve_internal_port(
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

    pub async fn ensure_network(&self, network_name: &str) {
        if self
            .docker
            .inspect_network(network_name, None)
            .await
            .is_ok()
        {
            return;
        }
        let _ = self
            .docker
            .create_network(NetworkCreateRequest {
                name: network_name.to_string(),
                ..Default::default()
            })
            .await;
    }

    pub async fn disconnect_from_network(&self, network_name: &str, container_name: &str) {
        let _ = self
            .docker
            .disconnect_network(
                network_name,
                NetworkDisconnectRequest {
                    container: container_name.to_string(),
                    force: Some(true),
                },
            )
            .await;
    }

    pub async fn remove_network(&self, network_name: &str) {
        let _ = self.docker.remove_network(network_name).await;
    }

    pub async fn ensure_container_on_network(
        &self,
        network_name: &str,
        container_name: &str,
    ) -> Result<(), bollard::errors::Error> {
        self.ensure_network(network_name).await;

        let container = self.docker.inspect_container(container_name, None).await?;
        let already_connected = container
            .network_settings
            .and_then(|settings| settings.networks)
            .is_some_and(|networks| networks.contains_key(network_name));

        if already_connected {
            return Ok(());
        }

        self.docker
            .connect_network(
                network_name,
                NetworkConnectRequest {
                    container: container_name.to_string(),
                    ..Default::default()
                },
            )
            .await
    }

    pub(super) async fn ensure_traefik_on_network(
        &self,
        network_name: &str,
    ) -> Result<(), bollard::errors::Error> {
        let container_name =
            std::env::var("TRAEFIK_CONTAINER").unwrap_or_else(|_| "paastech-traefik-1".to_string());
        match self
            .ensure_container_on_network(network_name, &container_name)
            .await
        {
            Ok(()) => Ok(()),
            Err(bollard::errors::Error::DockerResponseServerError {
                status_code: 404, ..
            }) => Ok(()),
            Err(e) => Err(e),
        }
    }

    #[allow(clippy::too_many_arguments)]
    pub(super) async fn create_and_start(
        &self,
        container_name: &str,
        network_name: &str,
        image: &str,
        internal_port: u16,
        host_port: u16,
        labels: HashMap<String, String>,
        env_vars: Vec<String>,
        traefik_network: Option<&str>,
    ) -> Result<String, Box<dyn std::error::Error + Send + Sync>> {
        self.ensure_network(network_name).await;
        if traefik_network.is_none() {
            self.ensure_traefik_on_network(network_name).await?;
        }
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
                        .name(container_name)
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
                        ..Default::default()
                    }),
                    labels: Some(labels_for_project_network(
                        labels,
                        traefik_network.unwrap_or(network_name),
                    )),
                    networking_config: Some(bollard::models::NetworkingConfig {
                        endpoints_config: Some(project_net(network_name)),
                    }),
                    ..Default::default()
                },
            )
            .await?;

        self.docker
            .start_container(
                container_name,
                Some(StartContainerOptionsBuilder::default().build()),
            )
            .await?;

        if let Some(traefik_net) = traefik_network
            && let Err(e) = self
                .ensure_container_on_network(traefik_net, container_name)
                .await
        {
            eprintln!("warn: failed to connect {container_name} to {traefik_net}: {e}");
        }

        let container_id = self
            .docker
            .inspect_container(container_name, None)
            .await
            .ok()
            .and_then(|info| info.id)
            .unwrap_or_default();

        Ok(container_id)
    }

    async fn create_and_start_worker(
        &self,
        container_name: &str,
        network_name: &str,
        image: &str,
        env_vars: Vec<String>,
    ) -> Result<String, Box<dyn std::error::Error + Send + Sync>> {
        self.ensure_network(network_name).await;

        self.docker
            .create_container(
                Some(
                    CreateContainerOptionsBuilder::default()
                        .name(container_name)
                        .build(),
                ),
                ContainerCreateBody {
                    image: Some(image.to_string()),
                    env: if env_vars.is_empty() {
                        None
                    } else {
                        Some(env_vars)
                    },
                    networking_config: Some(bollard::models::NetworkingConfig {
                        endpoints_config: Some(project_net(network_name)),
                    }),
                    ..Default::default()
                },
            )
            .await?;

        self.docker
            .start_container(
                container_name,
                Some(StartContainerOptionsBuilder::default().build()),
            )
            .await?;

        let container_id = self
            .docker
            .inspect_container(container_name, None)
            .await
            .ok()
            .and_then(|info| info.id)
            .unwrap_or_default();

        Ok(container_id)
    }

    #[allow(clippy::too_many_arguments)]
    pub async fn start_process(
        &self,
        project_id: Uuid,
        network_name: &str,
        app_name: &str,
        process_name: &str,
        process_type: &ProcessType,
        image: &str,
        internal_port: Option<u16>,
        base_domain: Option<&str>,
        public_host: Option<&str>,
        env_vars: Vec<String>,
        replica_group: Option<&str>,
    ) -> Result<ProcessStartResult, DeployError> {
        let image = self.pull(image).await?;
        let container_name = app_process_container_name(project_id, app_name, process_name);

        match process_type {
            ProcessType::Web => {
                let internal_port = self.resolve_internal_port(&image, internal_port).await?;
                let host_port = find_free_port().map_err(|e| {
                    DeployError::Other(format!(
                        "Failed to find free port for {container_name}: {e}"
                    ))
                })?;
                let domain = resolve_domain(base_domain);
                let traefik_net = (domain != "localhost").then(traefik_network_name);
                let labels = traefik_labels(
                    app_name,
                    Some(process_name),
                    internal_port,
                    &domain,
                    public_host,
                    replica_group,
                );
                let mut env_vars = env_vars;
                env_vars.push(format!("PORT={internal_port}"));
                let container_id = self
                    .create_and_start(
                        &container_name,
                        network_name,
                        &image,
                        internal_port,
                        host_port,
                        labels,
                        env_vars,
                        traefik_net.as_deref(),
                    )
                    .await
                    .map_err(|e| {
                        DeployError::Other(format!("Failed to create/start {container_name}: {e}"))
                    })?;

                Ok(ProcessStartResult {
                    container_id,
                    internal_port: Some(internal_port),
                    host_port: Some(host_port),
                })
            }
            ProcessType::Worker => {
                let container_id = self
                    .create_and_start_worker(&container_name, network_name, &image, env_vars)
                    .await
                    .map_err(|e| {
                        DeployError::Other(format!("Failed to create/start {container_name}: {e}"))
                    })?;

                Ok(ProcessStartResult {
                    container_id,
                    internal_port: None,
                    host_port: None,
                })
            }
        }
    }

    #[allow(clippy::too_many_arguments)]
    pub async fn start_service(
        &self,
        service_id: &str,
        network_name: &str,
        image: &str,
        container_port: u16,
        existing_port: Option<u16>,
        env_vars: Vec<String>,
        binds: Vec<String>,
    ) -> Result<(String, u16), Box<dyn std::error::Error + Send + Sync>> {
        self.ensure_network(network_name).await;
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
                        endpoints_config: Some(project_net(network_name)),
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

    pub fn get_logs_stream(
        &self,
        container_name: String,
        tail: Option<usize>,
    ) -> impl futures_util::Stream<Item = Result<String, bollard::errors::Error>> + Send + 'static
    {
        let docker = self.docker.clone();
        let tail_str = tail
            .map(|n| n.to_string())
            .unwrap_or_else(|| "all".to_string());

        futures_util::stream::once(async move {
            docker
                .logs(
                    &container_name,
                    Some(
                        LogsOptionsBuilder::default()
                            .stdout(true)
                            .stderr(true)
                            .follow(true)
                            .tail(&tail_str)
                            .build(),
                    ),
                )
                .map_ok(|log| log.to_string())
        })
        .flatten()
    }

    pub async fn start_garage_container(
        &self,
        admin_token: &str,
    ) -> Result<(String, u16), Box<dyn std::error::Error + Send + Sync>> {
        use crate::garage::{
            CONTAINER_NAME, admin_container_port, garage_container_config, garage_image,
            init_network,
        };

        // Remove stale container if present
        let _ = self
            .docker
            .stop_container(
                CONTAINER_NAME,
                Some(StopContainerOptionsBuilder::default().build()),
            )
            .await;
        let _ = self
            .docker
            .remove_container(
                CONTAINER_NAME,
                Some(RemoveContainerOptionsBuilder::default().build()),
            )
            .await;

        let config_dir = "/tmp/paastech/s3";
        std::fs::create_dir_all(config_dir)?;
        let config_path = format!("{}/garage.toml", config_dir);
        std::fs::write(&config_path, garage_container_config(admin_token))?;

        self.docker
            .create_image(
                Some(
                    CreateImageOptionsBuilder::default()
                        .from_image(&garage_image())
                        .build(),
                ),
                None,
                None,
            )
            .try_collect::<Vec<_>>()
            .await?;

        let admin_host_port = find_free_port()?;
        let port_key = format!("{}/tcp", admin_container_port());
        let mut port_bindings: HashMap<String, Option<Vec<PortBinding>>> = HashMap::new();
        port_bindings.insert(
            port_key.clone(),
            Some(vec![PortBinding {
                host_ip: Some("0.0.0.0".to_string()),
                host_port: Some(admin_host_port.to_string()),
            }]),
        );

        self.ensure_network(init_network()).await;

        self.docker
            .create_container(
                Some(
                    CreateContainerOptionsBuilder::default()
                        .name(CONTAINER_NAME)
                        .build(),
                ),
                ContainerCreateBody {
                    image: Some(garage_image().to_string()),
                    exposed_ports: Some(vec![port_key]),
                    host_config: Some(HostConfig {
                        port_bindings: Some(port_bindings),
                        binds: Some(vec![format!("{}:/etc/garage.toml:ro", config_path)]),
                        ..Default::default()
                    }),
                    networking_config: Some(bollard::models::NetworkingConfig {
                        endpoints_config: Some(project_net(init_network())),
                    }),
                    ..Default::default()
                },
            )
            .await?;

        self.docker
            .start_container(
                CONTAINER_NAME,
                Some(StartContainerOptionsBuilder::default().build()),
            )
            .await?;

        let container_id = self
            .docker
            .inspect_container(CONTAINER_NAME, None)
            .await
            .ok()
            .and_then(|info| info.id)
            .unwrap_or_default();

        Ok((container_id, admin_host_port))
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
}

#[cfg(test)]
mod tests {
    use super::{
        build_traefik_labels, exposed_tcp_ports, image_with_default_tag, labels_for_project_network,
    };

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
    fn build_traefik_labels_uses_versioned_router_and_service_names() {
        let labels = build_traefik_labels("api", None, 8080, Some("123"), "example.com", None);

        assert_eq!(
            labels.get("traefik.http.routers.api-123.rule"),
            Some(&"Host(`api.example.com`)".to_string())
        );
        assert_eq!(
            labels.get("traefik.http.routers.api-123.service"),
            Some(&"api-123".to_string())
        );
        assert_eq!(
            labels.get("traefik.http.services.api-123.loadbalancer.server.port"),
            Some(&"8080".to_string())
        );
        assert!(!labels.contains_key("traefik.http.routers.api.rule"));
        assert!(!labels.contains_key("traefik.http.routers.api.service"));
        assert_eq!(
            labels.get("traefik.http.routers.api-123.tls.domains[0].main"),
            Some(&"example.com".to_string())
        );
        assert_eq!(
            labels.get("traefik.http.routers.api-123.tls.domains[0].sans"),
            Some(&"*.example.com".to_string())
        );
    }

    #[test]
    fn build_traefik_labels_scopes_process_services_without_changing_host() {
        let labels =
            build_traefik_labels("app", Some("api"), 8000, Some("123"), "example.com", None);

        assert_eq!(
            labels.get("traefik.http.routers.app-api-123.rule"),
            Some(&"Host(`app.example.com`)".to_string())
        );
        assert_eq!(
            labels.get("traefik.http.routers.app-api-123.service"),
            Some(&"app-api-123".to_string())
        );
        assert_eq!(
            labels.get("traefik.http.services.app-api-123.loadbalancer.server.port"),
            Some(&"8000".to_string())
        );
        assert_eq!(
            labels.get("traefik.http.routers.app-api-123.tls.domains[0].main"),
            Some(&"example.com".to_string())
        );
        assert_eq!(
            labels.get("traefik.http.routers.app-api-123.tls.domains[0].sans"),
            Some(&"*.example.com".to_string())
        );
    }

    #[test]
    fn build_traefik_labels_does_not_add_tls_domains_for_localhost() {
        let labels = build_traefik_labels("app", None, 8080, Some("123"), "localhost", None);

        assert!(!labels.contains_key("traefik.http.routers.app-123.tls.domains[0].main"));
        assert!(!labels.contains_key("traefik.http.routers.app-123.tls.domains[0].sans"));
        assert!(!labels.contains_key("traefik.http.routers.app-123.tls"));
    }

    #[test]
    fn build_traefik_labels_can_use_public_host() {
        let labels = build_traefik_labels(
            "api",
            None,
            8080,
            Some("123"),
            "example.com",
            Some("api.datachef.localhost"),
        );

        assert_eq!(
            labels.get("traefik.http.routers.api-123.rule"),
            Some(&"Host(`api.datachef.localhost`)".to_string())
        );
    }

    #[test]
    fn labels_for_project_network_selects_reachable_network_for_traefik() {
        let labels = labels_for_project_network(
            build_traefik_labels("api", None, 8080, Some("123"), "example.com", None),
            "paastech-project",
        );

        assert_eq!(
            labels.get("traefik.docker.network"),
            Some(&"paastech-project".to_string())
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
