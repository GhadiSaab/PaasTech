use actix_web::rt::time::sleep;
use bollard::models::ContainerCreateBody;
use bollard::query_parameters::{
    CreateContainerOptionsBuilder, ListContainersOptionsBuilder, RemoveContainerOptionsBuilder,
    RestartContainerOptionsBuilder, StartContainerOptionsBuilder, StopContainerOptionsBuilder,
};
use bollard::{API_DEFAULT_VERSION, Docker};
use sqlx::PgPool;
use std::time::Duration;

use crate::registry::Registry;

#[allow(dead_code)]
pub struct Scheduler {
    docker: Docker,
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

    pub async fn stop(&self, pool: &PgPool, app_name: &str) {
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

        if let Err(e) = Registry::update_status(pool, app_name, "stopped").await {
            eprintln!("registry: failed to update status for {app_name}: {e}");
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

    pub async fn list(&self) -> Vec<String> {
        self.docker
            .list_containers(Some(
                ListContainersOptionsBuilder::default().all(true).build(),
            ))
            .await
            .unwrap()
            .into_iter()
            .filter_map(|c| c.names)
            .flatten()
            .map(|name| name.trim_start_matches('/').to_string())
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

    pub async fn deploy(&self, pool: &PgPool, app_name: &str, image: &str, port: i32) {
        if let Err(e) = self
            .docker
            .create_container(
                Some(
                    CreateContainerOptionsBuilder::default()
                        .name(app_name)
                        .build(),
                ),
                ContainerCreateBody {
                    image: Some(image.to_string()),
                    ..Default::default()
                },
            )
            .await
        {
            eprintln!("docker: failed to create {app_name}: {e}");
            return;
        }

        if let Err(e) = self
            .docker
            .start_container(
                app_name,
                Some(StartContainerOptionsBuilder::default().build()),
            )
            .await
        {
            eprintln!("docker: failed to start {app_name}: {e}");
            return;
        }

        let container_id = self
            .docker
            .inspect_container(app_name, None)
            .await
            .ok()
            .and_then(|info| info.id)
            .unwrap_or_default();

        if let Err(e) =
            Registry::save(pool, app_name, image, &container_id, port, "running", None).await
        {
            eprintln!("registry: failed to save app {app_name}: {e}");
        }
    }
}
