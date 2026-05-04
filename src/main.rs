mod engine;
mod registry;
mod scheduler;

use crate::engine::{extract_zip, launch_code, save_multipart_file};
use actix_multipart::Multipart;
use actix_web::{
    App, Error, HttpResponse, HttpServer, Responder, delete, error, get, patch, post, web,
};
use futures_util::TryStreamExt;
use reqwest::Client;
use serde::{Deserialize, Serialize};
use sqlx::PgPool;
use tokio::fs;
use tokio::fs::File;
use tokio::io::AsyncWriteExt;
use utoipa::{OpenApi, ToSchema};
use utoipa_swagger_ui::SwaggerUi;
use uuid::Uuid;

use registry::Registry;
use scheduler::Scheduler;

const VALID_SERVICES: &[&str] = &["postgres", "redis", "s3"];

fn docker_image_for_service(name: &str) -> &'static str {
    match name {
        "postgres" => "library/postgres",
        "redis" => "library/redis",
        "s3" => "dxflrs/garage",
        _ => unreachable!(),
    }
}

async fn validate_docker_tag(client: &Client, image: &str, tag: &str) -> Result<(), Error> {
    let url = format!(
        "https://hub.docker.com/v2/repositories/{}/tags/{}/",
        image, tag
    );
    let resp = client
        .get(&url)
        .send()
        .await
        .map_err(|_| error::ErrorInternalServerError("Failed to reach Docker Hub"))?;

    match resp.status().as_u16() {
        200 => Ok(()),
        404 => Err(error::ErrorBadRequest(format!(
            "Version '{}' does not exist for this service on Docker Hub",
            tag
        ))),
        _ => Err(error::ErrorInternalServerError(
            "Unexpected response from Docker Hub",
        )),
    }
}

#[derive(Deserialize)]
struct DockerTagsResponse {
    results: Vec<DockerTag>,
}

#[derive(Deserialize)]
struct DockerTag {
    name: String,
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

#[derive(OpenApi)]
#[openapi(
    paths(
        get_service_versions,
        upload_app,
        create_resource,
        get_resources,
        get_resource,
        update_resource,
        delete_resource,
    ),
    components(schemas(Resource, CreateResourcePayload, UpdateResourcePayload)),
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

    let scheduler = web::Data::new(Scheduler::new());

    let http_client = Client::new();

    println!("Running on http://{}:{}", config.host, config.port);
    println!(
        "Swagger UI: http://{}:{}/swagger-ui/",
        config.host, config.port
    );
    HttpServer::new(move || {
        App::new()
            .app_data(web::Data::new(pool.clone()))
            .app_data(web::Data::new(http_client.clone()))
            .service(
                SwaggerUi::new("/swagger-ui/{_:.*}")
                    .url("/api-docs/openapi.json", ApiDoc::openapi()),
            )
            .service(get_service_versions)
            .app_data(scheduler.clone())
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
    })
    .bind((config.host, config.port))?
    .run()
    .await
}

// structs

#[derive(Serialize, Deserialize, ToSchema)]
struct Resource {
    id: String,
    display_name: String,
    name: String,
    version: String,
    application_ids: Vec<String>,
}

#[derive(Deserialize, ToSchema)]
struct CreateResourcePayload {
    display_name: String,
    name: String,
    version: String,
    application_id: Option<String>,
}

#[derive(Deserialize, ToSchema)]
struct UpdateResourcePayload {
    display_name: Option<String>,
    version: Option<String>,
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
    if !VALID_SERVICES.contains(&name.as_str()) {
        return Err(error::ErrorBadRequest(format!(
            "Invalid service name '{}'. Must be one of: {}",
            name,
            VALID_SERVICES.join(", ")
        )));
    }

    let docker_image = docker_image_for_service(&name);
    let url = format!(
        "https://hub.docker.com/v2/repositories/{}/tags/?page_size=50&ordering=last_updated",
        docker_image
    );

    let resp = client
        .get(&url)
        .send()
        .await
        .map_err(|_| error::ErrorInternalServerError("Failed to reach Docker Hub"))?;

    if !resp.status().is_success() {
        return Err(error::ErrorInternalServerError(
            "Failed to fetch versions from Docker Hub",
        ));
    }

    let tags: DockerTagsResponse = resp
        .json()
        .await
        .map_err(|_| error::ErrorInternalServerError("Failed to parse Docker Hub response"))?;

    let versions: Vec<String> = tags.results.into_iter().map(|t| t.name).collect();

    Ok(HttpResponse::Ok().json(versions))
}

#[utoipa::path(
    post,
    path = "/app/upload",
    request_body(content_type = "multipart/form-data", description = "Zip archive of the application"),
    responses(
        (status = 200, description = "File uploaded successfully"),
        (status = 500, description = "Internal server error"),
    ),
    tag = "apps"
)]
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
    payload: web::Json<CreateResourcePayload>,
) -> Result<impl Responder, Error> {
    if !VALID_SERVICES.contains(&payload.name.as_str()) {
        return Err(error::ErrorBadRequest(format!(
            "Invalid service name '{}'. Must be one of: {}",
            payload.name,
            VALID_SERVICES.join(", ")
        )));
    }

    let docker_image = docker_image_for_service(&payload.name);
    validate_docker_tag(&client, docker_image, &payload.version).await?;

    let id = Uuid::new_v4();

    sqlx::query!(
        "INSERT INTO services (id, display_name, name, version) VALUES ($1, $2, $3, $4)",
        id,
        payload.display_name,
        payload.name,
        payload.version,
    )
    .execute(pool.get_ref())
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
        .execute(pool.get_ref())
        .await
        .map_err(error::ErrorInternalServerError)?;
    }

    Ok(HttpResponse::Created().json(Resource {
        id: id.to_string(),
        display_name: payload.display_name.clone(),
        name: payload.name.clone(),
        version: payload.version.clone(),
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
            COALESCE(array_agg(aps.application_id::text) FILTER (WHERE aps.application_id IS NOT NULL), '{}') as "application_ids!: Vec<String>"
        FROM services s
        LEFT JOIN application_services aps ON s.id = aps.service_id
        GROUP BY s.id, s.display_name, s.name, s.version"#
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
            COALESCE(array_agg(aps.application_id::text) FILTER (WHERE aps.application_id IS NOT NULL), '{}') as "application_ids!: Vec<String>"
        FROM services s
        LEFT JOIN application_services aps ON s.id = aps.service_id
        WHERE s.id = $1
        GROUP BY s.id, s.display_name, s.name, s.version"#,
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
    .execute(pool.get_ref())
    .await
    .map_err(error::ErrorInternalServerError)?;

    if result.rows_affected() == 0 {
        return Err(error::ErrorNotFound("Resource not found"));
    }

    Ok(HttpResponse::Ok().body("Resource successfully updated"))
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
