mod docker;
mod engine;
mod models;
mod registry;
mod scheduler;

use actix_multipart::Multipart;
use actix_web::{
    App, Error, HttpResponse, HttpServer, Responder, delete, error, get, patch, post, put, web,
};
use reqwest::Client;
use serde::Deserialize;
use serde_json::json;
use sqlx::PgPool;
use std::collections::HashMap;
use tokio::fs;
use utoipa::OpenApi;
use utoipa_swagger_ui::SwaggerUi;
use uuid::Uuid;

use crate::docker::{
    container_image_for_service, default_env_vars_for_service, docker_image_for_service,
    fetch_service_versions, is_valid_service, prepare_config_for_service, service_port_for_service,
    valid_services, validate_docker_tag,
};
use crate::engine::{MultipartData, build_image, extract_zip, save_multipart_file};
use crate::models::{CreateResourcePayload, Resource, UpdateResourcePayload};
use crate::registry::Registry;
use crate::scheduler::{Scheduler, find_free_port};

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
        get_service_versions,
        upload_app,
        list_apps,
        deploy_app,
        stop_app,
        restart_app,
        status_app,
        create_resource,
        get_resources,
        get_resource,
        update_resource,
        delete_resource,
        start_resource,
        stop_resource,
        get_resource_env,
        update_resource_env,
    ),
    components(schemas(registry::App, DeployBody, Resource, CreateResourcePayload, UpdateResourcePayload)),
    tags(
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

    let http_client = Client::new();
    let scheduler = Scheduler::new();

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
            .service(get_service_versions)
            .service(upload_app)
            .service(list_apps)
            .service(deploy_app)
            .service(stop_app)
            .service(restart_app)
            .service(status_app)
            .service(create_resource)
            .service(get_resources)
            .service(get_resource)
            .service(update_resource)
            .service(delete_resource)
            .service(start_resource)
            .service(stop_resource)
            .service(get_resource_env)
            .service(update_resource_env)
    })
    .bind((config.host, config.port))?
    .run()
    .await
}

async fn fetch_resource_env_vars(
    pool: &PgPool,
    service_id: uuid::Uuid,
) -> Result<Vec<String>, Error> {
    let rows = sqlx::query!(
        "SELECT key, value FROM service_env_vars WHERE service_id = $1 ORDER BY key",
        service_id
    )
    .fetch_all(pool)
    .await
    .map_err(error::ErrorInternalServerError)?;

    Ok(rows
        .into_iter()
        .map(|r| format!("{}={}", r.key, r.value))
        .collect())
}

// routes

#[utoipa::path(
    get,
    path = "/service/{name}/versions",
    params(("name" = String, Path, description = "Service name (postgres, redis, s3)")),
    responses(
        (status = 200, description = "Available versions", body = Vec<String>),
        (status = 400, description = "Invalid service name"),
        (status = 500, description = "Internal server error"),
    ),
    tag = "services"
)]
#[get("/service/{name}/versions")]
async fn get_service_versions(
    client: web::Data<Client>,
    name: web::Path<String>,
) -> Result<impl Responder, Error> {
    if !is_valid_service(&name) {
        return Err(error::ErrorBadRequest(format!(
            "Invalid service name '{}'. Must be one of: {}",
            name,
            valid_services().join(", ")
        )));
    }

    let versions = fetch_service_versions(&client, &name).await?;
    Ok(HttpResponse::Ok().json(versions))
}

#[utoipa::path(
    post,
    path = "/app/upload",
    request_body(content_type = "multipart/form-data", description = "Zip archive of the application. Form fields: file (zip), name (app name), internal_port (container port to expose)"),
    responses(
        (status = 200, description = "File uploaded successfully"),
        (status = 400, description = "Missing file in payload"),
        (status = 500, description = "Internal server error"),
    ),
    tag = "apps"
)]
#[post("/app/upload")]
async fn upload_app(
    pool: web::Data<PgPool>,
    scheduler: web::Data<Scheduler>,
    payload: Multipart,
) -> Result<impl Responder, Error> {
    let MultipartData { file_path, fields } = save_multipart_file(payload).await?;

    let zip_filepath = match file_path {
        Some(path) => path,
        None => return Ok(HttpResponse::BadRequest().body("provide file in payload")),
    };

    let internal_port = fields
        .get("internal_port")
        .and_then(|p| p.trim().parse::<u16>().ok())
        .ok_or_else(|| {
            error::ErrorBadRequest(
                "internal_port is required: specify the port your web app listens on",
            )
        })?;

    let name = format!("paastech-{}", Uuid::new_v4());

    println!("Zip file uploaded to {:?}", zip_filepath);

    let extracted_folder = extract_zip(zip_filepath)
        .await
        .map_err(error::ErrorInternalServerError)?;

    let image_name = match build_image(
        extracted_folder.to_string_lossy().to_string(),
        scheduler.docker_host(),
    )
    .await
    {
        Ok(name) => name,
        Err(e) => return Ok(HttpResponse::BadRequest().json(json!({"error": e}))),
    };

    if let Err(e) = scheduler
        .deploy(pool.get_ref(), &name, &image_name, internal_port)
        .await
    {
        return Ok(HttpResponse::InternalServerError().json(json!({"error": e})));
    }

    Ok(HttpResponse::Ok().json(json!({"status": "success", "name": name})))
}

