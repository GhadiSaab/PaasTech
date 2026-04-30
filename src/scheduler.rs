use bollard::Docker;
use bollard::models::{ContainerCreateBody, ContainerSummaryStateEnum};
use bollard::query_parameters::{
    CreateContainerOptionsBuilder, ListContainersOptionsBuilder,
    RemoveContainerOptionsBuilder, RestartContainerOptionsBuilder,
    StartContainerOptionsBuilder, StopContainerOptionsBuilder,
};
use actix_web::rt::time::sleep;
use std::time::Duration;

pub struct Scheduler {
    docker: Docker,
}

impl Scheduler {
    pub fn new() -> Self {
        let docker = Docker::connect_with_defaults().expect("Impossible de se connecter au Docker Engine");
        Self { docker }
    }

    pub async fn stop(&self, app_name: &str) {
        self.docker
            .stop_container(app_name, Some(StopContainerOptionsBuilder::default().build()))
            .await
            .unwrap();

        self.docker
            .remove_container(app_name, Some(RemoveContainerOptionsBuilder::default().build()))
            .await
            .unwrap();
    }

    pub async fn inspect(&self, app_name: &str) -> String {
        let info = self.docker.inspect_container(app_name, None).await.unwrap();
        let status = info
            .state
            .and_then(|s| s.status)
            .map(|s| s.to_string())
            .unwrap_or_else(|| "unknown".to_string());
        status
    }

    pub async fn list(&self) -> Vec<String> {
        self.docker
            .list_containers(Some(ListContainersOptionsBuilder::default().all(true).build()))
            .await
            .unwrap()
            .into_iter()
            .filter_map(|c| c.names)
            .flatten()
            .map(|name| name.trim_start_matches('/').to_string())
            .collect()
    }

    pub async fn restart(&self, app_name: &str) {
        self.docker
            .restart_container(app_name, Some(RestartContainerOptionsBuilder::default().build()))
            .await
            .unwrap();
    }

    pub async fn watch(&self) {
        loop {
            sleep(Duration::from_secs(5)).await;

            let containers = self
                .docker
                .list_containers(Some(ListContainersOptionsBuilder::default().all(true).build()))
                .await
                .unwrap();

            for container in containers {
                let is_exited = container
                    .state
                    .as_ref()
                    .map(|s| *s == ContainerSummaryStateEnum::EXITED)
                    .unwrap_or(false);

                if is_exited {
                    if let Some(names) = &container.names {
                        for name in names {
                            let name = name.trim_start_matches('/');
                            println!("watch: restarting exited container {name}");
                            self.restart(name).await;
                        }
                    }
                }
            }
        }
    }

    pub async fn deploy(&self, app_name: &str, image: &str) {
        self.docker.create_container(
            Some(CreateContainerOptionsBuilder::default().name(app_name).build()),
            ContainerCreateBody {
                image: Some(image.to_string()),
                ..Default::default()
            }
        ).await.unwrap();

        self.docker.start_container(
            app_name,
            Some(StartContainerOptionsBuilder::default().build()),
        ).await.unwrap();
    }
}