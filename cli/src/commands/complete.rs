use crate::api_base;
use clap_complete::engine::CompletionCandidate;
use serde::Deserialize;
use std::time::Duration;

fn fetch_sync<T: for<'de> Deserialize<'de>>(url: &str) -> Vec<T> {
    let Ok(rt) = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
    else {
        return vec![];
    };
    rt.block_on(async {
        let Ok(client) = reqwest::Client::builder()
            .timeout(Duration::from_secs(3))
            .build()
        else {
            return vec![];
        };
        match client.get(url).send().await {
            Ok(resp) => resp.json::<Vec<T>>().await.unwrap_or_default(),
            Err(_) => vec![],
        }
    })
}

pub fn app_names() -> Vec<CompletionCandidate> {
    #[derive(Deserialize)]
    struct App {
        name: String,
    }
    fetch_sync::<App>(&format!("{}/app", api_base()))
        .into_iter()
        .map(|a| CompletionCandidate::new(a.name))
        .collect()
}

pub fn resource_names() -> Vec<CompletionCandidate> {
    #[derive(Deserialize)]
    struct Resource {
        display_name: String,
    }
    fetch_sync::<Resource>(&format!("{}/resource", api_base()))
        .into_iter()
        .map(|r| CompletionCandidate::new(r.display_name))
        .collect()
}
