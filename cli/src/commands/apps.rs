use super::utils::{colored_status, http_client, spinner};
use crate::api_base;
use crate::commands::projects::{current_project, require_project};
use colored::Colorize;
use serde::Deserialize;
use serde_json::Value;
use std::fs::File;
use std::io::{Read, Write};
use std::path::{Component, Path, PathBuf};
use std::time::Duration;
use walkdir::WalkDir;
use zip::write::SimpleFileOptions;

#[derive(Deserialize)]
struct App {
    name: String,
    status: String,
    processes: Vec<AppProcess>,
    #[allow(dead_code)]
    env: Option<Value>,
    created_at: Option<String>,
}

#[derive(Deserialize)]
struct AppProcess {
    name: String,
    image_id: Option<String>,
    host_port: Option<i32>,
}

#[derive(Deserialize)]
struct ProjectManifest {
    project: ManifestProject,
}

#[derive(Deserialize)]
struct ManifestProject {
    name: String,
}

fn print_table(apps: &[App]) {
    let col_name = apps.iter().map(|a| a.name.len()).max().unwrap_or(4).max(4);
    let col_image = apps
        .iter()
        .map(|a| {
            primary_process(a)
                .and_then(|p| p.image_id.as_deref())
                .unwrap_or("-")
                .len()
        })
        .max()
        .unwrap_or(5)
        .max(5);
    let col_port = apps
        .iter()
        .map(|a| {
            primary_process(a)
                .and_then(|p| p.host_port)
                .map(|p| p.to_string().len())
                .unwrap_or(1)
        })
        .max()
        .unwrap_or(4)
        .max(4);
    let col_status = apps
        .iter()
        .map(|a| a.status.len())
        .max()
        .unwrap_or(6)
        .max(6);
    let col_created = 26_usize;

    let sep = format!(
        "+-{}-+-{}-+-{}-+-{}-+-{}-+",
        "-".repeat(col_name),
        "-".repeat(col_image),
        "-".repeat(col_port),
        "-".repeat(col_status),
        "-".repeat(col_created),
    );

    println!("{}", sep);
    println!(
        "| {:<col_name$} | {:<col_image$} | {:<col_port$} | {:<col_status$} | {:<col_created$} |",
        "name".bold(),
        "image".bold(),
        "port".bold(),
        "status".bold(),
        "created at".bold(),
        col_name = col_name,
        col_image = col_image,
        col_port = col_port,
        col_status = col_status,
        col_created = col_created,
    );
    println!("{}", sep);

    for a in apps {
        let primary = primary_process(a);
        let image = primary.and_then(|p| p.image_id.as_deref()).unwrap_or("-");
        let port = primary
            .and_then(|p| p.host_port)
            .map(|p| p.to_string())
            .unwrap_or_else(|| "-".into());
        let created = a.created_at.as_deref().unwrap_or("-");
        let (status_colored, status_len) = colored_status(&a.status);
        let status_pad = col_status.saturating_sub(status_len);

        println!(
            "| {:<col_name$} | {:<col_image$} | {:<col_port$} | {}{} | {:<col_created$} |",
            a.name,
            image,
            port,
            status_colored,
            " ".repeat(status_pad),
            created,
            col_name = col_name,
            col_image = col_image,
            col_port = col_port,
            col_created = col_created,
        );
    }

    println!("{}", sep);
}

fn primary_process(app: &App) -> Option<&AppProcess> {
    app.processes
        .iter()
        .find(|process| process.name == "web")
        .or_else(|| app.processes.first())
}

// GET /app — exists
pub async fn list() -> Result<(), String> {
    let project = require_project()?;
    let url = format!("{}/project/{}/app", api_base(), project);
    let resp = reqwest::get(url)
        .await
        .map_err(|e| format!("Request failed: {e}"))?;

    if !resp.status().is_success() {
        return Err(format!("Server error: {}", resp.status()));
    }

    let apps: Vec<App> = resp
        .json()
        .await
        .map_err(|e| format!("Failed to parse response: {e}"))?;

    if apps.is_empty() {
        println!("No applications found.");
        return Ok(());
    }

    print_table(&apps);
    Ok(())
}

