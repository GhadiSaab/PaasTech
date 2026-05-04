use actix_multipart::Multipart;
use actix_web::{Error, web};
use futures_util::TryStreamExt;
use std::path::PathBuf;
use std::process::Command;
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

pub async fn launch_code(from: PathBuf) {
    println!("trying to launch code from {:?}", from);

    let app_py = from.join("app.py");

    if !app_py.exists() {
        println!("no such {:?}.....", app_py);
        return;
    }

    println!("> app found");

    match Command::new("python3").arg(app_py).status() {
        Ok(code) => {
            println!("app did run, code is {}", code)
        }
        Err(e) => {
            eprintln!("app couldn't run........ {}", e)
        }
    }
}
