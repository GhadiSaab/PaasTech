use super::utils::{colored_status, spinner};
use crate::api_base;
use crate::commands::projects::{current_project, require_project};
use colored::Colorize;
use serde::Deserialize;
use std::collections::HashMap;

#[derive(Deserialize)]
struct Resource {
    id: String,
    display_name: String,
    name: String,
    version: String,
    status: String,
    application_ids: Vec<String>,
}

async fn find_apps(names: &[&str]) -> Result<Vec<String>, String> {
    if names.is_empty() {
        return Ok(Vec::new());
    }

    let url = match current_project()? {
        Some(project) => format!("{}/project/{}/app", api_base(), project),
        None => format!("{}/app", api_base()),
    };
    let resp = reqwest::get(url)
        .await
        .map_err(|e| format!("Request failed: {e}"))?;

    if !resp.status().is_success() {
        return Err(format!("Server error: {}", resp.status()));
    }

    #[derive(Deserialize)]
    struct AppEntry {
        id: String,
        name: String,
    }

    let apps: Vec<AppEntry> = resp
        .json()
        .await
        .map_err(|e| format!("Failed to parse response: {e}"))?;

    names
        .iter()
        .map(|&name| {
            apps.iter()
                .find(|a| a.name == name)
                .map(|a| a.id.clone())
                .ok_or_else(|| format!("App '{}' not found", name))
        })
        .collect()
}

fn resource_url(id: &str, action: Option<&str>) -> Result<reqwest::Url, String> {
    let raw = match action {
        Some(act) => format!("{}/resource/{}/{}", api_base(), id, act),
        None => format!("{}/resource/{}", api_base(), id),
    };
    reqwest::Url::parse(&raw).map_err(|e| format!("Invalid URL: {e}"))
}

async fn find_resource(name_or_id: &str) -> Result<Resource, String> {
    let url = match current_project()? {
        Some(project) => format!("{}/project/{}/resource", api_base(), project),
        None => format!("{}/resource", api_base()),
    };
    let resp = reqwest::get(url)
        .await
        .map_err(|e| format!("Request failed: {e}"))?;

    if !resp.status().is_success() {
        return Err(format!("Server error: {}", resp.status()));
    }

    let mut resources: Vec<Resource> = resp
        .json()
        .await
        .map_err(|e| format!("Failed to parse response: {e}"))?;

    if let Some(pos) = resources.iter().position(|r| r.id == name_or_id) {
        return Ok(resources.remove(pos));
    }

    let mut matches: Vec<Resource> = resources
        .drain(..)
        .filter(|r| r.display_name == name_or_id)
        .collect();

    match matches.len() {
        0 => Err(format!("Resource '{}' not found", name_or_id)),
        1 => Ok(matches.remove(0)),
        _ => Err(format!(
            "Multiple resources named '{}', use the UUID directly",
            name_or_id
        )),
    }
}

fn print_table(resources: &[Resource]) {
    let col_id = 36_usize;
    let col_name = resources
        .iter()
        .map(|r| r.display_name.len())
        .max()
        .unwrap_or(4)
        .max(4);
    let col_type = resources
        .iter()
        .map(|r| r.name.len())
        .max()
        .unwrap_or(4)
        .max(4);
    let col_version = resources
        .iter()
        .map(|r| r.version.len())
        .max()
        .unwrap_or(7)
        .max(7);
    let col_status = resources
        .iter()
        .map(|r| r.status.len())
        .max()
        .unwrap_or(6)
        .max(6);
    let col_links = 36_usize;

    let sep = format!(
        "+-{}-+-{}-+-{}-+-{}-+-{}-+-{}-+",
        "-".repeat(col_id),
        "-".repeat(col_name),
        "-".repeat(col_type),
        "-".repeat(col_version),
        "-".repeat(col_status),
        "-".repeat(col_links),
    );

    println!("{}", sep);
    println!(
        "| {:<col_id$} | {:<col_name$} | {:<col_type$} | {:<col_version$} | {:<col_status$} | {:<col_links$} |",
        "id".bold(),
        "name".bold(),
        "type".bold(),
        "version".bold(),
        "status".bold(),
        "linked apps".bold(),
        col_id = col_id,
        col_name = col_name,
        col_type = col_type,
        col_version = col_version,
        col_status = col_status,
        col_links = col_links,
    );
    println!("{}", sep);

    for r in resources {
        let (status_colored, status_len) = colored_status(&r.status);
        let status_pad = col_status.saturating_sub(status_len);
        let links = if r.application_ids.is_empty() {
            "-".to_string()
        } else {
            r.application_ids.join(", ")
        };

        println!(
            "| {:<col_id$} | {:<col_name$} | {:<col_type$} | {:<col_version$} | {}{} | {:<col_links$} |",
            r.id,
            r.display_name,
            r.name,
            r.version,
            status_colored,
            " ".repeat(status_pad),
            links,
            col_id = col_id,
            col_name = col_name,
            col_type = col_type,
            col_version = col_version,
            col_links = col_links,
        );
    }

    println!("{}", sep);
}