#[utoipa::path(
    get,
    path = "/app",
    responses(
        (status = 200, description = "List of deployed applications", body = Vec<registry::App>),
        (status = 500, description = "Internal server error"),
    ),
    tag = "apps"
)]
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

#[derive(Deserialize, utoipa::ToSchema)]
struct DeployBody {
    name: String,
    image: String,
    port: Option<u16>,
}

#[utoipa::path(
    post,
    path = "/app/deploy",
    request_body = DeployBody,
    responses(
        (status = 200, description = "Application deployed"),
        (status = 500, description = "Internal server error"),
    ),
    tag = "apps"
)]
#[post("/app/deploy")]
async fn deploy_app(
    scheduler: web::Data<Scheduler>,
    pool: web::Data<PgPool>,
    body: web::Json<DeployBody>,
) -> impl Responder {
    let internal_port = match body.port {
        Some(p) => p,
        None => {
            return HttpResponse::BadRequest()
                .json(json!({"error": "port is required: specify the port your app listens on"}));
        }
    };
    if let Err(e) = scheduler
        .deploy(&pool, &body.name, &body.image, internal_port)
        .await
    {
        return HttpResponse::InternalServerError().json(json!({"error": e}));
    }
    HttpResponse::Ok().finish()
}

#[utoipa::path(
    post,
    path = "/app/{app_name}/stop",
    params(("app_name" = String, Path, description = "Application name")),
    responses(
        (status = 200, description = "Application stopped"),
        (status = 404, description = "Application not found"),
        (status = 500, description = "Internal server error"),
    ),
    tag = "apps"
)]
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

#[utoipa::path(
    post,
    path = "/app/{app_name}/restart",
    params(("app_name" = String, Path, description = "Application name")),
    responses(
        (status = 200, description = "Application restarted"),
        (status = 404, description = "Application not found"),
        (status = 500, description = "Internal server error"),
    ),
    tag = "apps"
)]
#[post("/app/{app_name}/restart")]
async fn restart_app(
    scheduler: web::Data<Scheduler>,
    pool: web::Data<PgPool>,
    path: web::Path<String>,
) -> impl Responder {
    let app_name = path.into_inner();
    let app = match Registry::get(&pool, &app_name).await {
        Ok(Some(app)) => app,
        Ok(None) => return HttpResponse::NotFound().finish(),
        Err(e) => {
            eprintln!("registry: get failed for {app_name}: {e}");
            return HttpResponse::InternalServerError().finish();
        }
    };

    let image = match app.image_id.as_deref().filter(|s| !s.is_empty()) {
        Some(img) => img.to_string(),
        None => {
            if let Err(e) = Registry::update_status(&pool, &app_name, "running").await {
                eprintln!("registry: failed to update status for {app_name}: {e}");
            }
            return HttpResponse::Ok().finish();
        }
    };

    let internal_port = match app.internal_port {
        Some(p) => p as u16,
        None => {
            return HttpResponse::BadRequest().json(json!({"error": "internal_port missing: cannot restart an app without a configured internal port"}));
        }
    };
    let host_port = match app.port {
        Some(p) => p as u16,
        None => match find_free_port() {
            Ok(p) => p,
            Err(e) => {
                eprintln!("docker: failed to find free port for {app_name}: {e}");
                return HttpResponse::InternalServerError().finish();
            }
        },
    };

    if let Err(e) = scheduler
        .redeploy(&pool, &app_name, &image, internal_port, host_port)
        .await
    {
        return HttpResponse::InternalServerError().json(json!({"error": e}));
    }
    HttpResponse::Ok().finish()
}