fn app_url(name: &str, action: &str) -> Result<reqwest::Url, String> {
    let mut url = reqwest::Url::parse(&api_base()).map_err(|e| format!("Invalid API URL: {e}"))?;
    {
        let mut segments = url
            .path_segments_mut()
            .map_err(|_| "API URL cannot be a base".to_string())?;
        if let Some(project) = current_project()? {
            segments.extend(&["project", &project, "app", name, action]);
        } else {
            segments.extend(&["app", name, action]);
        }
    }
    Ok(url)
}

// POST /app/{name}/stop — exists
pub async fn stop(name: &str) -> Result<(), String> {
    let url = app_url(name, "stop")?;
    let pb = spinner(&format!("Stopping {}...", name));
    let client = http_client();
    let resp = client
        .post(url)
        .send()
        .await
        .map_err(|e| format!("Request failed: {e}"))?;
    pb.finish_and_clear();

    match resp.status().as_u16() {
        200 => println!("{} App {} stopped", "✓".green(), name.bold()),
        404 => return Err(format!("App '{}' not found", name)),
        code => return Err(format!("Server error: {}", code)),
    }

    Ok(())
}

// POST /app/{name}/restart — exists
pub async fn restart(name: &str) -> Result<(), String> {
    let url = app_url(name, "restart")?;
    let pb = spinner(&format!("Restarting {}...", name));
    let client = http_client();
    let resp = client
        .post(url)
        .send()
        .await
        .map_err(|e| format!("Request failed: {e}"))?;
    pb.finish_and_clear();

    match resp.status().as_u16() {
        200 => println!("{} App {} restarted", "✓".green(), name.bold()),
        404 => return Err(format!("App '{}' not found", name)),
        code => return Err(format!("Server error: {}", code)),
    }

    Ok(())
}

// DELETE /app/{name}
pub async fn delete(name: &str) -> Result<(), String> {
    let mut url = reqwest::Url::parse(&api_base()).map_err(|e| format!("Invalid API URL: {e}"))?;
    {
        let mut segments = url
            .path_segments_mut()
            .map_err(|_| "API URL cannot be a base".to_string())?;
        if let Some(project) = current_project()? {
            segments.extend(&["project", &project, "app", name]);
        } else {
            segments.extend(&["app", name]);
        }
    }
    let url = url;
    let pb = spinner(&format!("Deleting {}...", name));
    let client = http_client();
    let resp = client
        .delete(url)
        .send()
        .await
        .map_err(|e| format!("Request failed: {e}"))?;
    pb.finish_and_clear();

    match resp.status().as_u16() {
        204 => println!("{} App {} deleted", "✓".green(), name.bold()),
        404 => return Err(format!("App '{}' not found", name)),
        code => return Err(format!("Server error: {}", code)),
    }

    Ok(())
}

// GET /app/{name}
pub async fn info(name: &str) -> Result<(), String> {
    // Status exists: GET /app/{name}/status
    let url = app_url(name, "status")?;
    let resp = reqwest::get(url)
        .await
        .map_err(|e| format!("Request failed: {e}"))?;

    match resp.status().as_u16() {
        200 => {
            let status_text = resp.text().await.unwrap_or_default();
            let (colored, _) = colored_status(&status_text);
            println!("{}: {}", name.bold(), colored);
        }
        404 => return Err(format!("App '{}' not found", name)),
        code => return Err(format!("Server error: {}", code)),
    }

    Ok(())
}

// POST /app/upload — exists
pub async fn upload(source: &str) -> Result<(), String> {
    let path = std::path::Path::new(source);

    if !path.exists() {
        return Err(format!("File not found: {}", source));
    }
    if path.extension().and_then(|e| e.to_str()) != Some("zip") {
        return Err("Source must be a .zip file".to_string());
    }

    upload_archive(path, None, None).await
}

