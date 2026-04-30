use actix_multipart::Multipart;
use actix_web::{post, App, Error, HttpResponse, HttpServer, Responder};
use futures_util::TryStreamExt;
use std::path::Path;
use tokio::fs;
use tokio::fs::File;
use tokio::io::AsyncWriteExt;

struct Config {
    host: String,
    port: u16,
}

async fn init() -> Config {
    fs::create_dir_all("uploads").await.expect("Folder creation failed.");

    dotenvy::dotenv().ok();

    let host = std::env::var("HOST").unwrap_or_else(|_| "127.0.0.1".to_string());
    let port = std::env::var("PORT")
        .unwrap_or_else(|_| "8080".to_string())
        .parse()
        .expect("PORT must be a valid number");

    Config { host, port }
}

#[actix_web::main]
async fn main() -> std::io::Result<()> {
    let config: Config = init().await;

    println!("Running on http://{}:{}", config.host, config.port);
    HttpServer::new(||
        App::new()
            .service(upload_app)
    )
        .bind((config.host, config.port))?
        .run()
        .await
}

// routes

#[post("/app/upload")]
async fn upload_app(mut payload: Multipart) -> Result<impl Responder, Error> {
    while let Ok(Some(mut field)) = payload.try_next().await {
        let filename: &str;

        match field.content_disposition() {
            Some(content) => {
                filename = match content.get_filename() {
                    Some(name) => name,
                    None => continue,
                };
            }
            None => continue,
        };

        // TODO check if it's a real zip file

        let filepath = format!("./uploads/{}", filename);
        let path = Path::new(&filepath);

        let mut f = File::create(path).await?;
        while let Ok(Some(chunk)) = field.try_next().await {
            f.write_all(&chunk).await?;
        }
    }

    Ok(HttpResponse::Ok().body("Zip file uploaded successfully!"))
}