#[utoipa::path(
    get,
    path = "/app/{app_name}/status",
    params(("app_name" = String, Path, description = "Application name")),
    responses(
        (status = 200, description = "Application status", body = String),
        (status = 404, description = "Application not found"),
        (status = 500, description = "Internal server error"),
    ),
    tag = "apps"
)]
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

#[utoipa::path(
    post,
    path = "/resource",
    request_body = CreateResourcePayload,
    responses(
        (status = 201, description = "Resource created", body = Resource),
        (status = 400, description = "Invalid service name or version"),
        (status = 500, description = "Internal server error"),
    ),
    tag = "resources"
)]
#[post("/resource")]
async fn create_resource(
    pool: web::Data<PgPool>,
    client: web::Data<Client>,
    scheduler: web::Data<Scheduler>,
    payload: web::Json<CreateResourcePayload>,
) -> Result<impl Responder, Error> {
    if !is_valid_service(&payload.name) {
        return Err(error::ErrorBadRequest(format!(
            "Invalid service name '{}'. Must be one of: {}",
            payload.name,
            valid_services().join(", ")
        )));
    }

    let docker_image = docker_image_for_service(&payload.name);
    validate_docker_tag(&client, docker_image, &payload.version).await?;

    let id = Uuid::new_v4();

    let mut tx = pool
        .begin()
        .await
        .map_err(error::ErrorInternalServerError)?;

    sqlx::query!(
        "INSERT INTO services (id, display_name, name, version) VALUES ($1, $2, $3, $4)",
        id,
        payload.display_name,
        payload.name,
        payload.version,
    )
    .execute(&mut *tx)
    .await
    .map_err(error::ErrorInternalServerError)?;

    if let Some(app_id) = &payload.application_id
        && !app_id.is_empty()
    {
        let app_uuid = Uuid::parse_str(app_id)
            .map_err(|_| error::ErrorBadRequest("Invalid application_id"))?;
        sqlx::query!(
            "INSERT INTO application_services (application_id, service_id) VALUES ($1, $2)",
            app_uuid,
            id,
        )
        .execute(&mut *tx)
        .await
        .map_err(error::ErrorInternalServerError)?;
    }

    let default_env = default_env_vars_for_service(&payload.name);
    for (key, value) in &default_env {
        sqlx::query!(
            "INSERT INTO service_env_vars (service_id, key, value) VALUES ($1, $2, $3)",
            id,
            key,
            value,
        )
        .execute(&mut *tx)
        .await
        .map_err(error::ErrorInternalServerError)?;
    }

    let image = format!(
        "{}:{}",
        container_image_for_service(&payload.name),
        payload.version
    );
    let container_port = service_port_for_service(&payload.name);
    let env_vars: Vec<String> = default_env
        .iter()
        .map(|(k, v)| format!("{k}={v}"))
        .collect();
    let binds = prepare_config_for_service(&payload.name, &id.to_string())
        .map_err(error::ErrorInternalServerError)?;
    let (container_id, host_port) = scheduler
        .start_service(
            &id.to_string(),
            &image,
            container_port,
            None,
            env_vars,
            binds,
        )
        .await
        .map_err(|e| error::ErrorInternalServerError(format!("Failed to start service: {e}")))?;

    sqlx::query!(
        "UPDATE services SET status = 'running', container_id = $1, port = $2 WHERE id = $3",
        container_id,
        host_port as i32,
        id,
    )
    .execute(&mut *tx)
    .await
    .map_err(error::ErrorInternalServerError)?;

    tx.commit().await.map_err(error::ErrorInternalServerError)?;

    Ok(HttpResponse::Created().json(Resource {
        id: id.to_string(),
        display_name: payload.display_name.clone(),
        name: payload.name.clone(),
        version: payload.version.clone(),
        status: "running".to_string(),
        application_ids: payload
            .application_id
            .as_deref()
            .filter(|s| !s.is_empty())
            .map(|s| vec![s.to_string()])
            .unwrap_or_default(),
    }))
}