// GET /app/{name}/logs
pub async fn logs(
    name: &str,
    tail: Option<u32>,
    follow: bool,
    process: Option<&str>,
) -> Result<(), String> {
    let mut url = app_url(name, "logs")?;
    if let Some(n) = tail {
        url.query_pairs_mut().append_pair("tail", &n.to_string());
    }
    if follow {
        url.query_pairs_mut().append_pair("follow", "true");
    }
    if let Some(process) = process {
        url.query_pairs_mut().append_pair("process", process);
    }

    let mut resp = reqwest::get(url)
        .await
        .map_err(|e| format!("Request failed: {e}"))?;

    match resp.status().as_u16() {
        200 => {
            if follow {
                use std::io::Write;
                while let Some(chunk) = resp
                    .chunk()
                    .await
                    .map_err(|e| format!("Stream error: {e}"))?
                {
                    print!("{}", String::from_utf8_lossy(&chunk));
                    std::io::stdout().flush().ok();
                }
            } else {
                let text = resp.text().await.unwrap_or_default();
                print!("{}", text);
            }
        }
        404 => return Err(format!("App '{}' not found", name)),
        code => return Err(format!("Server error: {}", code)),
    }

    Ok(())
}

pub async fn deploy_current_dir(port: u16) -> Result<(), String> {
    let root = project_root()?;
    validate_source_project(&root)?;
    let app_name = read_project_name(&root)?;

    let archive = package_project(&root)?;
    let result = upload_archive(&archive, Some(port), Some(&app_name)).await;
    let _ = std::fs::remove_file(&archive);
    result
}

pub async fn update_current_dir(port: u16) -> Result<(), String> {
    let root = project_root()?;
    validate_source_project(&root)?;
    let app_name = read_project_name(&root)?;
    let project = require_project()?;
    let client = http_client();

    let status_resp = client
        .get(format!(
            "{}/project/{}/app/{}/status",
            api_base(),
            project,
            app_name
        ))
        .send()
        .await
        .map_err(|e| format!("Request failed: {e}"))?;
    if status_resp.status().as_u16() == 404 {
        return Err(format!(
            "App '{}' has not been deployed yet. Run `paastech deploy` first.",
            app_name
        ));
    }
    if !status_resp.status().is_success() {
        return Err(format!("Server error: {}", status_resp.status()));
    }

    let archive = package_project(&root)?;
    let file = tokio::fs::File::open(&archive)
        .await
        .map_err(|e| format!("Failed to open archive: {e}"))?;
    let file_size = file
        .metadata()
        .await
        .map_err(|e| format!("Failed to stat archive: {e}"))?
        .len();

    let pb = spinner(&format!("Uploading {} for rolling update...", app_name));
    let part = reqwest::multipart::Part::stream_with_length(file, file_size)
        .file_name("app.zip")
        .mime_str("application/zip")
        .map_err(|e| format!("MIME error: {e}"))?;
    let form = reqwest::multipart::Form::new()
        .part("file", part)
        .text("internal_port", port.to_string());

    let resp = client
        .post(format!(
            "{}/project/{}/app/{}/update",
            api_base(),
            project,
            app_name
        ))
        .multipart(form)
        .send()
        .await
        .map_err(|e| format!("Request failed: {e}"))?;
    let _ = std::fs::remove_file(&archive);
    pb.finish_and_clear();

    if !resp.status().is_success() {
        let status = resp.status();
        let text = resp.text().await.unwrap_or_default();
        return Err(format!("Update failed ({}): {}", status, text));
    }

    wait_until_running(&client, &project, &app_name).await
}

