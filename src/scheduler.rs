use bollard::Docker;
use bollard::models::{ContainerCreateBody, ContainerSummaryStateEnum};
use bollard::query_parameters::{
    CreateImageOptionsBuilder, ListContainersOptionsBuilder,
    RemoveContainerOptionsBuilder, RestartContainerOptionsBuilder,
    StartContainerOptionsBuilder, StopContainerOptionsBuilder,
};
use actix_web::rt::time::sleep;
use futures_util::StreamExt;
use serde::Serialize;
use std::time::Duration;

pub struct Scheduler {
    docker: Docker,
}

#[derive(Serialize)]
pub struct ContainerInfo {
    pub id: String,
    pub image: String,
    pub name: String,
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

    pub async fn status(&self, app_name: &str) -> String {
        let info = self.docker.inspect_container(app_name, None).await.unwrap();
        info.state
            .and_then(|s| s.status)
            .map(|s| s.to_string())
            .unwrap_or_else(|| "unknown".to_string())
    }

    pub async fn list(&self) -> Vec<ContainerInfo> {
        self.docker
            .list_containers(Some(ListContainersOptionsBuilder::default().all(true).build()))
            .await
            .unwrap()
            .into_iter()
            .map(|c| ContainerInfo {
                id: c.id.unwrap_or_default(),
                image: c.image.unwrap_or_default(),
                name: c.names
                    .and_then(|names| names.into_iter().next())
                    .map(|n| n.trim_start_matches('/').to_string())
                    .unwrap_or_default(),
            })
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

    async fn pull(&self, image: &str) {
        let image_with_tag = if image.contains(':') {
            image.to_string()
        } else {
            format!("{image}:latest")
        };

        let mut stream = self.docker.create_image(
            Some(CreateImageOptionsBuilder::default().from_image(&image_with_tag).build()),
            None,
            None,
        );
        while stream.next().await.is_some() {}
    }

    pub async fn deploy(&self, image: &str) -> String {
        let image_exists = self.docker.inspect_image(image).await.is_ok();
        if !image_exists {
            self.pull(image).await;
        }

        let response = self.docker.create_container(
            None::<bollard::query_parameters::CreateContainerOptions>,
            ContainerCreateBody {
                image: Some(image.to_string()),
                ..Default::default()
            }
        ).await.unwrap();

        self.docker.start_container(
            &response.id,
            Some(StartContainerOptionsBuilder::default().build()),
        ).await.unwrap();

        response.id
    }
}