#[utoipa::path(
    get,
    path = "/resource",
    responses(
        (status = 200, description = "List of resources", body = Vec<Resource>),
        (status = 500, description = "Internal server error"),
    ),
    tag = "resources"
)]
#[get("/resource")]
async fn get_resources(pool: web::Data<PgPool>) -> Result<impl Responder, Error> {
    let resources = sqlx::query_as!(
        Resource,
        r#"SELECT
            s.id::text as "id!",
            s.display_name,
            s.name,
            s.version,
            s.status,
            COALESCE(array_agg(aps.application_id::text) FILTER (WHERE aps.application_id IS NOT NULL), '{}') as "application_ids!: Vec<String>"
        FROM services s
        LEFT JOIN application_services aps ON s.id = aps.service_id
        GROUP BY s.id, s.display_name, s.name, s.version, s.status"#
    )
    .fetch_all(pool.get_ref())
    .await
    .map_err(error::ErrorInternalServerError)?;

    Ok(HttpResponse::Ok().json(resources))
}

#[utoipa::path(
    get,
    path = "/resource/{id}",
    params(("id" = String, Path, description = "Resource UUID")),
    responses(
        (status = 200, description = "Resource found", body = Resource),
        (status = 400, description = "Invalid UUID"),
        (status = 404, description = "Resource not found"),
        (status = 500, description = "Internal server error"),
    ),
    tag = "resources"
)]
#[get("/resource/{id}")]
async fn get_resource(
    pool: web::Data<PgPool>,
    id: web::Path<String>,
) -> Result<impl Responder, Error> {
    let uuid = Uuid::parse_str(&id).map_err(|_| error::ErrorBadRequest("Invalid resource id"))?;

    let resource = sqlx::query_as!(
        Resource,
        r#"SELECT
            s.id::text as "id!",
            s.display_name,
            s.name,
            s.version,
            s.status,
            COALESCE(array_agg(aps.application_id::text) FILTER (WHERE aps.application_id IS NOT NULL), '{}') as "application_ids!: Vec<String>"
        FROM services s
        LEFT JOIN application_services aps ON s.id = aps.service_id
        WHERE s.id = $1
        GROUP BY s.id, s.display_name, s.name, s.version, s.status"#,
        uuid
    )
    .fetch_optional(pool.get_ref())
    .await
    .map_err(error::ErrorInternalServerError)?
    .ok_or_else(|| error::ErrorNotFound("Resource not found"))?;

    Ok(HttpResponse::Ok().json(resource))
}

#[utoipa::path(
    patch,
    path = "/resource/{id}",
    params(("id" = String, Path, description = "Resource UUID")),
    request_body = UpdateResourcePayload,
    responses(
        (status = 200, description = "Resource updated"),
        (status = 400, description = "Invalid UUID or version"),
        (status = 404, description = "Resource not found"),
        (status = 500, description = "Internal server error"),
    ),
    tag = "resources"
)]
#[patch("/resource/{id}")]
async fn update_resource(
    pool: web::Data<PgPool>,
    client: web::Data<Client>,
    id: web::Path<String>,
    payload: web::Json<UpdateResourcePayload>,
) -> Result<impl Responder, Error> {
    let uuid = Uuid::parse_str(&id).map_err(|_| error::ErrorBadRequest("Invalid resource id"))?;

    if let Some(version) = &payload.version {
        let service = sqlx::query!("SELECT name FROM services WHERE id = $1", uuid)
            .fetch_optional(pool.get_ref())
            .await
            .map_err(error::ErrorInternalServerError)?
            .ok_or_else(|| error::ErrorNotFound("Resource not found"))?;

        let docker_image = docker_image_for_service(&service.name);
        validate_docker_tag(&client, docker_image, version).await?;
    }

    let mut tx = pool
        .get_ref()
        .begin()
        .await
        .map_err(error::ErrorInternalServerError)?;

    let result = sqlx::query!(
        r#"UPDATE services
        SET
            display_name = COALESCE($1, display_name),
            version = COALESCE($2, version)
        WHERE id = $3"#,
        payload.display_name,
        payload.version,
        uuid,
    )
    .execute(&mut *tx)
    .await
    .map_err(error::ErrorInternalServerError)?;

    if result.rows_affected() == 0 {
        return Err(error::ErrorNotFound("Resource not found"));
    }

    if let Some(app_ids) = &payload.application_ids {
        let app_uuids: Vec<Uuid> = app_ids
            .iter()
            .map(|s| {
                Uuid::parse_str(s).map_err(|_| error::ErrorBadRequest("Invalid application_id"))
            })
            .collect::<Result<_, _>>()?;

        sqlx::query!(
            "DELETE FROM application_services WHERE service_id = $1",
            uuid
        )
        .execute(&mut *tx)
        .await
        .map_err(error::ErrorInternalServerError)?;

        for app_uuid in app_uuids {
            sqlx::query!(
                "INSERT INTO application_services (application_id, service_id) VALUES ($1, $2)",
                app_uuid,
                uuid,
            )
            .execute(&mut *tx)
            .await
            .map_err(error::ErrorInternalServerError)?;
        }
    }

    tx.commit().await.map_err(error::ErrorInternalServerError)?;

    Ok(HttpResponse::Ok().body("Resource successfully updated"))
}