async fn upload_archive(
    path: &Path,
    fallback_port: Option<u16>,
    app_name: Option<&str>,
) -> Result<(), String> {
    let pb = spinner("Packaging and uploading...");

    let filename = path
        .file_name()
        .and_then(|n| n.to_str())
        .unwrap_or("app.zip")
        .to_string();

    let file = tokio::fs::File::open(path)
        .await
        .map_err(|e| format!("Failed to open archive: {e}"))?;
    let file_size = file
        .metadata()
        .await
        .map_err(|e| format!("Failed to stat archive: {e}"))?
        .len();

    let part = reqwest::multipart::Part::stream_with_length(file, file_size)
        .file_name(filename)
        .mime_str("application/zip")
        .map_err(|e| format!("MIME error: {e}"))?;

    let mut form = reqwest::multipart::Form::new().part("file", part);
    if let Some(port) = fallback_port {
        form = form.text("internal_port", port.to_string());
    }
    if let Some(name) = app_name {
        form = form.text("name", name.to_string());
    }

    let client = http_client();
    let project = require_project()?;
    let resp = client
        .post(format!("{}/project/{}/app/upload", api_base(), project))
        .multipart(form)
        .send()
        .await
        .map_err(|e| format!("Request failed: {e}"))?;

    pb.finish_and_clear();

    if !resp.status().is_success() {
        let status = resp.status();
        let text = resp.text().await.unwrap_or_default();
        return Err(format!("Deploy failed ({}): {}", status, text));
    }

    let body: serde_json::Value = resp
        .json()
        .await
        .map_err(|e| format!("Failed to parse response: {e}"))?;

    let name = body["name"]
        .as_str()
        .ok_or("Server response missing 'name' field")?
        .to_string();

    println!(
        "{} Deploy accepted — app name: {}",
        "✓".green(),
        name.bold()
    );

    wait_until_running(&client, &project, &name).await
}

async fn wait_until_running(
    client: &reqwest::Client,
    project: &str,
    name: &str,
) -> Result<(), String> {
    let pb = spinner("Building and deploying...");
    loop {
        tokio::time::sleep(Duration::from_secs(5)).await;

        let status_resp = client
            .get(format!(
                "{}/project/{}/app/{}/status",
                api_base(),
                project,
                name
            ))
            .send()
            .await
            .map_err(|e| format!("Status check failed: {e}"))?;

        let status_text = status_resp.text().await.unwrap_or_default();

        match status_text.as_str() {
            "running" => {
                pb.finish_and_clear();
                println!("{} App {} is running", "✓".green(), name.bold());
                return Ok(());
            }
            "failed" => {
                pb.finish_and_clear();
                return Err(format!("Build or deploy failed for {}", name));
            }
            other => {
                pb.set_message(format!("Status: {other}..."));
            }
        }
    }
}

fn project_root() -> Result<PathBuf, String> {
    let mut dir =
        std::env::current_dir().map_err(|e| format!("Failed to read current directory: {e}"))?;
    loop {
        let candidate = dir.join("paastech.toml");
        if candidate.is_file() {
            return Ok(dir);
        }
        if !dir.pop() {
            return Err(
                "No paastech.toml found. Run `paastech init <project-name>` first.".to_string(),
            );
        }
    }
}

fn validate_source_project(root: &Path) -> Result<(), String> {
    if !root.join("paastech.toml").is_file() {
        return Err("No paastech.toml found in project root".to_string());
    }

    Ok(())
}

fn read_project_name(root: &Path) -> Result<String, String> {
    let manifest_path = root.join("paastech.toml");
    let content = std::fs::read_to_string(&manifest_path)
        .map_err(|e| format!("Failed to read {}: {e}", manifest_path.display()))?;
    let manifest: ProjectManifest = toml::from_str(&content)
        .map_err(|e| format!("Invalid {}: {e}", manifest_path.display()))?;
    Ok(manifest.project.name)
}

fn package_project(root: &Path) -> Result<PathBuf, String> {
    let archive_path =
        std::env::temp_dir().join(format!("paastech-deploy-{}.zip", std::process::id()));
    let file = File::create(&archive_path)
        .map_err(|e| format!("Failed to create {}: {e}", archive_path.display()))?;
    let mut zip = zip::ZipWriter::new(file);
    let options = SimpleFileOptions::default().compression_method(zip::CompressionMethod::Deflated);

    for entry in WalkDir::new(root).follow_links(false) {
        let entry = entry.map_err(|e| format!("Failed to walk project files: {e}"))?;
        let path = entry.path();
        if path == root || should_skip(root, path) {
            continue;
        }

        let relative = path
            .strip_prefix(root)
            .map_err(|e| format!("Failed to build archive path: {e}"))?;
        let archive_name = relative
            .to_string_lossy()
            .replace(std::path::MAIN_SEPARATOR, "/");

        if entry.file_type().is_dir() {
            zip.add_directory(format!("{archive_name}/"), options)
                .map_err(|e| format!("Failed to add directory to archive: {e}"))?;
            continue;
        }

        if !entry.file_type().is_file() {
            continue;
        }

        zip.start_file(archive_name, options)
            .map_err(|e| format!("Failed to add file to archive: {e}"))?;
        let mut source =
            File::open(path).map_err(|e| format!("Failed to open {}: {e}", path.display()))?;
        let mut buffer = Vec::new();
        source
            .read_to_end(&mut buffer)
            .map_err(|e| format!("Failed to read {}: {e}", path.display()))?;
        zip.write_all(&buffer)
            .map_err(|e| format!("Failed to write archive: {e}"))?;
    }

    zip.finish()
        .map_err(|e| format!("Failed to finish archive: {e}"))?;
    Ok(archive_path)
}

