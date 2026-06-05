use super::utils::{colored_status, spinner};
use crate::api_base;
use colored::Colorize;
use serde::Deserialize;
use serde_json::Value;
use std::time::Duration;

#[derive(Deserialize)]
struct App {
    name: String,
    image_id: Option<String>,
    port: Option<i32>,
    status: Option<String>,
    #[allow(dead_code)]
    env: Option<Value>,
    created_at: Option<String>,
}

fn print_table(apps: &[App]) {
    let col_name = apps.iter().map(|a| a.name.len()).max().unwrap_or(4).max(4);
    let col_image = apps
        .iter()
        .map(|a| a.image_id.as_deref().unwrap_or("-").len())
        .max()
        .unwrap_or(5)
        .max(5);
    let col_port = apps
        .iter()
        .map(|a| a.port.map(|p| p.to_string().len()).unwrap_or(1))
        .max()
        .unwrap_or(4)
        .max(4);
    let col_status = apps
        .iter()
        .map(|a| a.status.as_deref().unwrap_or("unknown").len())
        .max()
        .unwrap_or(6)
        .max(6);
    let col_created = 26_usize.max(10);

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
        let image = a.image_id.as_deref().unwrap_or("-");
        let port = a.port.map(|p| p.to_string()).unwrap_or_else(|| "-".into());
        let created = a.created_at.as_deref().unwrap_or("-");
        let raw_status = a.status.as_deref().unwrap_or("unknown");
        let (status_colored, status_len) = colored_status(raw_status);
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

// POST /app/deploy — exists
pub async fn deploy(name: &str, image: &str, port: u16) -> Result<(), String> {
    let pb = spinner(&format!("Deploying {} ({})", name, image));

    let client = reqwest::Client::new();
    let body = serde_json::json!({
        "name": name,
        "image": image,
        "port": port
    });

    let resp = client
        .post(format!("{}/app/deploy", api_base()))
        .json(&body)
        .send()
        .await
        .map_err(|e| format!("Request failed: {e}"))?;

    pb.finish_and_clear();

    if resp.status().is_success() {
        println!("{} App {} deployed", "✓".green(), name.bold());
    } else {
        let status = resp.status();
        let text = resp.text().await.unwrap_or_default();
        return Err(format!("Deploy failed ({}): {}", status, text));
    }

    Ok(())
}

// GET /app — exists
pub async fn list() -> Result<(), String> {
    let resp = reqwest::get(format!("{}/app", api_base()))
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
    url.path_segments_mut()
        .map_err(|_| "API URL cannot be a base".to_string())?
        .extend(&["app", name, action]);
    Ok(url)
}

// POST /app/{name}/stop — exists
pub async fn stop(name: &str) -> Result<(), String> {
    let url = app_url(name, "stop")?;
    let pb = spinner(&format!("Stopping {}...", name));
    let client = reqwest::Client::new();
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
    let client = reqwest::Client::new();
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
    url.path_segments_mut()
        .map_err(|_| "API URL cannot be a base".to_string())?
        .extend(&["app", name]);
    let url = url;
    let pb = spinner(&format!("Deleting {}...", name));
    let client = reqwest::Client::new();
    let resp = client
        .delete(url)
        .send()
        .await
        .map_err(|e| format!("Request failed: {e}"))?;
    pb.finish_and_clear();

    match resp.status().as_u16() {
        204 => println!("App {} successfully deleted", name.bold()),
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

    let pb = spinner("Uploading...");

    let file_bytes = tokio::fs::read(path)
        .await
        .map_err(|e| format!("Failed to read file: {e}"))?;

    let filename = path
        .file_name()
        .and_then(|n| n.to_str())
        .unwrap_or("app.zip")
        .to_string();

    let part = reqwest::multipart::Part::bytes(file_bytes)
        .file_name(filename)
        .mime_str("application/zip")
        .map_err(|e| format!("MIME error: {e}"))?;

    let form = reqwest::multipart::Form::new().part("file", part);

    let client = reqwest::Client::new();
    let resp = client
        .post(format!("{}/app/upload", api_base()))
        .multipart(form)
        .send()
        .await
        .map_err(|e| format!("Request failed: {e}"))?;

    pb.finish_and_clear();

    if !resp.status().is_success() {
        let status = resp.status();
        let text = resp.text().await.unwrap_or_default();
        return Err(format!("Upload failed ({}): {}", status, text));
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
        "{} Upload accepted — app name: {}",
        "✓".green(),
        name.bold()
    );

    let pb = spinner("Building and deploying...");
    loop {
        tokio::time::sleep(Duration::from_secs(5)).await;

        let status_resp = client
            .get(format!("{}/app/{}/status", api_base(), name))
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

// GET /app/{name}/logs
pub async fn logs(name: &str, tail: Option<u32>) -> Result<(), String> {
    let mut url = app_url(name, "logs")?;
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
        404 => return Err(format!("App '{}' not found", name)),
        code => return Err(format!("Server error: {}", code)),
    }

    Ok(())
}

// POST /app/{name}/env
pub async fn env_set(name: &str, pair: &str) -> Result<(), String> {
    let (key, value) = pair
        .split_once('=')
        .ok_or_else(|| "Invalid format: expected KEY=VALUE".to_string())?;

    let url = app_url(name, "env")?;
    let pb = spinner(&format!("Setting env for {}...", name));
    let client = reqwest::Client::new();
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
        404 => return Err(format!("App '{}' not found", name)),
        code => {
            let text = resp.text().await.unwrap_or_default();
            return Err(format!("Server error ({}): {}", code, text));
        }
    }

    Ok(())
}

// GET /app/{name}/env
pub async fn env_list(name: &str) -> Result<(), String> {
    let url = app_url(name, "env")?;
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
        404 => return Err(format!("App '{}' not found", name)),
        code => return Err(format!("Server error: {}", code)),
    }

    Ok(())
}