// POST /resource
pub async fn create(
    display_name: &str,
    service_type: &str,
    version: Option<&str>,
    links: &[&str],
    connection: Option<&str>,
) -> Result<(), String> {
    if service_type == "postgres" && !links.is_empty() && connection.is_none() {
        return Err(
            "Missing --connection. Specify the connection profile for linked apps, for example --connection sync or --connection asyncpg"
                .to_string(),
        );
    }

    let app_uuids = find_apps(links).await?;

    let mut body = serde_json::json!({
        "display_name": display_name,
        "name": service_type,
    });
    if let Some(version) = version {
        body["version"] = serde_json::Value::String(version.to_string());
    }
    if !app_uuids.is_empty() && connection.is_some() {
        body["attachments"] = serde_json::json!(
            app_uuids
                .iter()
                .map(|application_id| {
                    serde_json::json!({
                        "application_id": application_id,
                        "connection_profile": connection.expect("checked above"),
                    })
                })
                .collect::<Vec<_>>()
        );
    }

    let project = require_project()?;
    let pb = spinner(&format!("Creating resource {}...", display_name));
    let client = reqwest::Client::new();
    let resp = client
        .post(format!("{}/project/{}/resource", api_base(), project))
        .json(&body)
        .send()
        .await
        .map_err(|e| format!("Request failed: {e}"))?;

    #[derive(Deserialize)]
    struct Created {
        id: String,
    }

    let resource_id = match resp.status().as_u16() {
        201 => {
            resp.json::<Created>()
                .await
                .map_err(|e| format!("Failed to parse response: {e}"))?
                .id
        }
        400 => {
            pb.finish_and_clear();
            let text = resp.text().await.unwrap_or_default();
            return Err(format!("Invalid request: {}", text));
        }
        409 => {
            pb.finish_and_clear();
            let text = resp.text().await.unwrap_or_default();
            return Err(text);
        }
        code => {
            pb.finish_and_clear();
            let text = resp.text().await.unwrap_or_default();
            return Err(format!("Server error ({}): {}", code, text));
        }
    };

    if !app_uuids.is_empty() {
        let url = resource_url(&resource_id, None)?;
        let link_body = match connection {
            Some(connection) => serde_json::json!({
                "attachments": app_uuids
                    .iter()
                    .map(|application_id| {
                        serde_json::json!({
                            "application_id": application_id,
                            "connection_profile": connection,
                        })
                    })
                    .collect::<Vec<_>>()
            }),
            None => serde_json::json!({ "application_ids": app_uuids }),
        };
        let link_resp = client
            .patch(url)
            .json(&link_body)
            .send()
            .await
            .map_err(|e| format!("Request failed: {e}"))?;
        pb.finish_and_clear();
        if !link_resp.status().is_success() {
            let text = link_resp.text().await.unwrap_or_default();
            return Err(format!(
                "Resource created but failed to link apps: {}",
                text
            ));
        }
    } else {
        pb.finish_and_clear();
    }

    println!("{} Resource {} created", "✓".green(), display_name.bold());
    Ok(())
}