fn should_skip(root: &Path, path: &Path) -> bool {
    let Ok(relative) = path.strip_prefix(root) else {
        return true;
    };

    let mut components = relative.components();
    let first = components.next();
    if matches!(
        first,
        Some(Component::Normal(name))
            if matches!(
                name.to_str(),
                Some(".git" | ".hg" | ".svn" | "target" | "node_modules" | ".venv" | "venv" | "__pycache__")
            )
    ) {
        return true;
    }

    path.file_name()
        .and_then(|name| name.to_str())
        .is_some_and(|name| {
            matches!(
                name,
                ".DS_Store" | ".env" | ".env.local" | ".envrc" | "paastech-deploy.zip"
            ) || name.ends_with(".zip")
        })
}

fn process_env_url(name: &str, process: &str) -> Result<reqwest::Url, String> {
    let mut url = reqwest::Url::parse(&api_base()).map_err(|e| format!("Invalid API URL: {e}"))?;
    {
        let mut segments = url
            .path_segments_mut()
            .map_err(|_| "API URL cannot be a base".to_string())?;
        if let Some(project) = current_project()? {
            segments.extend(&["project", &project, "app", name, "process", process, "env"]);
        } else {
            segments.extend(&["app", name, "process", process, "env"]);
        }
    }
    Ok(url)
}

// POST /app/{name}/process/{process}/env
pub async fn env_set(name: &str, process: &str, pair: &str) -> Result<(), String> {
    let (key, value) = pair
        .split_once('=')
        .ok_or_else(|| "Invalid format: expected KEY=VALUE".to_string())?;

    let url = process_env_url(name, process)?;
    let pb = spinner(&format!("Setting env for {}/{}...", name, process));
    let client = http_client();
    let body = serde_json::json!({ "key": key, "value": value });

    let resp = client
        .post(url)
        .json(&body)
        .send()
        .await
        .map_err(|e| format!("Request failed: {e}"))?;
    pb.finish_and_clear();

    match resp.status().as_u16() {
        200 | 201 | 204 => println!("{} {}={}", "✓".green(), key.bold(), value),
        404 => return Err(format!("App '{}' or process '{}' not found", name, process)),
        code => {
            let text = resp.text().await.unwrap_or_default();
            return Err(format!("Server error ({}): {}", code, text));
        }
    }

    Ok(())
}

// GET /app/{name}/process/{process}/env
pub async fn env_list(name: &str, process: &str) -> Result<(), String> {
    let url = process_env_url(name, process)?;
    let resp = reqwest::get(url)
        .await
        .map_err(|e| format!("Request failed: {e}"))?;

    match resp.status().as_u16() {
        200 => {
            let vars: serde_json::Map<String, Value> = resp
                .json()
                .await
                .map_err(|e| format!("Failed to parse response: {e}"))?;
            if vars.is_empty() {
                println!("No environment variables set.");
            } else {
                for (key, val) in &vars {
                    let v = val
                        .as_str()
                        .map(|s| s.to_string())
                        .unwrap_or_else(|| val.to_string());
                    println!("{}={}", key.bold(), v);
                }
            }
        }
        404 => return Err(format!("App '{}' or process '{}' not found", name, process)),
        code => return Err(format!("Server error: {}", code)),
    }

    Ok(())
}
