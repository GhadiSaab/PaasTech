use actix_multipart::Multipart;
use actix_web::{Error, web};
use futures_util::TryStreamExt;
use std::path::PathBuf;
use tokio::fs::File;
use tokio::io::AsyncWriteExt;
use tokio::process::Command as TokioCommand;
use uuid::Uuid;

pub async fn save_multipart_file(mut payload: Multipart) -> Result<Option<PathBuf>, Error> {
    while let Ok(Some(mut field)) = payload.try_next().await {
        let filename = match field.content_disposition() {
            Some(content) => match content.get_filename() {
                Some(name) => name.to_string(),
                None => continue,
            },
            None => continue,
        };

        // TODO: check if it's a real zip file

        let filepath = PathBuf::from(format!("/tmp/uploads/{}", filename));

        let mut file = File::create(&filepath).await?;

        while let Ok(Some(chunk)) = field.try_next().await {
            file.write_all(&chunk).await?;
        }
        file.flush().await?;

        return Ok(Some(filepath));
    }
    Ok(None)
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

pub async fn build_image(from: PathBuf) -> Result<String, String> {
    let image_name = format!("paastech-{}", Uuid::new_v4());

    let output = TokioCommand::new("pack")
        .args([
            "build",
            &image_name,
            "--path",
            &from.to_string_lossy(),
            "--docker-host",
            "unix:///run/user/1000/docker.sock",
        ])
        .output()
        .await
        .map_err(|e| format!("Failed to run pack build: {}", e))?;

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        return Err(format!("Build failed:\n{}", stderr));
    }

    Ok(image_name)
}

pub async fn run_container(image_name: &str, port: u16) -> Result<(), String> {
    let output = TokioCommand::new("docker")
        .args([
            "run",
            "-d",
            "-p",
            &format!("8081:{}", port),
            "--env",
            &format!("PORT={}", port),
            "--name",
            image_name,
            image_name,
        ])
        .output()
        .await
        .map_err(|e| format!("Failed to run container: {}", e))?;

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        return Err(format!("Container failed to start:\n{}", stderr));
    }

    Ok(())
}