// GET /resource
pub async fn list() -> Result<(), String> {
    let project = require_project()?;
    let url = format!("{}/project/{}/resource", api_base(), project);
    let resp = reqwest::get(url)
        .await
        .map_err(|e| format!("Request failed: {e}"))?;

    if !resp.status().is_success() {
        return Err(format!("Server error: {}", resp.status()));
    }

    let resources: Vec<Resource> = resp
        .json()
        .await
        .map_err(|e| format!("Failed to parse response: {e}"))?;

    if resources.is_empty() {
        println!("No resources found.");
        return Ok(());
    }

    print_table(&resources);
    Ok(())
}

// GET /resource/{id}
pub async fn info(display_name: &str) -> Result<(), String> {
    let resource = find_resource(display_name).await?;
    let url = resource_url(&resource.id, None)?;

    let resp = reqwest::get(url)
        .await
        .map_err(|e| format!("Request failed: {e}"))?;

    match resp.status().as_u16() {
        200 => {
            let r: Resource = resp
                .json()
                .await
                .map_err(|e| format!("Failed to parse response: {e}"))?;
            let (status_colored, _) = colored_status(&r.status);
            println!("Name:    {}", r.display_name.bold());
            println!("Type:    {}", r.name);
            println!("Version: {}", r.version);
            println!("Status:  {}", status_colored);
            println!("ID:      {}", r.id);
            if r.application_ids.is_empty() {
                println!("Linked:  -");
            } else {
                println!("Linked:  {}", r.application_ids.join(", "));
            }
        }
        404 => return Err(format!("Resource '{}' not found", display_name)),
        code => return Err(format!("Server error: {}", code)),
    }

    Ok(())
}

// DELETE /resource/{id}
pub async fn delete(display_name: &str) -> Result<(), String> {
    let resource = find_resource(display_name).await?;
    let pb = spinner(&format!("Deleting {}...", display_name));
    let url = resource_url(&resource.id, None)?;

    let client = reqwest::Client::new();
    let resp = client
        .delete(url)
        .send()
        .await
        .map_err(|e| format!("Request failed: {e}"))?;
    pb.finish_and_clear();

    match resp.status().as_u16() {
        204 => println!("Resource {} successfully deleted", display_name.bold()),
        404 => return Err(format!("Resource '{}' not found", display_name)),
        code => return Err(format!("Server error: {}", code)),
    }

    Ok(())
}

// PATCH /resource/{id} — update version and/or linked apps
pub async fn edit(
    display_name: &str,
    version: Option<&str>,
    links: &[&str],
    connection: Option<&str>,
) -> Result<(), String> {
    let resource = find_resource(display_name).await?;

    if resource.name == "postgres" && !links.is_empty() && connection.is_none() {
        return Err(
            "Missing --connection. Specify the connection profile for linked apps, for example --connection sync or --connection asyncpg"
                .to_string(),
        );
    }

    let pb = spinner(&format!("Updating {}...", display_name));
    let url = resource_url(&resource.id, None)?;

    let mut body = serde_json::json!({});
    if let Some(v) = version {
        body["version"] = serde_json::Value::String(v.to_string());
    }
    if !links.is_empty() {
        let new_uuids = find_apps(links).await?;
        let mut ids: Vec<String> = resource.application_ids.clone();
        for uuid in new_uuids {
            if !ids.contains(&uuid) {
                ids.push(uuid);
            }
        }
        if let Some(connection) = connection {
            body["attachments"] = serde_json::json!(
                ids.iter()
                    .map(|application_id| {
                        serde_json::json!({
                            "application_id": application_id,
                            "connection_profile": connection,
                        })
                    })
                    .collect::<Vec<_>>()
            );
        } else {
            body["application_ids"] = serde_json::json!(ids);
        }
    }

    let client = reqwest::Client::new();
    let resp = client
        .patch(url)
        .json(&body)
        .send()
        .await
        .map_err(|e| format!("Request failed: {e}"))?;
    pb.finish_and_clear();

    match resp.status().as_u16() {
        200 => println!("{} Resource {} updated", "✓".green(), display_name.bold()),
        404 => return Err(format!("Resource '{}' not found", display_name)),
        code => {
            let text = resp.text().await.unwrap_or_default();
            return Err(format!("Server error ({}): {}", code, text));
        }
    }

    Ok(())
}

