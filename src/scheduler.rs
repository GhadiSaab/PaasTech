use actix_web::rt::time::sleep;
use bollard::Docker;
use bollard::models::{
    ContainerCreateBody, EndpointSettings, HostConfig, NetworkConnectRequest, NetworkCreateRequest,
    PortBinding,
};
use bollard::query_parameters::{
    CreateContainerOptionsBuilder, CreateImageOptionsBuilder, ListContainersOptionsBuilder,
    LogsOptionsBuilder, RemoveContainerOptionsBuilder, RenameContainerOptionsBuilder,
    RestartContainerOptionsBuilder, StartContainerOptionsBuilder, StopContainerOptionsBuilder,
};
use futures_util::StreamExt;
use futures_util::TryStreamExt;
use serde::Serialize;
use sqlx::PgPool;
use std::collections::HashMap;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use crate::engine::ProcessType;
use crate::registry::{App, Registry};
use uuid::Uuid;

#[derive(Debug)]
pub enum DeployError {
    AppNotFound(String),
    PortRequired(String),
    Other(String),
}

impl std::fmt::Display for DeployError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::AppNotFound(message) | Self::PortRequired(message) | Self::Other(message) => {
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

fn build_traefik_labels(
    app_name: &str,
    internal_port: u16,
    version: &str,
    base_domain: &str,
    public_host: Option<&str>,
) -> HashMap<String, String> {
    let service_name = format!("{app_name}-{version}");
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
    labels
}

fn traefik_labels(
    app_name: &str,
    internal_port: u16,
    base_domain: &str,
    public_host: Option<&str>,
) -> HashMap<String, String> {
    let version = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs()
        .to_string();
    build_traefik_labels(app_name, internal_port, &version, base_domain, public_host)
}

fn resolve_domain(base_domain: Option<&str>) -> String {
    base_domain
        .map(|s| s.to_string())
        .or_else(|| std::env::var("BASE_DOMAIN").ok())
        .unwrap_or_else(|| "localhost".to_string())
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

    async fn wait_until_running(&self, app_name: &str, timeout: Duration) -> bool {
        let deadline = std::time::Instant::now() + timeout;
        while std::time::Instant::now() < deadline {
            if self.inspect(app_name).await == "running" {
                return true;
            }
            sleep(Duration::from_millis(500)).await;
        }

        false
    }

    async fn mark_app_status(pool: &PgPool, project_id: Uuid, app_name: &str, status: &str) {
        if let Err(e) = Registry::update_status(pool, project_id, app_name, status).await {
            eprintln!("watch: failed to update status for {app_name}: {e}");
        }
    }

    async fn restart_exited_app(
        &self,
        pool: &PgPool,
        project_id: Uuid,
        app_name: &str,
        container_name: &str,
    ) {
        println!("watch: app {app_name} crashed, restarting");
        Self::mark_app_status(pool, project_id, app_name, "crashed").await;

        match self.restart(container_name).await {
            Ok(()) => {
                if self
                    .wait_until_running(container_name, Duration::from_secs(10))
                    .await
                {
                    Self::mark_app_status(pool, project_id, app_name, "running").await;
                } else {
                    eprintln!("watch: app {app_name} did not reach running after restart");
                }
            }
            Err(e) => {
                eprintln!("watch: failed to restart {app_name}: {e}");
            }
        }
    }

    async fn restart_exited_process(
        &self,
        pool: &PgPool,
        process_id: uuid::Uuid,
        container_name: &str,
    ) {
        println!("watch: process {container_name} crashed, restarting");
        if let Err(e) = Registry::update_process_status(pool, process_id, "crashed").await {
            eprintln!("watch: failed to update process {container_name}: {e}");
        }

        match self.restart(container_name).await {
            Ok(()) => {
                if self
                    .wait_until_running(container_name, Duration::from_secs(10))
                    .await
                {
                    if let Err(e) =
                        Registry::update_process_status(pool, process_id, "running").await
                    {
                        eprintln!("watch: failed to update process {container_name}: {e}");
                    }
                } else {
                    eprintln!(
                        "watch: process {container_name} did not reach running after restart"
                    );
                    let _ = Registry::update_process_status(pool, process_id, "failed").await;
                }
            }
            Err(e) => {
                eprintln!("watch: failed to restart process {container_name}: {e}");
                let _ = Registry::update_process_status(pool, process_id, "failed").await;
            }
        }
    }

    async fn recreate_missing_app(&self, pool: &PgPool, app: &App) {
        println!("watch: app {} container is missing, recreating", app.name);
        Self::mark_app_status(pool, app.project_id, &app.name, "crashed").await;
        let project = match Registry::get_project_by_id(pool, app.project_id).await {
            Ok(Some(project)) => project,
            Ok(None) => {
                eprintln!("watch: cannot recreate {} without a project", app.name);
                return;
            }
            Err(e) => {
                eprintln!("watch: failed to load project for {}: {e}", app.name);
                return;
            }
        };

        let image = match app.image_id.as_deref().filter(|image| !image.is_empty()) {
            Some(image) => image,
            None => {
                eprintln!("watch: cannot recreate {} without an image_id", app.name);
                return;
            }
        };
        let internal_port = match app.internal_port {
            Some(port) => port as u16,
            None => {
                eprintln!(
                    "watch: cannot recreate {}: no internal_port recorded",
                    app.name
                );
                return;
            }
        };
        let host_port = match app.port {
            Some(port) => port as u16,
            None => match find_free_port() {
                Ok(port) => port,
                Err(e) => {
                    eprintln!(
                        "watch: failed to find port while recreating {}: {e}",
                        app.name
                    );
                    return;
                }
            },
        };

        let env_vars = match Registry::merged_env_vars(pool, app.project_id, app.id).await {
            Ok(vars) => vars,
            Err(e) => {
                eprintln!("watch: failed to fetch env vars for {}: {e}", app.name);
                return;
            }
        };

        match self
            .redeploy(
                pool,
                app.project_id,
                &project.network_name,
                &app.name,
                image,
                internal_port,
                host_port,
                env_vars,
                app.base_domain.as_deref(),
            )
            .await
        {
            Ok(()) => {
                if self
                    .wait_until_running(
                        &app_container_name(app.project_id, &app.name),
                        Duration::from_secs(10),
                    )
                    .await
                {
                    Self::mark_app_status(pool, app.project_id, &app.name, "running").await;
                } else {
                    eprintln!(
                        "watch: app {} did not reach running after recreate",
                        app.name
                    );
                }
            }
            Err(e) => {
                eprintln!("watch: failed to recreate {}: {e}", app.name);
            }
        }
    }

    async fn recreate_missing_process(
        &self,
        pool: &PgPool,
        process: &crate::registry::ActiveAppProcess,
    ) {
        let container_name = app_process_container_name(
            process.project_id,
            &process.app_name,
            &process.process_name,
        );
        println!("watch: process {container_name} is missing, recreating");
        let _ = Registry::update_process_status(pool, process.id, "crashed").await;

        let image = match process.image_id.as_deref().filter(|s| !s.is_empty()) {
            Some(img) => img,
            None => {
                eprintln!("watch: cannot recreate process {container_name}: no image_id");
                return;
            }
        };

        let env_vars =
            match Registry::merged_env_vars(pool, process.project_id, process.application_id).await
            {
                Ok(vars) => vars,
                Err(e) => {
                    eprintln!("watch: failed to fetch env vars for process {container_name}: {e}");
                    return;
                }
            };

        match self
            .start_process(
                process.project_id,
                &process.project_network,
                &process.app_name,
                &process.process_name,
                &process.process_type,
                image,
                process.internal_port.map(|p| p as u16),
                process.base_domain.as_deref(),
                process.public_host.as_deref(),
                env_vars,
            )
            .await
        {
            Ok(started) => {
                let wait_name = container_name.clone();
                if self
                    .wait_until_running(&wait_name, Duration::from_secs(10))
                    .await
                {
                    let _ = Registry::update_process_running(
                        pool,
                        process.id,
                        image,
                        &started.container_id,
                        started.internal_port.map(|p| p as i32),
                        started.host_port.map(|p| p as i32),
                    )
                    .await;
                } else {
                    eprintln!(
                        "watch: process {container_name} did not reach running after recreate"
                    );
                    let _ = Registry::update_process_status(pool, process.id, "failed").await;
                }
            }
            Err(e) => {
                eprintln!("watch: failed to recreate process {container_name}: {e}");
                let _ = Registry::update_process_status(pool, process.id, "failed").await;
            }
        }
    }

    pub async fn watch(&self, pool: &PgPool) {
        loop {
            sleep(Duration::from_secs(5)).await;

            match Registry::list_active_processes(pool).await {
                Ok(processes) => {
                    for process in processes {
                        let container_name = app_process_container_name(
                            process.project_id,
                            &process.app_name,
                            &process.process_name,
                        );
                        let status = self.inspect(&container_name).await;

                        match status.as_str() {
                            "exited" => {
                                self.restart_exited_process(pool, process.id, &container_name)
                                    .await;
                            }
                            "unknown" => {
                                self.recreate_missing_process(pool, &process).await;
                            }
                            _ => {}
                        }
                    }
                }
                Err(e) => eprintln!("watch: failed to list app processes: {e}"),
            }

            let apps = match Registry::list_all(pool).await {
                Ok(apps) => apps,
                Err(e) => {
                    eprintln!("watch: failed to list apps from registry: {e}");
                    continue;
                }
            };

            for app in apps {
                match Registry::list_processes(pool, app.id).await {
                    Ok(processes) if !processes.is_empty() => continue,
                    Ok(_) => {}
                    Err(e) => {
                        eprintln!("watch: failed to list processes for {}: {e}", app.name);
                        continue;
                    }
                }

                let should_be_running =
                    matches!(app.status.as_deref(), Some("running") | Some("crashed"));
                if !should_be_running {
                    continue;
                }

                let container_name = app_container_name(app.project_id, &app.name);
                let status = self.inspect(&container_name).await;

                match status.as_str() {
                    "exited" => {
                        self.restart_exited_app(pool, app.project_id, &app.name, &container_name)
                            .await
                    }
                    "unknown" => self.recreate_missing_app(pool, &app).await,
                    _ => {}
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

    async fn create_and_start(
        &self,
        container_name: &str,
        network_name: &str,
        image: &str,
        internal_port: u16,
        host_port: u16,
        labels: HashMap<String, String>,
        env_vars: Vec<String>,
    ) -> Result<String, Box<dyn std::error::Error + Send + Sync>> {
        self.ensure_network(network_name).await;
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
                    labels: Some(labels),
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
                let labels = traefik_labels(app_name, internal_port, &domain, public_host);
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
        project_id: Uuid,
        network_name: &str,
        app_name: &str,
        image: &str,
        internal_port: Option<u16>,
        env_vars: Vec<String>,
        base_domain: Option<&str>,
    ) -> Result<(), DeployError> {
        let image = self.pull(image).await?;
        let internal_port = self.resolve_internal_port(&image, internal_port).await?;

        let host_port = find_free_port().map_err(|e| {
            DeployError::Other(format!("Failed to find free port for {app_name}: {e}"))
        })?;

        let domain = resolve_domain(base_domain);
        let labels = traefik_labels(app_name, internal_port, &domain, None);
        let container_name = app_container_name(project_id, app_name);

        let container_id = self
            .create_and_start(
                &container_name,
                network_name,
                &image,
                internal_port,
                host_port,
                labels,
                {
                    let mut env_vars = env_vars;
                    env_vars.push(format!("PORT={internal_port}"));
                    env_vars
                },
            )
            .await
            .map_err(|e| DeployError::Other(format!("Failed to create/start {app_name}: {e}")))?;

        Registry::upsert_in_project(
            pool,
            project_id,
            app_name,
            &image,
            &container_id,
            Some(internal_port as i32),
            host_port as i32,
            "running",
            base_domain,
        )
        .await
        .map_err(|e| DeployError::Other(format!("Failed to save app {app_name}: {e}")))?;

        Ok(())
    }

    pub async fn redeploy(
        &self,
        pool: &PgPool,
        project_id: Uuid,
        network_name: &str,
        app_name: &str,
        image: &str,
        internal_port: u16,
        host_port: u16,
        env_vars: Vec<String>,
        base_domain: Option<&str>,
    ) -> Result<(), DeployError> {
        let image = self.pull(image).await?;
        let container_name = app_container_name(project_id, app_name);

        let _ = self
            .docker
            .stop_container(
                &container_name,
                Some(StopContainerOptionsBuilder::default().build()),
            )
            .await;

        let _ = self
            .docker
            .remove_container(
                &container_name,
                Some(RemoveContainerOptionsBuilder::default().build()),
            )
            .await;

        let domain = resolve_domain(base_domain);
        let labels = traefik_labels(app_name, internal_port, &domain, None);

        let container_id = self
            .create_and_start(
                &container_name,
                network_name,
                &image,
                internal_port,
                host_port,
                labels,
                {
                    let mut env_vars = env_vars;
                    env_vars.push(format!("PORT={internal_port}"));
                    env_vars
                },
            )
            .await
            .map_err(|e| DeployError::Other(format!("Failed to recreate {app_name}: {e}")))?;

        Registry::update_container_id(pool, project_id, app_name, &container_id)
            .await
            .map_err(|e| DeployError::Other(format!("Failed to update container_id: {e}")))?;

        Registry::update_status(pool, project_id, app_name, "running")
            .await
            .map_err(|e| {
                DeployError::Other(format!("Failed to update status for {app_name}: {e}"))
            })?;

        Ok(())
    }

    pub async fn rolling_update(
        &self,
        pool: &PgPool,
        project_id: Uuid,
        network_name: &str,
        name: &str,
        new_image: &str,
        internal_port: Option<u16>,
        env: Vec<String>,
        base_domain: Option<&str>,
    ) -> Result<(), DeployError> {
        let existing = Registry::get_in_project(pool, project_id, name)
            .await
            .map_err(|e| DeployError::Other(format!("Failed to load app {name}: {e}")))?
            .ok_or_else(|| DeployError::AppNotFound(format!("app not found: {name}")))?;
        let domain = resolve_domain(base_domain.or(existing.base_domain.as_deref()));
        let image = self.pull(new_image).await?;
        let internal_port = self.resolve_internal_port(&image, internal_port).await?;

        let version = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_secs()
            .to_string();
        let app_container = app_container_name(project_id, name);
        let canary_name = format!("{app_container}-canary-{version}");

        let canary_port = find_free_port().map_err(|e| {
            DeployError::Other(format!("Failed to find canary port for {name}: {e}"))
        })?;
        let port_key = format!("{}/tcp", internal_port);
        let mut port_bindings: HashMap<String, Option<Vec<PortBinding>>> = HashMap::new();
        port_bindings.insert(
            port_key.clone(),
            Some(vec![PortBinding {
                host_ip: Some("0.0.0.0".to_string()),
                host_port: Some(canary_port.to_string()),
            }]),
        );

        self.ensure_network(network_name).await;
        self.docker
            .create_container(
                Some(
                    CreateContainerOptionsBuilder::default()
                        .name(&canary_name)
                        .build(),
                ),
                ContainerCreateBody {
                    image: Some(image.clone()),
                    env: {
                        let mut e = env;
                        e.push(format!("PORT={internal_port}"));
                        Some(e)
                    },
                    exposed_ports: Some(vec![port_key]),
                    host_config: Some(HostConfig {
                        port_bindings: Some(port_bindings),
                        ..Default::default()
                    }),
                    networking_config: Some(bollard::models::NetworkingConfig {
                        endpoints_config: Some(project_net(network_name)),
                    }),
                    ..Default::default()
                },
            )
            .await
            .map_err(|e| {
                DeployError::Other(format!("Failed to create canary {canary_name}: {e}"))
            })?;

        self.docker
            .start_container(
                &canary_name,
                Some(StartContainerOptionsBuilder::default().build()),
            )
            .await
            .map_err(|e| {
                DeployError::Other(format!("Failed to start canary {canary_name}: {e}"))
            })?;

        let health_url = format!("http://localhost:{canary_port}/health");
        let client = reqwest::Client::builder()
            .timeout(Duration::from_millis(500))
            .build()
            .map_err(|e| DeployError::Other(format!("Failed to build health client: {e}")))?;

        let deadline = std::time::Instant::now() + Duration::from_secs(15);
        let mut healthy = false;
        while std::time::Instant::now() < deadline {
            if let Ok(resp) = client.get(&health_url).send().await
                && resp.status().is_success()
            {
                healthy = true;
                break;
            }
            sleep(Duration::from_millis(500)).await;
        }

        if !healthy {
            let _ = self
                .docker
                .stop_container(
                    &canary_name,
                    Some(StopContainerOptionsBuilder::default().build()),
                )
                .await;
            let _ = self
                .docker
                .remove_container(
                    &canary_name,
                    Some(RemoveContainerOptionsBuilder::default().build()),
                )
                .await;
            return Err(DeployError::Other(
                "health probe timed out after 15s; old container left running".to_string(),
            ));
        }

        // Release the app name without stopping the old container. Its Traefik labels
        // remain active while the replacement starts under the production name.
        let old_name = format!("{app_container}-old-{version}");
        let final_port = match find_free_port() {
            Ok(port) => port,
            Err(e) => {
                let _ = self
                    .docker
                    .stop_container(
                        &canary_name,
                        Some(StopContainerOptionsBuilder::default().build()),
                    )
                    .await;
                let _ = self
                    .docker
                    .remove_container(
                        &canary_name,
                        Some(RemoveContainerOptionsBuilder::default().build()),
                    )
                    .await;
                return Err(DeployError::Other(format!(
                    "Failed to find production port for {name}: {e}"
                )));
            }
        };
        let mut labels = build_traefik_labels(name, internal_port, &version, &domain, None);
        let router_name = format!("{name}-{version}");
        labels.insert(
            format!("traefik.http.routers.{router_name}.priority"),
            version.clone(),
        );
        if let Err(e) = self
            .docker
            .rename_container(
                &app_container,
                RenameContainerOptionsBuilder::default()
                    .name(&old_name)
                    .build(),
            )
            .await
        {
            let _ = self
                .docker
                .stop_container(
                    &canary_name,
                    Some(StopContainerOptionsBuilder::default().build()),
                )
                .await;
            let _ = self
                .docker
                .remove_container(
                    &canary_name,
                    Some(RemoveContainerOptionsBuilder::default().build()),
                )
                .await;
            return Err(DeployError::Other(format!(
                "Failed to rename {name} to {old_name}: {e}"
            )));
        }

        let final_id = match self
            .create_and_start(
                &app_container,
                network_name,
                &image,
                internal_port,
                final_port,
                labels,
                vec![format!("PORT={internal_port}")],
            )
            .await
        {
            Ok(id) => id,
            Err(e) => {
                let _ = self
                    .docker
                    .remove_container(
                        &app_container,
                        Some(RemoveContainerOptionsBuilder::default().force(true).build()),
                    )
                    .await;
                let _ = self
                    .docker
                    .rename_container(
                        &old_name,
                        RenameContainerOptionsBuilder::default()
                            .name(&app_container)
                            .build(),
                    )
                    .await;
                let _ = self
                    .docker
                    .stop_container(
                        &canary_name,
                        Some(StopContainerOptionsBuilder::default().build()),
                    )
                    .await;
                let _ = self
                    .docker
                    .remove_container(
                        &canary_name,
                        Some(RemoveContainerOptionsBuilder::default().build()),
                    )
                    .await;
                return Err(DeployError::Other(format!(
                    "Failed to create/start replacement {name}: {e}"
                )));
            }
        };

        let health_url = format!("http://localhost:{final_port}/health");
        let deadline = std::time::Instant::now() + Duration::from_secs(15);
        let mut healthy = false;

        while std::time::Instant::now() < deadline {
            if let Ok(resp) = client.get(&health_url).send().await
                && resp.status().is_success()
            {
                healthy = true;
                break;
            }
            sleep(Duration::from_millis(500)).await;
        }

        if !healthy {
            let _ = self
                .docker
                .remove_container(
                    &app_container,
                    Some(RemoveContainerOptionsBuilder::default().force(true).build()),
                )
                .await;
            let _ = self
                .docker
                .rename_container(
                    &old_name,
                    RenameContainerOptionsBuilder::default()
                        .name(&app_container)
                        .build(),
                )
                .await;
            let _ = self
                .docker
                .stop_container(
                    &canary_name,
                    Some(StopContainerOptionsBuilder::default().build()),
                )
                .await;
            let _ = self
                .docker
                .remove_container(
                    &canary_name,
                    Some(RemoveContainerOptionsBuilder::default().build()),
                )
                .await;
            return Err(DeployError::Other(
                "production health probe timed out after 15s; old container left running"
                    .to_string(),
            ));
        }

        let mut registry_error = None;
        for attempt in 1..=3 {
            match Registry::upsert_in_project(
                pool,
                project_id,
                name,
                &image,
                &final_id,
                Some(internal_port as i32),
                final_port as i32,
                "running",
                Some(&domain),
            )
            .await
            {
                Ok(_) => break,
                Err(e) if attempt < 3 => {
                    eprintln!("registry: failed to update {name} on attempt {attempt}: {e}");
                    sleep(Duration::from_millis(250 * attempt)).await;
                }
                Err(e) => {
                    registry_error = Some(e);
                    break;
                }
            }
        }

        if let Some(e) = registry_error {
            let _ = self
                .docker
                .remove_container(
                    &app_container,
                    Some(RemoveContainerOptionsBuilder::default().force(true).build()),
                )
                .await;
            let rollback = self
                .docker
                .rename_container(
                    &old_name,
                    RenameContainerOptionsBuilder::default()
                        .name(&app_container)
                        .build(),
                )
                .await;
            let _ = self
                .docker
                .stop_container(
                    &canary_name,
                    Some(StopContainerOptionsBuilder::default().build()),
                )
                .await;
            let _ = self
                .docker
                .remove_container(
                    &canary_name,
                    Some(RemoveContainerOptionsBuilder::default().build()),
                )
                .await;

            return match rollback {
                Ok(()) => Err(DeployError::Other(format!(
                    "Failed to update registry for {name}: {e}; rolled back to previous container"
                ))),
                Err(rollback_error) => Err(DeployError::Other(format!(
                    "Failed to update registry for {name}: {e}; rollback failed: {rollback_error}"
                ))),
            };
        }

        // Traefik batches Docker provider updates. Keep the old backend alive until
        // the replacement has had time to enter Traefik's dynamic configuration.
        sleep(Duration::from_secs(3)).await;

        let _ = self
            .docker
            .stop_container(
                &old_name,
                Some(StopContainerOptionsBuilder::default().build()),
            )
            .await;
        let _ = self
            .docker
            .remove_container(
                &old_name,
                Some(RemoveContainerOptionsBuilder::default().build()),
            )
            .await;

        // Tear down the canary — it was only used for health probing.
        let _ = self
            .docker
            .stop_container(
                &canary_name,
                Some(StopContainerOptionsBuilder::default().build()),
            )
            .await;
        let _ = self
            .docker
            .remove_container(
                &canary_name,
                Some(RemoveContainerOptionsBuilder::default().build()),
            )
            .await;

        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::{build_traefik_labels, exposed_tcp_ports, image_with_default_tag};

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
        let labels = build_traefik_labels("api", 8080, "123", "example.com", None);

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
    }

    #[test]
    fn build_traefik_labels_can_use_public_host() {
        let labels = build_traefik_labels(
            "api",
            8080,
            "123",
            "example.com",
            Some("api.datachef.localhost"),
        );

        assert_eq!(
            labels.get("traefik.http.routers.api-123.rule"),
            Some(&"Host(`api.datachef.localhost`)".to_string())
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
