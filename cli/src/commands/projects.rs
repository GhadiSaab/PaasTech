use crate::api_base;
use colored::Colorize;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::path::{Path, PathBuf};

#[derive(Deserialize)]
struct Project {
    name: String,
    network_name: String,
}

#[derive(Deserialize, Serialize)]
struct ProjectManifest {
    project: ManifestProject,
}

#[derive(Deserialize, Serialize)]
struct ManifestProject {
    name: String,
}

fn manifest_path() -> Result<PathBuf, String> {
    std::env::current_dir()
        .map(|dir| dir.join("paastech.toml"))
        .map_err(|e| format!("Failed to read current directory: {e}"))
}

fn find_manifest() -> Result<Option<PathBuf>, String> {
    let mut dir =
        std::env::current_dir().map_err(|e| format!("Failed to read current directory: {e}"))?;
    loop {
        let candidate = dir.join("paastech.toml");
        if candidate.is_file() {
            return Ok(Some(candidate));
        }
        if !dir.pop() {
            return Ok(None);
        }
    }
}

fn read_project_name(path: &Path) -> Result<String, String> {
    let content = std::fs::read_to_string(path)
        .map_err(|e| format!("Failed to read {}: {e}", path.display()))?;
    let manifest: ProjectManifest =
        toml::from_str(&content).map_err(|e| format!("Invalid {}: {e}", path.display()))?;
    Ok(manifest.project.name)
}

pub fn current_project() -> Result<Option<String>, String> {
    find_manifest()?
        .map(|path| read_project_name(&path))
        .transpose()
}

pub fn require_project() -> Result<String, String> {
    current_project()?.ok_or_else(|| {
        "No paastech.toml found. Run `paastech init <project-name>` first.".to_string()
    })
}

pub async fn init(name: &str) -> Result<(), String> {
    let client = reqwest::Client::new();
    let resp = client
        .post(format!("{}/project", api_base()))
        .json(&serde_json::json!({ "name": name }))
        .send()
        .await
        .map_err(|e| format!("Request failed: {e}"))?;

    match resp.status().as_u16() {
        201 | 409 => {}
        code => {
            let text = resp.text().await.unwrap_or_default();
            return Err(format!("Failed to create project ({}): {}", code, text));
        }
    }

    let manifest = ProjectManifest {
        project: ManifestProject {
            name: name.to_string(),
        },
    };
    let content =
        toml::to_string_pretty(&manifest).map_err(|e| format!("Failed to write manifest: {e}"))?;
    let path = manifest_path()?;
    std::fs::write(&path, content)
        .map_err(|e| format!("Failed to write {}: {e}", path.display()))?;

    println!("{} Project {} initialized", "✓".green(), name.bold());
    Ok(())
}

pub async fn info() -> Result<(), String> {
    let project = require_project()?;
    let resp = reqwest::get(format!("{}/project/{}", api_base(), project))
        .await
        .map_err(|e| format!("Request failed: {e}"))?;
    match resp.status().as_u16() {
        200 => {
            let p: Project = resp
                .json()
                .await
                .map_err(|e| format!("Failed to parse response: {e}"))?;
            println!("Name:    {}", p.name.bold());
            println!("Network: {}", p.network_name);
            Ok(())
        }
        404 => Err(format!("Project '{}' not found on server", project)),
        code => Err(format!("Server error: {}", code)),
    }
}

pub async fn list() -> Result<(), String> {
    let resp = reqwest::get(format!("{}/project", api_base()))
        .await
        .map_err(|e| format!("Request failed: {e}"))?;
    if !resp.status().is_success() {
        return Err(format!("Server error: {}", resp.status()));
    }
    let projects: Vec<Project> = resp
        .json()
        .await
        .map_err(|e| format!("Failed to parse response: {e}"))?;
    if projects.is_empty() {
        println!("No projects found.");
        return Ok(());
    }
    for project in projects {
        println!("{}\t{}", project.name.bold(), project.network_name);
    }
    Ok(())
}

pub async fn delete(name: &str) -> Result<(), String> {
    let url = format!("{}/project/{}", api_base(), name);
    let client = reqwest::Client::new();
    let resp = client
        .delete(&url)
        .send()
        .await
        .map_err(|e| format!("Request failed: {e}"))?;

    match resp.status().as_u16() {
        204 => {
            if let Ok(Some(path)) = find_manifest() {
                if let Ok(manifest_name) = read_project_name(&path) {
                    if manifest_name == name {
                        let _ = std::fs::remove_file(&path);
                    }
                }
            }
            println!("{} Project {} deleted", "✓".green(), name.bold());
            Ok(())
        }
        400 => {
            let text = resp.text().await.unwrap_or_default();
            Err(text)
        }
        404 => Err(format!("Project '{}' not found", name)),
        code => {
            let text = resp.text().await.unwrap_or_default();
            Err(format!("Server error ({}): {}", code, text))
        }
    }
}

pub async fn env_set(pair: &str) -> Result<(), String> {
    let project = require_project()?;
    let (key, value) = pair
        .split_once('=')
        .ok_or_else(|| "Invalid format: expected KEY=VALUE".to_string())?;
    let url = format!("{}/project/{}/env", api_base(), project);
    let mut env = match reqwest::get(&url).await {
        Ok(resp) if resp.status().is_success() => resp
            .json::<HashMap<String, String>>()
            .await
            .map_err(|e| format!("Failed to parse env vars: {e}"))?,
        Ok(resp) => return Err(format!("Server error: {}", resp.status())),
        Err(e) => return Err(format!("Request failed: {e}")),
    };
    env.insert(key.to_string(), value.to_string());
    let client = reqwest::Client::new();
    let resp = client
        .put(url)
        .json(&env)
        .send()
        .await
        .map_err(|e| format!("Request failed: {e}"))?;
    if !resp.status().is_success() {
        let status = resp.status();
        let text = resp.text().await.unwrap_or_default();
        return Err(format!("Server error ({}): {}", status, text));
    }
    println!("{} {}={}", "✓".green(), key.bold(), value);
    Ok(())
}

pub async fn env_list() -> Result<(), String> {
    let project = require_project()?;
    let resp = reqwest::get(format!("{}/project/{}/env", api_base(), project))
        .await
        .map_err(|e| format!("Request failed: {e}"))?;
    if !resp.status().is_success() {
        return Err(format!("Server error: {}", resp.status()));
    }
    let vars: HashMap<String, String> = resp
        .json()
        .await
        .map_err(|e| format!("Failed to parse env vars: {e}"))?;
    if vars.is_empty() {
        println!("No environment variables set.");
        return Ok(());
    }
    let mut sorted: Vec<_> = vars.iter().collect();
    sorted.sort_by_key(|(key, _)| key.as_str());
    for (key, value) in sorted {
        println!("{}={}", key.bold(), value);
    }
    Ok(())
}
