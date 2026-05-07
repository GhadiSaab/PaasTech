mod engine;
mod registry;
mod scheduler;

use crate::engine::{build_image, extract_zip, save_multipart_file};
use crate::scheduler::Scheduler;
use actix_multipart::Multipart;
use actix_web::{App, Error, HttpResponse, HttpServer, Responder, post, web};
use serde::Deserialize;
use serde_json::json;
use sqlx::PgPool;
use std::net::TcpListener;
use tokio::fs;

const PORT_RANGE_START: u16 = 30000;
const PORT_RANGE_END: u16 = 40000;

fn find_available_port() -> Option<u16> {
    for port in PORT_RANGE_START..=PORT_RANGE_END {
        if let Ok(listener) = TcpListener::bind(("0.0.0.0", port)) {
            drop(listener);
            return Some(port);
        }
    }
    None
}

#[derive(Deserialize)]
struct UploadQuery {
    #[serde(default)]
    port: Option<u16>,
}

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
            .app_data(web::QueryConfig::default().error_handler(|err, _| {
                actix_web::error::ErrorBadRequest(
                    serde_json::to_string(&serde_json::json!({"error": err.to_string()}))
                        .unwrap_or_else(|_| "{\"error\":\"unknown\"}".to_string()),
                )
            }))
            .service(upload_app)
    })
    .bind((config.host, config.port))?
    .run()
    .await
}

// routes

#[post("/app/upload")]
async fn upload_app(
    payload: Multipart,
    query: web::Query<UploadQuery>,
    pool: web::Data<PgPool>,
) -> Result<impl Responder, Error> {
    let container_port = match query.port {
        Some(p) => p,
        None => {
            return Ok(HttpResponse::BadRequest().json(json!({"error": "missing port parameter"})));
        }
    };

    let zip_filepath = match save_multipart_file(payload).await {
        Ok(Some(path)) => path,
        Ok(None) => {
            return Ok(HttpResponse::BadRequest().json(json!({"error": "provide file in payload"})));
        }
        Err(e) => {
            return Ok(HttpResponse::BadRequest().json(json!({"error": e.to_string()})));
        }
    };

    let extracted_folder = match extract_zip(zip_filepath).await {
        Ok(path) => path,
        Err(e) => {
            return Ok(
                HttpResponse::BadRequest().json(json!({"error": format!("extract failed: {}", e)}))
            );
        }
    };

    let image_name = match build_image(extracted_folder).await {
        Ok(name) => name,
        Err(e) => return Ok(HttpResponse::BadRequest().json(json!({"error": e}))),
    };

    let host_port = match find_available_port() {
        Some(p) => p,
        None => {
            return Ok(HttpResponse::ServiceUnavailable()
                .json(json!({"error": "no available host port found in range 30000-40000"})));
        }
    };

    let scheduler = Scheduler::new();
    scheduler
        .deploy(
            pool.get_ref(),
            &image_name,
            &image_name,
            container_port as i32,
            host_port as i32,
        )
        .await;

    Ok(HttpResponse::Ok()
        .json(json!({"status": "success", "image": image_name, "port": host_port})))
}
