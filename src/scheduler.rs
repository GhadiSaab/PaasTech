use actix_web::rt::time::sleep;
use bollard::models::{ContainerCreateBody, HostConfig, PortBinding};
use bollard::query_parameters::{
    CreateContainerOptionsBuilder, CreateImageOptionsBuilder, ListContainersOptionsBuilder,
    RemoveContainerOptionsBuilder, RestartContainerOptionsBuilder, StartContainerOptionsBuilder,
    StopContainerOptionsBuilder,
};
use bollard::{API_DEFAULT_VERSION, Docker};
use futures_util::StreamExt;
use futures_util::TryStreamExt;
use serde::Serialize;
use sqlx::PgPool;
use std::collections::HashMap;
use std::time::Duration;

use crate::registry::Registry;

pub(crate) fn find_free_port() -> Result<u16, std::io::Error> {
    let listener = std::net::TcpListener::bind("0.0.0.0:0")?;
    Ok(listener.local_addr()?.port())
}

#[derive(Clone)]
pub struct Scheduler {
    docker: Docker,
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
        let docker =
            Docker::connect_with_unix("/run/user/1000/docker.sock", 120, API_DEFAULT_VERSION)
                .unwrap_or_else(|_| {
                    Docker::connect_with_defaults()
                        .expect("Impossible de se connecter au Docker Engine")
                });
        Self { docker }
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

    async fn pull(&self, image: &str) {
        let image_with_tag = if image.contains(':') {
            image.to_string()
        } else {
            format!("{image}:latest")
        };

        let mut stream = self.docker.create_image(
            Some(
                CreateImageOptionsBuilder::default()
                    .from_image(&image_with_tag)
                    .build(),
            ),
            None,
            None,
        );
        while stream.next().await.is_some() {}
    }

    async fn create_and_start(
        &self,
        app_name: &str,
        image: &str,
        internal_port: Option<u16>,
        host_port: u16,
    ) -> Result<String, Box<dyn std::error::Error + Send + Sync>> {
        let (exposed_ports, host_config_body) = match internal_port {
            Some(cp) => {
                let port_key = format!("{}/tcp", cp);
                let mut port_bindings: HashMap<String, Option<Vec<PortBinding>>> = HashMap::new();
                port_bindings.insert(
                    port_key.clone(),
                    Some(vec![PortBinding {
                        host_ip: Some("0.0.0.0".to_string()),
                        host_port: Some(host_port.to_string()),
                    }]),
                );
                (
                    Some(vec![port_key]),
                    Some(HostConfig {
                        port_bindings: Some(port_bindings),
                        ..Default::default()
                    }),
                )
            }
            None => (None, None),
        };

        self.docker
            .create_container(
                Some(
                    CreateContainerOptionsBuilder::default()
                        .name(app_name)
                        .build(),
                ),
                ContainerCreateBody {
                    image: Some(image.to_string()),
                    exposed_ports,
                    host_config: host_config_body,
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
        internal_port: Option<u16>,
    ) {
        self.pull(image).await;

        let host_port = match find_free_port() {
            Ok(p) => p,
            Err(e) => {
                eprintln!("docker: failed to find free port for {app_name}: {e}");
                return;
            }
        };

        let container_id = match self
            .create_and_start(app_name, image, internal_port, host_port)
            .await
        {
            Ok(id) => id,
            Err(e) => {
                eprintln!("docker: failed to create/start {app_name}: {e}");
                return;
            }
        };
        if let Err(e) = Registry::save(
            pool,
            app_name,
            image,
            &container_id,
            internal_port.map(|p| p as i32),
            host_port as i32,
            "running",
        )
        .await
        {
            eprintln!("registry: failed to save app {app_name}: {e}");
        }
    }

    pub async fn redeploy(
        &self,
        pool: &PgPool,
        app_name: &str,
        image: &str,
        internal_port: Option<u16>,
        host_port: u16,
    ) {
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

        self.pull(image).await;

        let container_id = match self
            .create_and_start(app_name, image, internal_port, host_port)
            .await
        {
            Ok(id) => id,
            Err(e) => {
                eprintln!("docker: failed to recreate {app_name}: {e}");
                return;
            }
        };

        if let Err(e) = Registry::update_container_id(pool, app_name, &container_id).await {
            eprintln!("registry: failed to update container_id for {app_name}: {e}");
        }

        if let Err(e) = Registry::update_status(pool, app_name, "running").await {
            eprintln!("registry: failed to update status for {app_name}: {e}");
        }
    }
}