// PATCH /resource/{id} — link to one or more applications
pub async fn attach(
    display_name: &str,
    apps: &[&str],
    connection: Option<&str>,
) -> Result<(), String> {
    if apps.is_empty() {
        return Err("Specify at least one --app to attach".to_string());
    }
    let resource = find_resource(display_name).await?;
    if resource.name == "postgres" && connection.is_none() {
        return Err(
            "Missing --connection. Specify the connection profile for attached apps, for example --connection sync or --connection asyncpg"
                .to_string(),
        );
    }
    let new_uuids = find_apps(apps).await?;
    let pb = spinner(&format!("Attaching {}...", display_name));
    let url = resource_url(&resource.id, None)?;

    let mut ids: Vec<String> = resource.application_ids.clone();
    for uuid in new_uuids {
        if !ids.contains(&uuid) {
            ids.push(uuid);
        }
    }

    let body = match connection {
        Some(connection) => serde_json::json!({
            "attachments": ids
                .iter()
                .map(|application_id| {
                    serde_json::json!({
                        "application_id": application_id,
                        "connection_profile": connection,
                    })
                })
                .collect::<Vec<_>>()
        }),
        None => serde_json::json!({ "application_ids": ids }),
    };

    let client = reqwest::Client::new();
    let resp = client
        .patch(url)
        .json(&body)
        .send()
        .await
        .map_err(|e| format!("Request failed: {e}"))?;
    pb.finish_and_clear();

    match resp.status().as_u16() {
        200 => println!(
            "{} Resource {} attached to {}",
            "✓".green(),
            display_name.bold(),
            apps.join(", ")
        ),
        404 => return Err(format!("Resource '{}' not found", display_name)),
        code => return Err(format!("Server error: {}", code)),
    }

    Ok(())
}

// POST /resource/{id}/start
pub async fn start(display_name: &str) -> Result<(), String> {
    let resource = find_resource(display_name).await?;
    let pb = spinner(&format!("Starting {}...", display_name));
    let url = resource_url(&resource.id, Some("start"))?;

    let client = reqwest::Client::new();
    let resp = client
        .post(url)
        .send()
        .await
        .map_err(|e| format!("Request failed: {e}"))?;
    pb.finish_and_clear();

    match resp.status().as_u16() {
        200 => println!("{} Resource {} started", "✓".green(), display_name.bold()),
        404 => return Err(format!("Resource '{}' not found", display_name)),
        409 => return Err(format!("Resource '{}' is already running", display_name)),
        code => return Err(format!("Server error: {}", code)),
    }

    Ok(())
}

// POST /resource/{id}/stop
pub async fn stop(display_name: &str) -> Result<(), String> {
    let resource = find_resource(display_name).await?;
    let pb = spinner(&format!("Stopping {}...", display_name));
    let url = resource_url(&resource.id, Some("stop"))?;

    let client = reqwest::Client::new();
    let resp = client
        .post(url)
        .send()
        .await
        .map_err(|e| format!("Request failed: {e}"))?;
    pb.finish_and_clear();

    match resp.status().as_u16() {
        200 => println!("{} Resource {} stopped", "✓".green(), display_name.bold()),
        404 => return Err(format!("Resource '{}' not found", display_name)),
        409 => return Err(format!("Resource '{}' is already stopped", display_name)),
        code => return Err(format!("Server error: {}", code)),
    }

    Ok(())
}

