use actix_web::rt::time::sleep;
use sqlx::PgPool;
use std::time::Duration;
use uuid::Uuid;

use crate::registry::{ActiveAppProcess, App, Registry};

use super::{Scheduler, app_container_name, app_process_container_name, find_free_port};

impl Scheduler {
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

    async fn restart_exited_process(&self, pool: &PgPool, process_id: Uuid, container_name: &str) {
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

    async fn recreate_missing_process(&self, pool: &PgPool, process: &ActiveAppProcess) {
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
                if self
                    .wait_until_running(&container_name, Duration::from_secs(10))
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
}
