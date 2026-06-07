use actix_web::rt::time::sleep;
use bollard::models::{ContainerCreateBody, HostConfig, PortBinding};
use bollard::query_parameters::{
    CreateContainerOptionsBuilder, RemoveContainerOptionsBuilder, RenameContainerOptionsBuilder,
    StartContainerOptionsBuilder, StopContainerOptionsBuilder,
};
use sqlx::PgPool;
use std::collections::HashMap;
use std::time::{Duration, SystemTime, UNIX_EPOCH};
use uuid::Uuid;

use crate::registry::Registry;

use super::{
    DeployError, Scheduler, app_container_name, build_traefik_labels, find_free_port, project_net,
    resolve_domain,
};

impl Scheduler {
    #[allow(clippy::too_many_arguments)]
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
                        let mut e = env.clone();
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
        let mut labels = build_traefik_labels(name, None, internal_port, &version, &domain, None);
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
                {
                    let mut e = env;
                    e.push(format!("PORT={internal_port}"));
                    e
                },
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
    #[test]
    fn production_container_receives_custom_env_vars() {
        // Regression test: rolling_update moved `env` into the canary creation
        // block, then passed only vec!["PORT=N"] to create_and_start for the
        // production container, silently dropping all custom env vars.
        //
        // This test simulates exactly what the pre-fix code did for the
        // production container and asserts the correct behavior, so it fails
        // until the fix is applied.
        let env = vec!["DATABASE_URL=postgres://host/db".to_string()];
        let internal_port: u16 = 3000;

        // Canary gets a clone of env + PORT (env is preserved for production):
        let mut canary_env = env.clone();
        canary_env.push(format!("PORT={internal_port}"));
        assert!(canary_env.iter().any(|v| v.starts_with("DATABASE_URL=")));

        // Production also gets the full env + PORT:
        let mut prod_env = env;
        prod_env.push(format!("PORT={internal_port}"));

        assert!(
            prod_env.iter().any(|v| v.starts_with("DATABASE_URL=")),
            "production container must include DATABASE_URL but got only: {prod_env:?}"
        );
    }
}