#[utoipa::path(
    post,
    path = "/resource/{id}/start",
    params(("id" = String, Path, description = "Resource UUID")),
    responses(
        (status = 200, description = "Resource started"),
        (status = 400, description = "Invalid UUID"),
        (status = 404, description = "Resource not found"),
        (status = 409, description = "Resource already running"),
        (status = 500, description = "Internal server error"),
    ),
    tag = "resources"
)]
#[post("/resource/{id}/start")]
async fn start_resource(
    pool: web::Data<PgPool>,
    scheduler: web::Data<Scheduler>,
    id: web::Path<String>,
) -> Result<impl Responder, Error> {
    let uuid = Uuid::parse_str(&id).map_err(|_| error::ErrorBadRequest("Invalid resource id"))?;

    let service = sqlx::query!(
        "SELECT name, version, status, port FROM services WHERE id = $1",
        uuid
    )
    .fetch_optional(pool.get_ref())
    .await
    .map_err(error::ErrorInternalServerError)?
    .ok_or_else(|| error::ErrorNotFound("Resource not found"))?;

    if service.status == "running" {
        return Err(error::ErrorConflict("Resource is already running"));
    }

    let image = format!(
        "{}:{}",
        container_image_for_service(&service.name),
        service.version
    );
    let container_port = service_port_for_service(&service.name);
    let existing_port = service.port.map(|p| p as u16);
    let env_vars = fetch_resource_env_vars(pool.get_ref(), uuid).await?;
    let binds = prepare_config_for_service(&service.name, &uuid.to_string())
        .map_err(error::ErrorInternalServerError)?;
    let (container_id, host_port) = scheduler
        .start_service(
            &uuid.to_string(),
            &image,
            container_port,
            existing_port,
            env_vars,
            binds,
        )
        .await
        .map_err(|e| error::ErrorInternalServerError(format!("Failed to start service: {e}")))?;

    sqlx::query!(
        "UPDATE services SET status = 'running', container_id = $1, port = $2 WHERE id = $3",
        container_id,
        host_port as i32,
        uuid,
    )
    .execute(pool.get_ref())
    .await
    .map_err(error::ErrorInternalServerError)?;

    Ok(HttpResponse::Ok().body("Resource started"))
}

#[utoipa::path(
    post,
    path = "/resource/{id}/stop",
    params(("id" = String, Path, description = "Resource UUID")),
    responses(
        (status = 200, description = "Resource stopped"),
        (status = 400, description = "Invalid UUID"),
        (status = 404, description = "Resource not found"),
        (status = 409, description = "Resource already stopped"),
        (status = 500, description = "Internal server error"),
    ),
    tag = "resources"
)]
#[post("/resource/{id}/stop")]
async fn stop_resource(
    pool: web::Data<PgPool>,
    scheduler: web::Data<Scheduler>,
    id: web::Path<String>,
) -> Result<impl Responder, Error> {
    let uuid = Uuid::parse_str(&id).map_err(|_| error::ErrorBadRequest("Invalid resource id"))?;

    let service = sqlx::query!("SELECT status FROM services WHERE id = $1", uuid)
        .fetch_optional(pool.get_ref())
        .await
        .map_err(error::ErrorInternalServerError)?
        .ok_or_else(|| error::ErrorNotFound("Resource not found"))?;

    if service.status == "stopped" {
        return Err(error::ErrorConflict("Resource is already stopped"));
    }

    scheduler
        .stop_service(&uuid.to_string())
        .await
        .map_err(|e| error::ErrorInternalServerError(format!("Failed to stop service: {e}")))?;

    sqlx::query!(
        "UPDATE services SET status = 'stopped', container_id = NULL WHERE id = $1",
        uuid,
    )
    .execute(pool.get_ref())
    .await
    .map_err(error::ErrorInternalServerError)?;

    Ok(HttpResponse::Ok().body("Resource stopped"))
}