// GET /resource/{id}/logs
pub async fn logs(display_name: &str, tail: Option<u32>) -> Result<(), String> {
    let resource = find_resource(display_name).await?;

    if resource.status == "stopped" {
        return Err(format!(
            "Resource '{}' is stopped — start it first to access logs",
            display_name
        ));
    }

    let mut url = resource_url(&resource.id, Some("logs"))?;
    if let Some(n) = tail {
        url.query_pairs_mut().append_pair("tail", &n.to_string());
    }

    let resp = reqwest::get(url)
        .await
        .map_err(|e| format!("Request failed: {e}"))?;

    match resp.status().as_u16() {
        200 => {
            let text = resp.text().await.unwrap_or_default();
            print!("{}", text);
        }
        404 => return Err(format!("Resource '{}' not found", display_name)),
        code => return Err(format!("Server error: {}", code)),
    }

    Ok(())
}

// PUT /resource/{id}/env — upserts a single KEY=VALUE, preserving existing vars
pub async fn env_set(display_name: &str, pair: &str) -> Result<(), String> {
    let (key, value) = pair
        .split_once('=')
        .ok_or_else(|| "Invalid format: expected KEY=VALUE".to_string())?;

    let resource = find_resource(display_name).await?;
    let pb = spinner(&format!("Setting env for {}...", display_name));

    let get_url = resource_url(&resource.id, Some("env"))?;
    let get_resp = reqwest::get(get_url)
        .await
        .map_err(|e| format!("Request failed: {e}"))?;

    let mut env_map: HashMap<String, String> = match get_resp.status().as_u16() {
        200 => get_resp
            .json()
            .await
            .map_err(|e| format!("Failed to parse existing env vars: {e}"))?,
        404 => HashMap::new(),
        code => {
            return Err(format!(
                "Failed to fetch existing env vars: server error {}",
                code
            ));
        }
    };

    env_map.insert(key.to_string(), value.to_string());

    let put_url = resource_url(&resource.id, Some("env"))?;
    let client = reqwest::Client::new();
    let resp = client
        .put(put_url)
        .json(&env_map)
        .send()
        .await
        .map_err(|e| format!("Request failed: {e}"))?;
    pb.finish_and_clear();

    match resp.status().as_u16() {
        200 => println!("{} {}={}", "✓".green(), key.bold(), value),
        404 => return Err(format!("Resource '{}' not found", display_name)),
        code => {
            let text = resp.text().await.unwrap_or_default();
            return Err(format!("Server error ({}): {}", code, text));
        }
    }

    Ok(())
}

// GET /service/{name}/versions
pub async fn versions(name: &str) -> Result<(), String> {
    let url = format!("{}/service/{}/versions", api_base(), name);
    let resp = reqwest::get(&url)
        .await
        .map_err(|e| format!("Request failed: {e}"))?;

    match resp.status().as_u16() {
        200 => {
            let tags: Vec<String> = resp
                .json()
                .await
                .map_err(|e| format!("Failed to parse response: {e}"))?;
            if tags.is_empty() {
                println!("No versions found for '{}'.", name);
            } else {
                println!("Available versions for {}:", name.bold());
                for tag in &tags {
                    println!("  {}", tag);
                }
            }
        }
        404 => return Err(format!("Service '{}' not found", name)),
        code => return Err(format!("Server error: {}", code)),
    }

    Ok(())
}

// GET /resource/{id}/env
pub async fn env_list(display_name: &str) -> Result<(), String> {
    let resource = find_resource(display_name).await?;
    let url = resource_url(&resource.id, Some("env"))?;

    let resp = reqwest::get(url)
        .await
        .map_err(|e| format!("Request failed: {e}"))?;

    match resp.status().as_u16() {
        200 => {
            let vars: HashMap<String, String> = resp
                .json()
                .await
                .map_err(|e| format!("Failed to parse response: {e}"))?;
            if vars.is_empty() {
                println!("No environment variables set.");
            } else {
                let mut sorted: Vec<_> = vars.iter().collect();
                sorted.sort_by_key(|(k, _)| k.as_str());
                for (k, v) in sorted {
                    println!("{}={}", k.bold(), v);
                }
            }
        }
        404 => return Err(format!("Resource '{}' not found", display_name)),
        code => return Err(format!("Server error: {}", code)),
    }

    Ok(())
}
