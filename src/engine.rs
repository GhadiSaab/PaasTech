use actix_multipart::Multipart;
use actix_web::{Error, web};
use futures_util::TryStreamExt;
use std::collections::HashMap;
use std::path::PathBuf;
use tokio::fs::File;
use tokio::io::AsyncWriteExt;
use tokio::process::Command as TokioCommand;
use uuid::Uuid;

pub struct MultipartData {
    pub file_path: Option<PathBuf>,
    pub fields: HashMap<String, String>,
}

pub async fn save_multipart_file(mut payload: Multipart) -> Result<MultipartData, Error> {
    let mut file_path = None;
    let mut fields = HashMap::new();

    while let Ok(Some(mut field)) = payload.try_next().await {
        let content_disposition = match field.content_disposition() {
            Some(cd) => cd,
            None => continue,
        };

        let field_name = content_disposition.get_name().unwrap_or("").to_string();

        if let Some(filename) = content_disposition.get_filename() {
            let filename = filename.to_string();
            let filepath = PathBuf::from(format!("/tmp/uploads/{}", filename));
            let mut file = File::create(&filepath).await?;
            while let Ok(Some(chunk)) = field.try_next().await {
                file.write_all(&chunk).await?;
            }
            file.flush().await?;
            file_path = Some(filepath);
        } else {
            let mut value = String::new();
            while let Ok(Some(chunk)) = field.try_next().await {
                value.push_str(&String::from_utf8_lossy(&chunk));
            }
            fields.insert(field_name, value);
        }
    }

    Ok(MultipartData { file_path, fields })
}

pub async fn extract_zip(source: PathBuf) -> Result<PathBuf, String> {
    let mut dest_path = source.clone();
    if let Some(stem) = source.file_stem() {
        dest_path.set_file_name(format!("{}-extract", stem.to_string_lossy()));
    } else {
        dest_path.push("-extract");
    }
    let dest_for_closure = dest_path.clone();

    web::block(move || {
        let sync_file = std::fs::File::open(&source)
            .map_err(|e| format!("Failed to open file for extraction: {}", e))?;

        zip_extract::extract(sync_file, &dest_for_closure, true)
            .map_err(|e| format!("Zip extraction error: {}", e))
    })
    .await
    .map_err(|e| format!("Thread pool error: {}", e))?
    .map(|_| dest_path)
}

pub async fn build_image(from: String, docker_host: &str) -> Result<String, String> {
    let image_name = format!("paastech-{}", Uuid::new_v4());

    let builder = std::env::var("BUILDER").map_err(|_| "BUILDER env var is not set".to_string())?;

    let mut cmd = TokioCommand::new("pack");
    cmd.args(["build", &image_name, "--path", &from, "--builder", &builder]);

    if !docker_host.is_empty() {
        cmd.args(["--docker-host", docker_host]);
    }

    let output = cmd
        .output()
        .await
        .map_err(|e| format!("Failed to run pack build: {}", e))?;

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        let stdout = String::from_utf8_lossy(&output.stdout);
        return Err(format!(
            "Build failed with exit code {}:\n--- stderr ---\n{}\n--- stdout ---\n{}",
            output.status, stderr, stdout
        ));
    }

    Ok(image_name)
}
