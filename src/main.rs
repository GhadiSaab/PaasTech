mod engine;
mod registry;
mod scheduler;

use crate::engine::{extract_zip, launch_code, save_multipart_file};
use actix_multipart::Multipart;
use actix_web::{App, Error, HttpResponse, HttpServer, Responder, post, web};
use sqlx::PgPool;
use tokio::fs;

struct Config {
    host: String,
    port: u16,
    database_url: String,
}

async fn init() -> Config {
    fs::create_dir_all("/tmp/uploads")
        .await
        .expect("Folder creation failed.");

    dotenvy::dotenv().ok();

    let host = std::env::var("HOST").unwrap_or_else(|_| "127.0.0.1".to_string());
    let port = std::env::var("PORT")
        .unwrap_or_else(|_| "8080".to_string())
        .parse()
        .expect("PORT must be a valid number");
    let database_url = std::env::var("DATABASE_URL")
        .unwrap_or_else(|_| "postgresql://paastech:paastech@localhost:5433/paastech".to_string());

    Config {
        host,
        port,
        database_url,
    }
}

#[actix_web::main]
async fn main() -> std::io::Result<()> {
    dotenvy::dotenv().ok();

    let config: Config = init().await;

    println!("Loading PSQL...");

    let pool = PgPool::connect(&config.database_url)
        .await
        .expect("Failed to connect to PostgreSQL");

    println!("Running on http://{}:{}", config.host, config.port);
    HttpServer::new(move || {
        App::new()
            .app_data(web::Data::new(pool.clone()))
            .service(upload_app)
    })
    .bind((config.host, config.port))?
    .run()
    .await
}

// routes

#[post("/app/upload")]
async fn upload_app(payload: Multipart) -> Result<impl Responder, Error> {
    let zip_filepath = match save_multipart_file(payload).await? {
        Some(path) => path,
        None => return Ok(HttpResponse::BadRequest().body("provide file in payload")),
    };

    println!("Zip file uploaded to {:?}", zip_filepath);

    let extracted_folder = extract_zip(zip_filepath).await.expect("extract failed");

    println!("Extraction worked");

    launch_code(extracted_folder).await;

    Ok(HttpResponse::Ok().body("worked\n"))
}
