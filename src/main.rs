mod docker;
mod engine;
mod extractor;
mod garage;
mod handlers;
mod models;
mod registry;
mod scheduler;
pub mod status;

use actix_web::{App, HttpServer, web};
use reqwest::Client;
use sqlx::PgPool;
use tokio::fs;
use utoipa::OpenApi;
use utoipa_swagger_ui::SwaggerUi;

use crate::models::{CreateProjectPayload, CreateResourcePayload, Resource, UpdateResourcePayload};
use crate::scheduler::Scheduler;

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

#[derive(OpenApi)]
#[openapi(
    paths(
        handlers::services::get_service_versions,
        handlers::projects::create,
        handlers::projects::list,
        handlers::projects::get,
        handlers::projects::delete,
        handlers::projects::get_env,
        handlers::projects::update_env,
        handlers::apps::upload,
        handlers::apps::list,
        handlers::apps::update,
        handlers::apps::stop,
        handlers::apps::restart,
        handlers::apps::status,
        handlers::apps::delete,
        handlers::apps::logs,
        handlers::apps::get_env,
        handlers::apps::set_env,
        handlers::apps::update_env,
        handlers::resources::create,
        handlers::resources::list,
        handlers::resources::get,
        handlers::resources::update,
        handlers::resources::delete,
        handlers::resources::start,
        handlers::resources::stop,
        handlers::resources::logs,
        handlers::resources::get_env,
        handlers::resources::update_env,
    ),
    components(schemas(
        registry::App,
        registry::Project,
        Resource,
        CreateProjectPayload,
        CreateResourcePayload,
        UpdateResourcePayload,
    )),
    tags(
        (name = "projects", description = "Project management"),
        (name = "services", description = "Service version management"),
        (name = "apps", description = "Application management"),
        (name = "resources", description = "Resource management"),
    ),
    info(title = "PaaSTech API", version = "0.1.0", description = "PaaSTech Platform as a Service API")
)]
struct ApiDoc;

#[actix_web::main]
async fn main() -> std::io::Result<()> {
    dotenvy::dotenv().ok();

    let config: Config = init().await;

    println!("Loading PSQL...");

    let pool = PgPool::connect(&config.database_url)
        .await
        .expect("Failed to connect to PostgreSQL");

    sqlx::raw_sql(include_str!("../database/init.sql"))
        .execute(&pool)
        .await
        .expect("Failed to apply database schema");

    let http_client = Client::new();
    let scheduler = Scheduler::new();
    let watcher_pool = pool.clone();
    let watcher_scheduler = scheduler.clone();
    tokio::spawn(async move {
        watcher_scheduler.watch(&watcher_pool).await;
    });

    println!("Running on http://{}:{}", config.host, config.port);
    println!(
        "Swagger UI: http://{}:{}/swagger-ui/",
        config.host, config.port
    );
    HttpServer::new(move || {
        App::new()
            .app_data(web::Data::new(pool.clone()))
            .app_data(web::Data::new(http_client.clone()))
            .app_data(web::Data::new(scheduler.clone()))
            .service(
                SwaggerUi::new("/swagger-ui/{_:.*}")
                    .url("/api-docs/openapi.json", ApiDoc::openapi()),
            )
            .configure(handlers::configure)
    })
    .bind((config.host, config.port))?
    .run()
    .await
}

#[cfg(test)]
mod tests;
