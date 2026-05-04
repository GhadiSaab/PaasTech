use actix_multipart::Multipart;
use actix_web::{web, Error};
use futures_util::TryStreamExt;
use std::path::PathBuf;
use tokio::fs::File;
use tokio::io::AsyncWriteExt;

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

pub async fn extract_zip(source: &PathBuf) -> Result<(), String> {
    let source_path = source.clone();
    let dest_path = PathBuf::from(format!("{}-extract", source.display()));

    web::block(move || {
        let sync_file = std::fs::File::open(&source_path)
            .map_err(|e| format!("Failed to open file for extraction: {}", e))?;

        zip_extract::extract(sync_file, &dest_path, true)
            .map_err(|e| format!("Zip extraction error: {}", e))
    })
    .await
    .map_err(|e| format!("Thread pool error: {}", e))?
}
