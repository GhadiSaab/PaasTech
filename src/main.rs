mod scheduler;

use actix_multipart::Multipart;
use actix_web::{get, post, web, App, Error, HttpResponse, HttpServer, Responder};
use futures_util::TryStreamExt;
use serde::Deserialize;
use std::path::Path;
use tokio::fs;
use tokio::fs::File;
use tokio::io::AsyncWriteExt;

use scheduler::Scheduler;

struct Config {
    host: String,
    port: u16,
}

async fn init() -> Config {
    fs::create_dir_all("uploads")
        .await
        .expect("Folder creation failed.");

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
    let scheduler = web::Data::new(Scheduler::new());

    println!("Running on http://{}:{}", config.host, config.port);
    HttpServer::new(move ||
        App::new()
            .app_data(scheduler.clone())
            .service(upload_app)
            .service(list_apps)
            .service(deploy_app)
            .service(stop_app)
            .service(restart_app)
            .service(status_app)
    )
        .bind((config.host, config.port))?
        .run()
        .await
}

// routes

#[post("/app/upload")]
async fn upload_app(mut payload: Multipart) -> Result<impl Responder, Error> {
    while let Ok(Some(mut field)) = payload.try_next().await {
        let filename: &str = match field.content_disposition() {
            Some(content) => match content.get_filename() {
                Some(name) => name,
                None => continue,
            },
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

#[get("/app/list")]
async fn list_apps(scheduler: web::Data<Scheduler>) -> impl Responder {
    let apps = scheduler.list().await;
    HttpResponse::Ok().json(apps)
}

#[derive(Deserialize)]
struct DeployBody {
    image: String,
}

#[post("/app/deploy")]
async fn deploy_app(
    scheduler: web::Data<Scheduler>,
    body: web::Json<DeployBody>,
) -> impl Responder {
    let container_id = scheduler.deploy(&body.image).await;
    HttpResponse::Ok().body(container_id)
}

#[post("/app/{container_id}/stop")]
async fn stop_app(
    scheduler: web::Data<Scheduler>,
    path: web::Path<String>,
) -> impl Responder {
    let container_id = path.into_inner();
    scheduler.stop(&container_id).await;
    HttpResponse::Ok().finish()
}

#[post("/app/{container_id}/restart")]
async fn restart_app(
    scheduler: web::Data<Scheduler>,
    path: web::Path<String>,
) -> impl Responder {
    let container_id = path.into_inner();
    scheduler.restart(&container_id).await;
    HttpResponse::Ok().finish()
}

#[get("/app/{container_id}/status")]
async fn status_app(
    scheduler: web::Data<Scheduler>,
    path: web::Path<String>,
) -> impl Responder {
    let container_id = path.into_inner();
    let status = scheduler.status(&container_id).await;
    HttpResponse::Ok().body(status)
}

#[cfg(test)]
mod tests;