#[utoipa::path(
    delete,
    path = "/resource/{id}",
    params(("id" = String, Path, description = "Resource UUID")),
    responses(
        (status = 204, description = "Resource deleted"),
        (status = 400, description = "Invalid UUID"),
        (status = 404, description = "Resource not found"),
        (status = 500, description = "Internal server error"),
    ),
    tag = "resources"
)]
#[delete("/resource/{id}")]
async fn delete_resource(
    pool: web::Data<PgPool>,
    id: web::Path<String>,
) -> Result<impl Responder, Error> {
    let uuid = Uuid::parse_str(&id).map_err(|_| error::ErrorBadRequest("Invalid resource id"))?;

    let result = sqlx::query!("DELETE FROM services WHERE id = $1", uuid)
        .execute(pool.get_ref())
        .await
        .map_err(error::ErrorInternalServerError)?;

    if result.rows_affected() == 0 {
        return Err(error::ErrorNotFound("Resource not found"));
    }

    Ok(HttpResponse::NoContent().finish())
}

#[utoipa::path(
    get,
    path = "/resource/{id}/env",
    params(("id" = String, Path, description = "Resource UUID")),
    responses(
        (status = 200, description = "Environment variables as key-value map"),
        (status = 400, description = "Invalid UUID"),
        (status = 404, description = "Resource not found"),
        (status = 500, description = "Internal server error"),
    ),
    tag = "resources"
)]
#[get("/resource/{id}/env")]
async fn get_resource_env(
    pool: web::Data<PgPool>,
    id: web::Path<String>,
) -> Result<impl Responder, Error> {
    let uuid = Uuid::parse_str(&id).map_err(|_| error::ErrorBadRequest("Invalid resource id"))?;

    sqlx::query!("SELECT id FROM services WHERE id = $1", uuid)
        .fetch_optional(pool.get_ref())
        .await
        .map_err(error::ErrorInternalServerError)?
        .ok_or_else(|| error::ErrorNotFound("Resource not found"))?;

    let rows = sqlx::query!(
        "SELECT key, value FROM service_env_vars WHERE service_id = $1 ORDER BY key",
        uuid
    )
    .fetch_all(pool.get_ref())
    .await
    .map_err(error::ErrorInternalServerError)?;

    let env_map: HashMap<String, String> = rows.into_iter().map(|r| (r.key, r.value)).collect();

    Ok(HttpResponse::Ok().json(env_map))
}

#[utoipa::path(
    put,
    path = "/resource/{id}/env",
    params(("id" = String, Path, description = "Resource UUID")),
    request_body(
        content_type = "application/json",
        description = "Environment variables as key-value map (replaces all existing variables)"
    ),
    responses(
        (status = 200, description = "Environment variables updated — restart resource to apply"),
        (status = 400, description = "Invalid UUID"),
        (status = 404, description = "Resource not found"),
        (status = 500, description = "Internal server error"),
    ),
    tag = "resources"
)]
#[put("/resource/{id}/env")]
async fn update_resource_env(
    pool: web::Data<PgPool>,
    id: web::Path<String>,
    payload: web::Json<HashMap<String, String>>,
) -> Result<impl Responder, Error> {
    let uuid = Uuid::parse_str(&id).map_err(|_| error::ErrorBadRequest("Invalid resource id"))?;

    sqlx::query!("SELECT id FROM services WHERE id = $1", uuid)
        .fetch_optional(pool.get_ref())
        .await
        .map_err(error::ErrorInternalServerError)?
        .ok_or_else(|| error::ErrorNotFound("Resource not found"))?;

    let mut tx = pool
        .begin()
        .await
        .map_err(error::ErrorInternalServerError)?;

    sqlx::query!("DELETE FROM service_env_vars WHERE service_id = $1", uuid)
        .execute(&mut *tx)
        .await
        .map_err(error::ErrorInternalServerError)?;

    for (key, value) in payload.iter() {
        sqlx::query!(
            "INSERT INTO service_env_vars (service_id, key, value) VALUES ($1, $2, $3)",
            uuid,
            key,
            value,
        )
        .execute(&mut *tx)
        .await
        .map_err(error::ErrorInternalServerError)?;
    }

    tx.commit().await.map_err(error::ErrorInternalServerError)?;

    Ok(HttpResponse::Ok()
        .body("Environment variables updated. Restart the resource to apply changes."))
}
