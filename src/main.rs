mod engine;
mod registry;
mod scheduler;

use crate::engine::{extract_zip, launch_code, save_multipart_file};
use actix_multipart::Multipart;
use actix_web::{App, Error, HttpResponse, HttpServer, Responder, get, post, web};
use serde::Deserialize;
use sqlx::PgPool;
use tokio::fs;

use registry::Registry;
use scheduler::Scheduler;

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

    let scheduler = web::Data::new(Scheduler::new());

    println!("Running on http://{}:{}", config.host, config.port);
    HttpServer::new(move || {
        App::new()
            .app_data(web::Data::new(pool.clone()))
            .app_data(scheduler.clone())
            .service(upload_app)
            .service(list_apps)
            .service(deploy_app)
            .service(stop_app)
            .service(restart_app)
            .service(status_app)
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

#[get("/app")]
async fn list_apps(pool: web::Data<PgPool>) -> impl Responder {
    match Registry::list(&pool).await {
        Ok(apps) => HttpResponse::Ok().json(apps),
        Err(e) => {
            eprintln!("registry: list_apps failed: {e}");
            HttpResponse::InternalServerError().finish()
        }
    }
}

#[derive(Deserialize)]
struct DeployBody {
    name: String,
    image: String,
    port: i32,
}

#[post("/app/deploy")]
async fn deploy_app(
    scheduler: web::Data<Scheduler>,
    pool: web::Data<PgPool>,
    body: web::Json<DeployBody>,
) -> impl Responder {
    let container_id = match scheduler.deploy(&body.name, &body.image).await {
        Some(id) => id,
        None => return HttpResponse::InternalServerError().finish(),
    };
    match Registry::save(
        &pool,
        &body.name,
        &body.image,
        &container_id,
        body.port,
        "running",
        None,
    )
    .await
    {
        Ok(_) => HttpResponse::Ok().finish(),
        Err(e) => {
            eprintln!("registry: failed to save app {}: {e}", body.name);
            HttpResponse::InternalServerError().finish()
        }
    }
}

#[post("/app/{app_name}/stop")]
async fn stop_app(
    scheduler: web::Data<Scheduler>,
    pool: web::Data<PgPool>,
    path: web::Path<String>,
) -> impl Responder {
    let app_name = path.into_inner();
    match Registry::get(&pool, &app_name).await {
        Ok(Some(_)) => {}
        Ok(None) => return HttpResponse::NotFound().finish(),
        Err(e) => {
            eprintln!("registry: get failed for {app_name}: {e}");
            return HttpResponse::InternalServerError().finish();
        }
    }
    scheduler.stop(&app_name).await;
    if let Err(e) = Registry::update_status(&pool, &app_name, "stopped").await {
        eprintln!("registry: failed to update status for {app_name}: {e}");
    }
    HttpResponse::Ok().finish()
}

#[post("/app/{app_name}/restart")]
async fn restart_app(
    scheduler: web::Data<Scheduler>,
    pool: web::Data<PgPool>,
    path: web::Path<String>,
) -> impl Responder {
    let app_name = path.into_inner();
    match Registry::get(&pool, &app_name).await {
        Ok(Some(_)) => {}
        Ok(None) => return HttpResponse::NotFound().finish(),
        Err(e) => {
            eprintln!("registry: get failed for {app_name}: {e}");
            return HttpResponse::InternalServerError().finish();
        }
    }
    scheduler.restart(&app_name).await;
    if let Err(e) = Registry::update_status(&pool, &app_name, "running").await {
        eprintln!("registry: failed to update status for {app_name}: {e}");
    }
    HttpResponse::Ok().finish()
}

#[get("/app/{app_name}/status")]
async fn status_app(
    scheduler: web::Data<Scheduler>,
    pool: web::Data<PgPool>,
    path: web::Path<String>,
) -> impl Responder {
    let app_name = path.into_inner();
    match Registry::get(&pool, &app_name).await {
        Ok(Some(_)) => {}
        Ok(None) => return HttpResponse::NotFound().finish(),
        Err(e) => {
            eprintln!("registry: get failed for {app_name}: {e}");
            return HttpResponse::InternalServerError().finish();
        }
    }
    let status = scheduler.inspect(&app_name).await;
    HttpResponse::Ok().body(status)
}

#[cfg(test)]
mod tests;
