use actix_web::rt::time::sleep;
use sqlx::PgPool;
use std::time::Duration;
use uuid::Uuid;

use crate::registry::{ActiveAppProcess, Registry};

use super::{Scheduler, app_process_container_name};

impl Scheduler {
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

    async fn recreate_missing_process(&self, pool: &PgPool, process: &ActiveAppProcess) {
        let container_name = app_process_container_name(
            process.project_id,
            &process.app_name,
            &process.process_name,
        );
        println!("watch: process {container_name} is missing, recreating");

        sleep(Duration::from_millis(500)).await;
        let process = match Registry::get_active_process(pool, process.id).await {
            Ok(Some(process)) => process,
            Ok(None) => {
                eprintln!("watch: process {container_name} was deleted before recreate");
                return;
            }
            Err(e) => {
                eprintln!("watch: failed to reload process {container_name}: {e}");
                return;
            }
        };
        let _ = Registry::update_process_status(pool, process.id, "crashed").await;

        let image = match process.image_id.as_deref().filter(|s| !s.is_empty()) {
            Some(img) => img,
            None => {
                eprintln!("watch: cannot recreate process {container_name}: no image_id");
                return;
            }
        };

        let env_vars =
            match Registry::merged_process_env_vars(pool, process.project_id, process.id).await {
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
                process.replica_group.as_deref(),
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
        }
    }
}
