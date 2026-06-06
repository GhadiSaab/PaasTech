mod docker;
mod engine;
mod models;
mod registry;
mod scheduler;

use actix_multipart::Multipart;
use actix_web::{
    App, Error, HttpResponse, HttpServer, Responder, delete, error, get, patch, post, put, web,
};
use docker::{
    connection_env_vars_for_service, container_image_for_service, default_env_vars_for_service,
    docker_image_for_service, fetch_service_versions, is_valid_service, prepare_config_for_service,
    service_port_for_service, valid_services, validate_connection_profile_for_service,
    validate_docker_tag,
};
use reqwest::Client;
use serde::Deserialize;
use serde_json::json;
use sqlx::{PgPool, Row};
use std::collections::HashMap;
use tokio::fs;
use utoipa::OpenApi;
use utoipa_swagger_ui::SwaggerUi;
use uuid::Uuid;

use crate::engine::{
    MultipartData, ProcessType, build_image_with_name, extract_zip, load_process_definitions,
    save_multipart_file,
};
use crate::models::{
    CreateProjectPayload, CreateResourcePayload, Resource, ResourceAttachment,
    UpdateResourcePayload,
};
use crate::registry::Registry;
use crate::scheduler::{
    DeployError, Scheduler, app_container_name, app_process_container_name, find_free_port,
};

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
        create_project,
        list_projects,
        get_project,
        delete_project,
        get_project_env,
        update_project_env,
        upload_app,
        list_project_apps,
        deploy_project_app,
        list_apps,
        deploy_app,
        update_app,
        stop_app,
        restart_app,
        status_app,
        delete_app,
        logs_app,
        logs_resource,
        get_app_env,
        set_app_env,
        update_app_env,
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
    components(schemas(registry::App, registry::Project, DeployBody, Resource, CreateProjectPayload, CreateResourcePayload, UpdateResourcePayload)),
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
            .service(get_service_versions)
            .service(create_project)
            .service(list_projects)
            .service(get_project)
            .service(delete_project)
            .service(get_project_env)
            .service(update_project_env)
            .service(upload_app)
            .service(upload_project_app)
            .service(list_project_apps)
            .service(deploy_project_app)
            .service(list_apps)
            .service(deploy_app)
            .service(update_app)
            .service(stop_app)
            .service(stop_project_app)
            .service(restart_app)
            .service(restart_project_app)
            .service(status_app)
            .service(status_project_app)
            .service(delete_app)
            .service(delete_project_app)
            .service(logs_app)
            .service(logs_project_app)
            .service(logs_resource)
            .service(get_app_env)
            .service(get_project_app_env)
            .service(set_app_env)
            .service(set_project_app_env)
            .service(update_app_env)
            .service(update_project_app_env)
            .service(create_resource)
            .service(create_project_resource)
            .service(get_project_resources)
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

async fn fetch_project_env_vars(
    pool: &PgPool,
    project_id: Uuid,
) -> Result<HashMap<String, String>, Error> {
    fetch_project_env_vars_db(pool, project_id)
        .await
        .map_err(error::ErrorInternalServerError)
}

async fn fetch_project_env_vars_db(
    pool: &PgPool,
    project_id: Uuid,
) -> Result<HashMap<String, String>, sqlx::Error> {
    let rows =
        sqlx::query("SELECT key, value FROM project_env_vars WHERE project_id = $1 ORDER BY key")
            .bind(project_id)
            .fetch_all(pool)
            .await?;

    let mut env = HashMap::new();
    for row in rows {
        let key: String = row.try_get("key")?;
        let value: String = row.try_get("value")?;
        env.insert(key, value);
    }
    Ok(env)
}

async fn merged_app_env_vars(
    pool: &PgPool,
    project_id: Uuid,
    application_id: Uuid,
) -> Result<Vec<String>, sqlx::Error> {
    let mut env = fetch_project_env_vars_db(pool, project_id).await?;
    for pair in Registry::get_app_env(pool, application_id).await? {
        if let Some((key, value)) = pair.split_once('=') {
            env.insert(key.to_string(), value.to_string());
        }
    }
    let mut pairs: Vec<_> = env.into_iter().collect();
    pairs.sort_by(|(left, _), (right, _)| left.cmp(right));
    Ok(pairs
        .into_iter()
        .map(|(key, value)| format!("{key}={value}"))
        .collect())
}

async fn default_project(pool: &PgPool) -> Result<registry::Project, Error> {
    Registry::ensure_default_project(pool)
        .await
        .map_err(error::ErrorInternalServerError)
}

async fn project_by_name(pool: &PgPool, name: &str) -> Result<registry::Project, Error> {
    Registry::get_project(pool, name)
        .await
        .map_err(error::ErrorInternalServerError)?
        .ok_or_else(|| error::ErrorNotFound("Project not found"))
}

// routes

#[utoipa::path(
    post,
    path = "/project",
    request_body = CreateProjectPayload,
    responses(
        (status = 201, description = "Project created", body = registry::Project),
        (status = 409, description = "Project already exists"),
        (status = 500, description = "Internal server error"),
    ),
    tag = "projects"
)]
#[post("/project")]
async fn create_project(
    pool: web::Data<PgPool>,
    payload: web::Json<CreateProjectPayload>,
) -> Result<impl Responder, Error> {
    let name = payload.name.trim();
    if name.is_empty()
        || name
            .chars()
            .any(|c| !(c.is_ascii_alphanumeric() || c == '-'))
    {
        return Err(error::ErrorBadRequest(
            "Project name must only contain letters, numbers, and hyphens",
        ));
    }

    match Registry::create_project(&pool, name).await {
        Ok(project) => Ok(HttpResponse::Created().json(project)),
        Err(sqlx::Error::Database(e)) if e.is_unique_violation() => {
            Err(error::ErrorConflict("Project already exists"))
        }
        Err(e) => Err(error::ErrorInternalServerError(e)),
    }
}

#[utoipa::path(
    get,
    path = "/project",
    responses((status = 200, description = "Projects", body = Vec<registry::Project>)),
    tag = "projects"
)]
#[get("/project")]
async fn list_projects(pool: web::Data<PgPool>) -> Result<impl Responder, Error> {
    Registry::ensure_default_project(&pool)
        .await
        .map_err(error::ErrorInternalServerError)?;
    let projects = Registry::list_projects(&pool)
        .await
        .map_err(error::ErrorInternalServerError)?;
    Ok(HttpResponse::Ok().json(projects))
}

#[utoipa::path(
    get,
    path = "/project/{project}",
    params(("project" = String, Path, description = "Project name")),
    responses(
        (status = 200, description = "Project", body = registry::Project),
        (status = 404, description = "Project not found"),
    ),
    tag = "projects"
)]
#[get("/project/{project}")]
async fn get_project(
    pool: web::Data<PgPool>,
    path: web::Path<String>,
) -> Result<impl Responder, Error> {
    let project = project_by_name(&pool, &path.into_inner()).await?;
    Ok(HttpResponse::Ok().json(project))
}

#[utoipa::path(
    delete,
    path = "/project/{project}",
    params(("project" = String, Path, description = "Project name")),
    responses(
        (status = 204, description = "Project deleted"),
        (status = 404, description = "Project not found"),
    ),
    tag = "projects"
)]
#[delete("/project/{project}")]
async fn delete_project(
    pool: web::Data<PgPool>,
    path: web::Path<String>,
) -> Result<impl Responder, Error> {
    let name = path.into_inner();
    if name == registry::DEFAULT_PROJECT_NAME {
        return Err(error::ErrorBadRequest("Default project cannot be deleted"));
    }
    let affected = Registry::delete_project(&pool, &name)
        .await
        .map_err(error::ErrorInternalServerError)?;
    if affected == 0 {
        return Err(error::ErrorNotFound("Project not found"));
    }
    Ok(HttpResponse::NoContent().finish())
}

#[utoipa::path(
    get,
    path = "/project/{project}/env",
    params(("project" = String, Path, description = "Project name")),
    responses(
        (status = 200, description = "Project environment variables"),
        (status = 404, description = "Project not found"),
    ),
    tag = "projects"
)]
#[get("/project/{project}/env")]
async fn get_project_env(
    pool: web::Data<PgPool>,
    path: web::Path<String>,
) -> Result<impl Responder, Error> {
    let project = project_by_name(&pool, &path.into_inner()).await?;
    let env = fetch_project_env_vars(&pool, project.id).await?;
    Ok(HttpResponse::Ok().json(env))
}

#[utoipa::path(
    put,
    path = "/project/{project}/env",
    params(("project" = String, Path, description = "Project name")),
    request_body(content_type = "application/json", description = "Environment variables as key-value map"),
    responses(
        (status = 200, description = "Project environment variables updated"),
        (status = 404, description = "Project not found"),
    ),
    tag = "projects"
)]
#[put("/project/{project}/env")]
async fn update_project_env(
    pool: web::Data<PgPool>,
    path: web::Path<String>,
    payload: web::Json<HashMap<String, String>>,
) -> Result<impl Responder, Error> {
    let project = project_by_name(&pool, &path.into_inner()).await?;
    let mut tx = pool
        .begin()
        .await
        .map_err(error::ErrorInternalServerError)?;
    sqlx::query("DELETE FROM project_env_vars WHERE project_id = $1")
        .bind(project.id)
        .execute(&mut *tx)
        .await
        .map_err(error::ErrorInternalServerError)?;
    for (key, value) in payload.iter() {
        sqlx::query("INSERT INTO project_env_vars (project_id, key, value) VALUES ($1, $2, $3)")
            .bind(project.id)
            .bind(key)
            .bind(value)
            .execute(&mut *tx)
            .await
            .map_err(error::ErrorInternalServerError)?;
    }
    tx.commit().await.map_err(error::ErrorInternalServerError)?;
    Ok(HttpResponse::Ok()
        .body("Project environment variables updated. Restart apps to apply changes."))
}

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

async fn handle_upload(
    pool: web::Data<PgPool>,
    scheduler: web::Data<Scheduler>,
    project: registry::Project,
    data: MultipartData,
) -> Result<impl Responder, Error> {
    let zip_filepath = match data.file_path {
        Some(path) => path,
        None => return Ok(HttpResponse::BadRequest().body("provide file in payload")),
    };

    let internal_port = data
        .fields
        .get("internal_port")
        .and_then(|p| p.trim().parse::<u16>().ok());

    let name = format!("paastech-{}", Uuid::new_v4());

    let extracted_folder = extract_zip(zip_filepath)
        .await
        .map_err(error::ErrorInternalServerError)?;

    let processes = match load_process_definitions(&extracted_folder, internal_port) {
        Ok(processes) => processes,
        Err(e) => {
            let _ = fs::remove_dir_all(&extracted_folder).await;
            return Ok(HttpResponse::BadRequest().json(json!({"error": e})));
        }
    };

    let app = match Registry::save_in_project(
        &pool,
        project.id,
        &name,
        "",
        "",
        internal_port.map(|p| p as i32),
        0,
        "building",
        None,
    )
    .await
    {
        Ok(app) => app,
        Err(e) => {
            let _ = fs::remove_dir_all(&extracted_folder).await;
            return Ok(HttpResponse::InternalServerError().json(json!({"error": e.to_string()})));
        }
    };

    let mut process_rows = Vec::new();
    for process in &processes {
        let row = match Registry::create_process(
            &pool,
            app.id,
            &process.name,
            process.process_type.as_str(),
            &process.path,
            process.public_host.as_deref(),
            json!(process.build_env),
            process.port.map(|p| p as i32),
            "building",
        )
        .await
        {
            Ok(row) => row,
            Err(e) => {
                let _ = fs::remove_dir_all(&extracted_folder).await;
                let _ = Registry::delete(&pool, &name).await;
                return Ok(
                    HttpResponse::InternalServerError().json(json!({"error": e.to_string()}))
                );
            }
        };
        process_rows.push(row);
    }

    let pool_bg = pool.clone();
    let scheduler_bg = scheduler.clone();
    let project_id_bg = project.id;
    let project_network_bg = project.network_name.clone();
    let name_bg = name.clone();
    tokio::spawn(async move {
        let mut failed = false;

        for (process, row) in processes.into_iter().zip(process_rows) {
            let image_name = format!("{}-{}", name_bg, process.name);
            let context = extracted_folder.join(&process.path);
            let env_vars =
                match merged_app_env_vars(&pool_bg, project_id_bg, row.application_id).await {
                    Ok(env_vars) => env_vars,
                    Err(e) => {
                        eprintln!(
                            "registry: failed to load env vars for {} process {}: {e}",
                            name_bg, process.name
                        );
                        let _ = Registry::update_process_status(&pool_bg, row.id, "failed").await;
                        failed = true;
                        break;
                    }
                };

            if let Err(e) = build_image_with_name(
                &image_name,
                context.to_string_lossy().to_string(),
                scheduler_bg.docker_host(),
                &process.build_env,
            )
            .await
            {
                eprintln!(
                    "build: failed for {} process {}: {e}",
                    name_bg, process.name
                );
                let _ = Registry::update_process_status(&pool_bg, row.id, "failed").await;
                failed = true;
                break;
            }

            match scheduler_bg
                .start_process(
                    project_id_bg,
                    &project_network_bg,
                    &name_bg,
                    &process.name,
                    &process.process_type,
                    &image_name,
                    process.port,
                    None,
                    process.public_host.as_deref(),
                    env_vars,
                )
                .await
            {
                Ok(started) => {
                    let _ = Registry::update_process_running(
                        &pool_bg,
                        row.id,
                        &image_name,
                        &started.container_id,
                        started.internal_port.map(|p| p as i32),
                        started.host_port.map(|p| p as i32),
                    )
                    .await;
                }
                Err(e) => {
                    eprintln!(
                        "deploy: failed for {} process {}: {e}",
                        name_bg, process.name
                    );
                    let _ = Registry::update_process_status(&pool_bg, row.id, "failed").await;
                    failed = true;
                    break;
                }
            }
        }

        let _ = fs::remove_dir_all(&extracted_folder).await;

        let status = if failed { "failed" } else { "running" };
        let _ = Registry::update_status(&pool_bg, project_id_bg, &name_bg, status).await;
    });

    Ok(HttpResponse::Accepted().json(json!({"name": name})))
}

#[utoipa::path(
    post,
    path = "/app/upload",
    request_body(content_type = "multipart/form-data", description = "Zip archive of the application. Form fields: file (zip), name (app name), internal_port (container port to expose)"),
    responses(
        (status = 202, description = "Upload accepted, build running in background"),
        (status = 400, description = "Missing file or invalid manifest"),
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
    let project = default_project(&pool).await?;
    let data = save_multipart_file(payload).await?;
    handle_upload(pool, scheduler, project, data).await
}

#[post("/project/{project}/app/upload")]
async fn upload_project_app(
    pool: web::Data<PgPool>,
    scheduler: web::Data<Scheduler>,
    path: web::Path<String>,
    payload: Multipart,
) -> Result<impl Responder, Error> {
    let project = project_by_name(&pool, &path.into_inner()).await?;
    let data = save_multipart_file(payload).await?;
    handle_upload(pool, scheduler, project, data).await
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

#[utoipa::path(
    get,
    path = "/project/{project}/app",
    params(("project" = String, Path, description = "Project name")),
    responses(
        (status = 200, description = "List of project applications", body = Vec<registry::App>),
        (status = 404, description = "Project not found"),
        (status = 500, description = "Internal server error"),
    ),
    tag = "apps"
)]
#[get("/project/{project}/app")]
async fn list_project_apps(
    pool: web::Data<PgPool>,
    path: web::Path<String>,
) -> Result<impl Responder, Error> {
    let project = project_by_name(&pool, &path.into_inner()).await?;
    let apps = Registry::list_in_project(&pool, project.id)
        .await
        .map_err(error::ErrorInternalServerError)?;
    Ok(HttpResponse::Ok().json(apps))
}

#[derive(Deserialize, utoipa::ToSchema)]
struct DeployBody {
    name: String,
    image: String,
    port: Option<u16>,
    base_domain: Option<String>,
}

#[utoipa::path(
    post,
    path = "/app/deploy",
    request_body = DeployBody,
    responses(
        (status = 200, description = "Application deployed"),
        (status = 422, description = "Internal application port is required"),
        (status = 500, description = "Internal server error"),
    ),
    tag = "apps"
)]
#[post("/app/deploy")]
async fn deploy_app(
    scheduler: web::Data<Scheduler>,
    pool: web::Data<PgPool>,
    body: web::Json<DeployBody>,
) -> HttpResponse {
    let project = match default_project(&pool).await {
        Ok(project) => project,
        Err(e) => return e.error_response(),
    };
    let existing_app = match Registry::get_in_project(&pool, project.id, &body.name).await {
        Ok(app) => app,
        Err(e) => {
            eprintln!("deploy: failed to look up {}: {e}", body.name);
            return HttpResponse::InternalServerError().finish();
        }
    };
    let env_vars = if let Some(app) = existing_app {
        match merged_app_env_vars(&pool, project.id, app.id).await {
            Ok(env) => env,
            Err(e) => {
                eprintln!("deploy: failed to load env vars for {}: {e}", body.name);
                return HttpResponse::InternalServerError().finish();
            }
        }
    } else {
        match fetch_project_env_vars(&pool, project.id).await {
            Ok(env) => env
                .into_iter()
                .map(|(key, value)| format!("{key}={value}"))
                .collect(),
            Err(e) => {
                eprintln!(
                    "deploy: failed to load project env vars for {}: {e}",
                    body.name
                );
                return HttpResponse::InternalServerError().finish();
            }
        }
    };
    match scheduler
        .deploy(
            &pool,
            project.id,
            &project.network_name,
            &body.name,
            &body.image,
            body.port,
            env_vars,
            body.base_domain.as_deref(),
        )
        .await
    {
        Ok(()) => HttpResponse::Ok().finish(),
        Err(DeployError::AppNotFound(message)) => HttpResponse::NotFound().body(message),
        Err(DeployError::PortRequired(message)) => {
            HttpResponse::UnprocessableEntity().body(message)
        }
        Err(DeployError::Other(message)) => {
            eprintln!("deploy: failed to deploy {}: {message}", body.name);
            HttpResponse::InternalServerError().body(message)
        }
    }
}

#[utoipa::path(
    post,
    path = "/project/{project}/app/deploy",
    params(("project" = String, Path, description = "Project name")),
    request_body = DeployBody,
    responses(
        (status = 200, description = "Application deployed"),
        (status = 404, description = "Project not found"),
        (status = 422, description = "Internal application port is required"),
        (status = 500, description = "Internal server error"),
    ),
    tag = "apps"
)]
#[post("/project/{project}/app/deploy")]
async fn deploy_project_app(
    scheduler: web::Data<Scheduler>,
    pool: web::Data<PgPool>,
    path: web::Path<String>,
    body: web::Json<DeployBody>,
) -> HttpResponse {
    let project = match project_by_name(&pool, &path.into_inner()).await {
        Ok(project) => project,
        Err(e) => return e.error_response(),
    };
    let existing_app = match Registry::get_in_project(&pool, project.id, &body.name).await {
        Ok(app) => app,
        Err(e) => {
            eprintln!("deploy: failed to look up {}: {e}", body.name);
            return HttpResponse::InternalServerError().finish();
        }
    };
    let env_vars = if let Some(app) = existing_app {
        match merged_app_env_vars(&pool, project.id, app.id).await {
            Ok(env) => env,
            Err(e) => {
                eprintln!("deploy: failed to load env vars for {}: {e}", body.name);
                return HttpResponse::InternalServerError().finish();
            }
        }
    } else {
        match fetch_project_env_vars(&pool, project.id).await {
            Ok(env) => env
                .into_iter()
                .map(|(key, value)| format!("{key}={value}"))
                .collect(),
            Err(e) => {
                eprintln!(
                    "deploy: failed to load project env vars for {}: {e}",
                    body.name
                );
                return HttpResponse::InternalServerError().finish();
            }
        }
    };
    match scheduler
        .deploy(
            &pool,
            project.id,
            &project.network_name,
            &body.name,
            &body.image,
            body.port,
            env_vars,
            body.base_domain.as_deref(),
        )
        .await
    {
        Ok(()) => HttpResponse::Ok().finish(),
        Err(DeployError::AppNotFound(message)) => HttpResponse::NotFound().body(message),
        Err(DeployError::PortRequired(message)) => {
            HttpResponse::UnprocessableEntity().body(message)
        }
        Err(DeployError::Other(message)) => {
            eprintln!("deploy: failed to deploy {}: {message}", body.name);
            HttpResponse::InternalServerError().body(message)
        }
    }
}

#[derive(Deserialize, utoipa::ToSchema)]
struct UpdateBody {
    image: String,
    port: Option<u16>,
    base_domain: Option<String>,
}

#[derive(Deserialize)]
struct ProjectAppPath {
    project: String,
    app_name: String,
}

async fn app_in_project_path(
    pool: &PgPool,
    project_name: &str,
    app_name: &str,
) -> Result<Option<registry::App>, Error> {
    let project = project_by_name(pool, project_name).await?;
    Registry::get_in_project(pool, project.id, app_name)
        .await
        .map_err(error::ErrorInternalServerError)
}

#[utoipa::path(
    post,
    path = "/app/{app_name}/update",
    params(("app_name" = String, Path, description = "Application name")),
    request_body = UpdateBody,
    responses(
        (status = 200, description = "Rolling update completed"),
        (status = 404, description = "Application not found"),
        (status = 422, description = "Internal application port is required"),
        (status = 500, description = "Rolling update failed"),
    ),
    tag = "apps"
)]
#[post("/app/{app_name}/update")]
async fn update_app(
    scheduler: web::Data<Scheduler>,
    pool: web::Data<PgPool>,
    path: web::Path<String>,
    body: web::Json<UpdateBody>,
) -> impl Responder {
    let project = match default_project(&pool).await {
        Ok(project) => project,
        Err(e) => return e.error_response(),
    };
    let app_name = path.into_inner();
    let app = match Registry::get_in_project(&pool, project.id, &app_name).await {
        Ok(Some(app)) => app,
        Ok(None) => return HttpResponse::NotFound().finish(),
        Err(e) => {
            eprintln!("registry: get failed for {app_name}: {e}");
            return HttpResponse::InternalServerError().finish();
        }
    };
    let env_vars = match merged_app_env_vars(&pool, project.id, app.id).await {
        Ok(env_vars) => env_vars,
        Err(e) => {
            eprintln!("registry: failed to merge env vars for {app_name}: {e}");
            return HttpResponse::InternalServerError().finish();
        }
    };
    match scheduler
        .rolling_update(
            &pool,
            project.id,
            &project.network_name,
            &app_name,
            &body.image,
            body.port,
            env_vars,
            body.base_domain.as_deref(),
        )
        .await
    {
        Ok(()) => HttpResponse::Ok().finish(),
        Err(DeployError::AppNotFound(message)) => HttpResponse::NotFound().body(message),
        Err(DeployError::PortRequired(message)) => {
            HttpResponse::UnprocessableEntity().body(message)
        }
        Err(DeployError::Other(message)) => {
            eprintln!("update: failed to update {app_name}: {message}");
            HttpResponse::InternalServerError().body(message)
        }
    }
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
    let app = match Registry::get(&pool, &app_name).await {
        Ok(Some(app)) => app,
        Ok(None) => return HttpResponse::NotFound().finish(),
        Err(e) => {
            eprintln!("registry: get failed for {app_name}: {e}");
            return HttpResponse::InternalServerError().finish();
        }
    };
    match Registry::list_processes(&pool, app.id).await {
        Ok(processes) if !processes.is_empty() => {
            for process in processes {
                scheduler
                    .stop(&app_process_container_name(
                        app.project_id,
                        &app_name,
                        &process.name,
                    ))
                    .await;
                if let Err(e) = Registry::update_process_status(&pool, process.id, "stopped").await
                {
                    eprintln!(
                        "registry: failed to update process {} for {app_name}: {e}",
                        process.name
                    );
                }
            }
        }
        Ok(_) => {
            scheduler
                .stop(&app_container_name(app.project_id, &app_name))
                .await
        }
        Err(e) => {
            eprintln!("registry: failed to list processes for {app_name}: {e}");
            return HttpResponse::InternalServerError().finish();
        }
    }

    if let Err(e) = Registry::update_status(&pool, app.project_id, &app_name, "stopped").await {
        eprintln!("registry: failed to update status for {app_name}: {e}");
    }
    HttpResponse::Ok().finish()
}

#[post("/project/{project}/app/{app_name}/stop")]
async fn stop_project_app(
    scheduler: web::Data<Scheduler>,
    pool: web::Data<PgPool>,
    path: web::Path<ProjectAppPath>,
) -> impl Responder {
    let path = path.into_inner();
    let app_name = path.app_name;
    let app = match app_in_project_path(&pool, &path.project, &app_name).await {
        Ok(Some(app)) => app,
        Ok(None) => return HttpResponse::NotFound().finish(),
        Err(e) => return e.error_response(),
    };

    match Registry::list_processes(&pool, app.id).await {
        Ok(processes) if !processes.is_empty() => {
            for process in processes {
                scheduler
                    .stop(&app_process_container_name(
                        app.project_id,
                        &app_name,
                        &process.name,
                    ))
                    .await;
                if let Err(e) = Registry::update_process_status(&pool, process.id, "stopped").await
                {
                    eprintln!(
                        "registry: failed to update process {} for {app_name}: {e}",
                        process.name
                    );
                }
            }
        }
        Ok(_) => {
            scheduler
                .stop(&app_container_name(app.project_id, &app_name))
                .await
        }
        Err(e) => {
            eprintln!("registry: failed to list processes for {app_name}: {e}");
            return HttpResponse::InternalServerError().finish();
        }
    }

    if let Err(e) = Registry::update_status(&pool, app.project_id, &app_name, "stopped").await {
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

    let processes = match Registry::list_processes(&pool, app.id).await {
        Ok(processes) => processes,
        Err(e) => {
            eprintln!("registry: failed to list processes for {app_name}: {e}");
            return HttpResponse::InternalServerError().finish();
        }
    };
    let project = match Registry::get_project_by_id(&pool, app.project_id).await {
        Ok(Some(project)) => project,
        Ok(None) => return HttpResponse::InternalServerError().body("project not found"),
        Err(e) => {
            eprintln!("registry: failed to load project for {app_name}: {e}");
            return HttpResponse::InternalServerError().finish();
        }
    };

    if !processes.is_empty() {
        for process in processes {
            let Some(image) = process.image_id.as_deref().filter(|s| !s.is_empty()) else {
                return HttpResponse::BadRequest().json(json!({
                    "error": format!("process '{}' has no image_id", process.name)
                }));
            };
            let process_type = process.process_type.clone();

            scheduler
                .stop(&app_process_container_name(
                    app.project_id,
                    &app_name,
                    &process.name,
                ))
                .await;

            let env_vars = match merged_app_env_vars(&pool, app.project_id, app.id).await {
                Ok(env_vars) => env_vars,
                Err(e) => {
                    eprintln!("registry: failed to load env vars for {app_name}: {e}");
                    return HttpResponse::InternalServerError().finish();
                }
            };

            match scheduler
                .start_process(
                    app.project_id,
                    &project.network_name,
                    &app_name,
                    &process.name,
                    &process_type,
                    image,
                    process.internal_port.map(|p| p as u16),
                    app.base_domain.as_deref(),
                    process.public_host.as_deref(),
                    env_vars,
                )
                .await
            {
                Ok(started) => {
                    if let Err(e) = Registry::update_process_running(
                        &pool,
                        process.id,
                        image,
                        &started.container_id,
                        started.internal_port.map(|p| p as i32),
                        started.host_port.map(|p| p as i32),
                    )
                    .await
                    {
                        eprintln!(
                            "registry: failed to update process {} for {app_name}: {e}",
                            process.name
                        );
                        return HttpResponse::InternalServerError().finish();
                    }
                }
                Err(e) => {
                    eprintln!(
                        "docker: failed to restart process {} for {app_name}: {e}",
                        process.name
                    );
                    let _ = Registry::update_process_status(&pool, process.id, "failed").await;
                    return HttpResponse::InternalServerError().body(e.to_string());
                }
            }
        }

        if let Err(e) = Registry::update_status(&pool, app.project_id, &app_name, "running").await {
            eprintln!("registry: failed to update status for {app_name}: {e}");
        }

        return HttpResponse::Ok().finish();
    }

    let image = match app.image_id.as_deref().filter(|s| !s.is_empty()) {
        Some(img) => img.to_string(),
        None => {
            if let Err(e) =
                Registry::update_status(&pool, app.project_id, &app_name, "running").await
            {
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
    let env_vars = match merged_app_env_vars(&pool, app.project_id, app.id).await {
        Ok(env_vars) => env_vars,
        Err(e) => {
            eprintln!("registry: failed to merge env vars for {app_name}: {e}");
            return HttpResponse::InternalServerError().finish();
        }
    };

    if let Err(e) = scheduler
        .redeploy(
            &pool,
            app.project_id,
            &project.network_name,
            &app_name,
            &image,
            internal_port,
            host_port,
            env_vars,
            app.base_domain.as_deref(),
        )
        .await
    {
        eprintln!("docker: failed to restart {app_name}: {e}");
        return HttpResponse::InternalServerError().body(e.to_string());
    }
    HttpResponse::Ok().finish()
}

#[post("/project/{project}/app/{app_name}/restart")]
async fn restart_project_app(
    scheduler: web::Data<Scheduler>,
    pool: web::Data<PgPool>,
    path: web::Path<ProjectAppPath>,
) -> impl Responder {
    let path = path.into_inner();
    let app_name = path.app_name;
    let app = match app_in_project_path(&pool, &path.project, &app_name).await {
        Ok(Some(app)) => app,
        Ok(None) => return HttpResponse::NotFound().finish(),
        Err(e) => return e.error_response(),
    };

    let processes = match Registry::list_processes(&pool, app.id).await {
        Ok(processes) => processes,
        Err(e) => {
            eprintln!("registry: failed to list processes for {app_name}: {e}");
            return HttpResponse::InternalServerError().finish();
        }
    };
    let project = match Registry::get_project_by_id(&pool, app.project_id).await {
        Ok(Some(project)) => project,
        Ok(None) => return HttpResponse::InternalServerError().body("project not found"),
        Err(e) => {
            eprintln!("registry: failed to load project for {app_name}: {e}");
            return HttpResponse::InternalServerError().finish();
        }
    };

    if !processes.is_empty() {
        for process in processes {
            let Some(image) = process.image_id.as_deref().filter(|s| !s.is_empty()) else {
                return HttpResponse::BadRequest().json(json!({
                    "error": format!("process '{}' has no image_id", process.name)
                }));
            };
            let process_type = process.process_type.clone();

            scheduler
                .stop(&app_process_container_name(
                    app.project_id,
                    &app_name,
                    &process.name,
                ))
                .await;

            let env_vars = match merged_app_env_vars(&pool, app.project_id, app.id).await {
                Ok(env_vars) => env_vars,
                Err(e) => {
                    eprintln!("registry: failed to load env vars for {app_name}: {e}");
                    return HttpResponse::InternalServerError().finish();
                }
            };

            match scheduler
                .start_process(
                    app.project_id,
                    &project.network_name,
                    &app_name,
                    &process.name,
                    &process_type,
                    image,
                    process.internal_port.map(|p| p as u16),
                    app.base_domain.as_deref(),
                    process.public_host.as_deref(),
                    env_vars,
                )
                .await
            {
                Ok(started) => {
                    if let Err(e) = Registry::update_process_running(
                        &pool,
                        process.id,
                        image,
                        &started.container_id,
                        started.internal_port.map(|p| p as i32),
                        started.host_port.map(|p| p as i32),
                    )
                    .await
                    {
                        eprintln!(
                            "registry: failed to update process {} for {app_name}: {e}",
                            process.name
                        );
                        return HttpResponse::InternalServerError().finish();
                    }
                }
                Err(e) => {
                    eprintln!(
                        "docker: failed to restart process {} for {app_name}: {e}",
                        process.name
                    );
                    let _ = Registry::update_process_status(&pool, process.id, "failed").await;
                    return HttpResponse::InternalServerError().body(e.to_string());
                }
            }
        }

        if let Err(e) = Registry::update_status(&pool, app.project_id, &app_name, "running").await {
            eprintln!("registry: failed to update status for {app_name}: {e}");
        }

        return HttpResponse::Ok().finish();
    }

    let image = match app.image_id.as_deref().filter(|s| !s.is_empty()) {
        Some(img) => img.to_string(),
        None => {
            if let Err(e) =
                Registry::update_status(&pool, app.project_id, &app_name, "running").await
            {
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
    let env_vars = match merged_app_env_vars(&pool, app.project_id, app.id).await {
        Ok(env_vars) => env_vars,
        Err(e) => {
            eprintln!("registry: failed to merge env vars for {app_name}: {e}");
            return HttpResponse::InternalServerError().finish();
        }
    };

    if let Err(e) = scheduler
        .redeploy(
            &pool,
            app.project_id,
            &project.network_name,
            &app_name,
            &image,
            internal_port,
            host_port,
            env_vars,
            app.base_domain.as_deref(),
        )
        .await
    {
        eprintln!("docker: failed to restart {app_name}: {e}");
        return HttpResponse::InternalServerError().body(e.to_string());
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
    let app = match Registry::get(&pool, &app_name).await {
        Ok(Some(app)) => app,
        Ok(None) => return HttpResponse::NotFound().finish(),
        Err(e) => {
            eprintln!("registry: get failed for {app_name}: {e}");
            return HttpResponse::InternalServerError().finish();
        }
    };

    let processes = match Registry::list_processes(&pool, app.id).await {
        Ok(p) => p,
        Err(e) => {
            eprintln!("registry: list_processes failed for {app_name}: {e}");
            return HttpResponse::InternalServerError().finish();
        }
    };

    let docker_status = if processes.is_empty() {
        scheduler
            .inspect(&app_container_name(app.project_id, &app_name))
            .await
    } else {
        // For multi-process apps, report the worst status across all process containers.
        let mut worst = "running".to_string();
        for process in &processes {
            let s = scheduler
                .inspect(&app_process_container_name(
                    app.project_id,
                    &app_name,
                    &process.name,
                ))
                .await;
            if s == "exited" || s == "unknown" {
                worst = s;
                break;
            } else if s != "running" {
                worst = s;
            }
        }
        worst
    };

    let status = if docker_status == "unknown" {
        app.status.as_deref().unwrap_or("unknown").to_string()
    } else {
        docker_status
    };
    HttpResponse::Ok().body(status)
}

#[get("/project/{project}/app/{app_name}/status")]
async fn status_project_app(
    scheduler: web::Data<Scheduler>,
    pool: web::Data<PgPool>,
    path: web::Path<ProjectAppPath>,
) -> impl Responder {
    let path = path.into_inner();
    let app_name = path.app_name;
    let app = match app_in_project_path(&pool, &path.project, &app_name).await {
        Ok(Some(app)) => app,
        Ok(None) => return HttpResponse::NotFound().finish(),
        Err(e) => return e.error_response(),
    };

    let processes = match Registry::list_processes(&pool, app.id).await {
        Ok(p) => p,
        Err(e) => {
            eprintln!("registry: list_processes failed for {app_name}: {e}");
            return HttpResponse::InternalServerError().finish();
        }
    };

    let docker_status = if processes.is_empty() {
        scheduler
            .inspect(&app_container_name(app.project_id, &app_name))
            .await
    } else {
        let mut worst = "running".to_string();
        for process in &processes {
            let s = scheduler
                .inspect(&app_process_container_name(
                    app.project_id,
                    &app_name,
                    &process.name,
                ))
                .await;
            if s == "exited" || s == "unknown" {
                worst = s;
                break;
            } else if s != "running" {
                worst = s;
            }
        }
        worst
    };

    let status = if docker_status == "unknown" {
        app.status.as_deref().unwrap_or("unknown").to_string()
    } else {
        docker_status
    };
    HttpResponse::Ok().body(status)
}

#[utoipa::path(
    delete,
    path = "/app/{app_name}",
    params(("app_name" = String, Path, description = "Application name")),
    responses(
        (status = 204, description = "Application deleted"),
        (status = 404, description = "Application not found"),
        (status = 500, description = "Internal server error"),
    ),
    tag = "apps"
)]
#[delete("/app/{app_name}")]
async fn delete_app(
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

    match Registry::list_processes(&pool, app.id).await {
        Ok(processes) if !processes.is_empty() => {
            for process in processes {
                scheduler
                    .stop(&app_process_container_name(
                        app.project_id,
                        &app_name,
                        &process.name,
                    ))
                    .await;
            }
        }
        Ok(_) => {
            scheduler
                .stop(&app_container_name(app.project_id, &app_name))
                .await
        }
        Err(e) => {
            eprintln!("registry: failed to list processes for {app_name}: {e}");
            return HttpResponse::InternalServerError().finish();
        }
    }

    let mut tx = match pool.begin().await {
        Ok(tx) => tx,
        Err(e) => {
            eprintln!("registry: failed to begin transaction for {app_name}: {e}");
            return HttpResponse::InternalServerError().finish();
        }
    };

    if let Err(e) = sqlx::query!(
        "DELETE FROM application_services WHERE application_id = $1",
        app.id,
    )
    .execute(&mut *tx)
    .await
    {
        eprintln!("registry: failed to delete application_services for {app_name}: {e}");
        return HttpResponse::InternalServerError().finish();
    }

    if let Err(e) = sqlx::query!("DELETE FROM applications WHERE id = $1", app.id)
        .execute(&mut *tx)
        .await
    {
        eprintln!("registry: failed to delete application {app_name}: {e}");
        return HttpResponse::InternalServerError().finish();
    }

    if let Err(e) = tx.commit().await {
        eprintln!("registry: failed to commit delete for {app_name}: {e}");
        return HttpResponse::InternalServerError().finish();
    }

    HttpResponse::NoContent().finish()
}

#[delete("/project/{project}/app/{app_name}")]
async fn delete_project_app(
    scheduler: web::Data<Scheduler>,
    pool: web::Data<PgPool>,
    path: web::Path<ProjectAppPath>,
) -> impl Responder {
    let path = path.into_inner();
    let app_name = path.app_name;
    let app = match app_in_project_path(&pool, &path.project, &app_name).await {
        Ok(Some(app)) => app,
        Ok(None) => return HttpResponse::NotFound().finish(),
        Err(e) => return e.error_response(),
    };

    match Registry::list_processes(&pool, app.id).await {
        Ok(processes) if !processes.is_empty() => {
            for process in processes {
                scheduler
                    .stop(&app_process_container_name(
                        app.project_id,
                        &app_name,
                        &process.name,
                    ))
                    .await;
            }
        }
        Ok(_) => {
            scheduler
                .stop(&app_container_name(app.project_id, &app_name))
                .await
        }
        Err(e) => {
            eprintln!("registry: failed to list processes for {app_name}: {e}");
            return HttpResponse::InternalServerError().finish();
        }
    }

    let mut tx = match pool.begin().await {
        Ok(tx) => tx,
        Err(e) => {
            eprintln!("registry: failed to begin transaction for {app_name}: {e}");
            return HttpResponse::InternalServerError().finish();
        }
    };

    if let Err(e) = sqlx::query!(
        "DELETE FROM application_services WHERE application_id = $1",
        app.id,
    )
    .execute(&mut *tx)
    .await
    {
        eprintln!("registry: failed to delete application_services for {app_name}: {e}");
        return HttpResponse::InternalServerError().finish();
    }

    if let Err(e) = sqlx::query!("DELETE FROM applications WHERE id = $1", app.id)
        .execute(&mut *tx)
        .await
    {
        eprintln!("registry: failed to delete application {app_name}: {e}");
        return HttpResponse::InternalServerError().finish();
    }

    if let Err(e) = tx.commit().await {
        eprintln!("registry: failed to commit delete for {app_name}: {e}");
        return HttpResponse::InternalServerError().finish();
    }

    HttpResponse::NoContent().finish()
}

#[derive(Deserialize)]
struct LogsQuery {
    tail: Option<usize>,
    process: Option<String>,
}

#[utoipa::path(
    get,
    path = "/app/{app_name}/logs",
    params(
        ("app_name" = String, Path, description = "Application name"),
        ("tail" = Option<usize>, Query, description = "Number of lines to return from the end (default: all)"),
    ),
    responses(
        (status = 200, description = "Container logs", content_type = "text/plain"),
        (status = 404, description = "Application not found"),
        (status = 500, description = "Internal server error"),
    ),
    tag = "apps"
)]
#[get("/app/{app_name}/logs")]
async fn logs_app(
    scheduler: web::Data<Scheduler>,
    pool: web::Data<PgPool>,
    path: web::Path<String>,
    query: web::Query<LogsQuery>,
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

    let container_name = match Registry::list_processes(&pool, app.id).await {
        Ok(processes) if !processes.is_empty() => {
            let selected_name = match query.process.as_deref() {
                Some(name) => processes
                    .iter()
                    .find(|process| process.name == name)
                    .map(|process| process.name.clone()),
                None => processes
                    .iter()
                    .find(|process| process.process_type == ProcessType::Web)
                    .or_else(|| processes.first())
                    .map(|process| process.name.clone()),
            };

            let Some(process_name) = selected_name else {
                return HttpResponse::NotFound().body("process not found");
            };
            app_process_container_name(app.project_id, &app_name, &process_name)
        }
        Ok(_) => app_container_name(app.project_id, &app_name),
        Err(e) => {
            eprintln!("registry: failed to list processes for {app_name}: {e}");
            return HttpResponse::InternalServerError().finish();
        }
    };

    match scheduler.get_logs(&container_name, query.tail).await {
        Ok(logs) => HttpResponse::Ok().content_type("text/plain").body(logs),
        Err(e) => {
            eprintln!("docker: failed to get logs for {container_name}: {e}");
            HttpResponse::InternalServerError().finish()
        }
    }
}

#[get("/project/{project}/app/{app_name}/logs")]
async fn logs_project_app(
    scheduler: web::Data<Scheduler>,
    pool: web::Data<PgPool>,
    path: web::Path<ProjectAppPath>,
    query: web::Query<LogsQuery>,
) -> impl Responder {
    let path = path.into_inner();
    let app_name = path.app_name;
    let app = match app_in_project_path(&pool, &path.project, &app_name).await {
        Ok(Some(app)) => app,
        Ok(None) => return HttpResponse::NotFound().finish(),
        Err(e) => return e.error_response(),
    };

    let container_name = match Registry::list_processes(&pool, app.id).await {
        Ok(processes) if !processes.is_empty() => {
            let selected_name = match query.process.as_deref() {
                Some(name) => processes
                    .iter()
                    .find(|process| process.name == name)
                    .map(|process| process.name.clone()),
                None => processes
                    .iter()
                    .find(|process| process.process_type == ProcessType::Web)
                    .or_else(|| processes.first())
                    .map(|process| process.name.clone()),
            };

            let Some(process_name) = selected_name else {
                return HttpResponse::NotFound().body("process not found");
            };
            app_process_container_name(app.project_id, &app_name, &process_name)
        }
        Ok(_) => app_container_name(app.project_id, &app_name),
        Err(e) => {
            eprintln!("registry: failed to list processes for {app_name}: {e}");
            return HttpResponse::InternalServerError().finish();
        }
    };

    match scheduler.get_logs(&container_name, query.tail).await {
        Ok(logs) => HttpResponse::Ok().content_type("text/plain").body(logs),
        Err(e) => {
            eprintln!("docker: failed to get logs for {container_name}: {e}");
            HttpResponse::InternalServerError().finish()
        }
    }
}

#[utoipa::path(
    get,
    path = "/resource/{id}/logs",
    params(
        ("id" = String, Path, description = "Resource UUID"),
        ("tail" = Option<usize>, Query, description = "Number of lines to return from the end (default: all)"),
    ),
    responses(
        (status = 200, description = "Container logs", content_type = "text/plain"),
        (status = 400, description = "Invalid UUID"),
        (status = 404, description = "Resource not found"),
        (status = 500, description = "Internal server error"),
    ),
    tag = "resources"
)]
#[get("/resource/{id}/logs")]
async fn logs_resource(
    scheduler: web::Data<Scheduler>,
    pool: web::Data<PgPool>,
    id: web::Path<String>,
    query: web::Query<LogsQuery>,
) -> Result<impl Responder, Error> {
    let uuid = Uuid::parse_str(&id).map_err(|_| error::ErrorBadRequest("Invalid resource id"))?;

    sqlx::query!("SELECT id FROM services WHERE id = $1", uuid)
        .fetch_optional(pool.get_ref())
        .await
        .map_err(error::ErrorInternalServerError)?
        .ok_or_else(|| error::ErrorNotFound("Resource not found"))?;

    let logs = scheduler
        .get_logs(&uuid.to_string(), query.tail)
        .await
        .map_err(|e| error::ErrorInternalServerError(format!("Failed to get logs: {e}")))?;

    Ok(HttpResponse::Ok().content_type("text/plain").body(logs))
}

async fn inject_service_env_into_app(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    app_uuid: Uuid,
    service_name: &str,
    service_host: &str,
    service_port: u16,
    service_env: &HashMap<String, String>,
    connection_profile: Option<&str>,
) -> Result<(), sqlx::Error> {
    let mut connection_env = service_env.clone();
    if let Some(connection_profile) = connection_profile {
        connection_env.insert(
            "PAASTECH_CONNECTION_PROFILE".to_string(),
            connection_profile.to_string(),
        );
    }
    let env_vars =
        connection_env_vars_for_service(service_name, service_host, service_port, &connection_env)
            .map_err(sqlx::Error::Protocol)?;

    if service_name == "postgres" {
        sqlx::query(
            "DELETE FROM application_env_vars WHERE application_id = $1 AND key IN ('DATABASE_ASYNC_URL', 'DATABASE_SYNC_URL')",
        )
        .bind(app_uuid)
        .execute(&mut **tx)
        .await?;
    }

    for (key, value) in env_vars {
        sqlx::query!(
            "INSERT INTO application_env_vars (application_id, key, value) VALUES ($1, $2, $3) ON CONFLICT (application_id, key) DO UPDATE SET value = EXCLUDED.value",
            app_uuid,
            key,
            value,
        )
        .execute(&mut **tx)
        .await?;
    }
    Ok(())
}

fn attachment_from_legacy_app_id(
    application_id: &str,
    connection_profile: Option<&str>,
    service_name: &str,
) -> Result<ResourceAttachment, Error> {
    validate_connection_profile_for_service(service_name, connection_profile)
        .map_err(error::ErrorBadRequest)?;

    Ok(ResourceAttachment {
        application_id: application_id.to_string(),
        connection_profile: connection_profile.unwrap_or_default().to_string(),
    })
}

fn resource_attachments_from_payload(
    service_name: &str,
    application_id: Option<&str>,
    connection_profile: Option<&str>,
    attachments: Option<&Vec<ResourceAttachment>>,
) -> Result<Vec<ResourceAttachment>, Error> {
    if let Some(attachments) = attachments {
        for attachment in attachments {
            validate_connection_profile_for_service(
                service_name,
                Some(attachment.connection_profile.as_str()),
            )
            .map_err(error::ErrorBadRequest)?;
        }
        return Ok(attachments
            .iter()
            .map(|attachment| ResourceAttachment {
                application_id: attachment.application_id.clone(),
                connection_profile: attachment.connection_profile.clone(),
            })
            .collect());
    }

    match application_id.filter(|id| !id.is_empty()) {
        Some(application_id) => Ok(vec![attachment_from_legacy_app_id(
            application_id,
            connection_profile,
            service_name,
        )?]),
        None => Ok(Vec::new()),
    }
}

async fn ensure_attached_service_network(
    pool: &PgPool,
    scheduler: &Scheduler,
    service_id: Uuid,
    app_ids: &[Uuid],
) -> Result<(), Error> {
    for app_id in app_ids {
        let Some(app) = sqlx::query_as::<_, registry::App>(
            r#"
            SELECT id, project_id, name, image_id, container_id, internal_port, port, status, base_domain, created_at
            FROM applications
            WHERE id = $1
            "#,
        )
        .bind(app_id)
        .fetch_optional(pool)
        .await
        .map_err(error::ErrorInternalServerError)?
        else {
            continue;
        };
        let project = Registry::get_project_by_id(pool, app.project_id)
            .await
            .map_err(error::ErrorInternalServerError)?
            .ok_or_else(|| error::ErrorInternalServerError("Project not found"))?;

        scheduler
            .ensure_container_on_network(&project.network_name, &service_id.to_string())
            .await
            .map_err(|e| {
                error::ErrorInternalServerError(format!(
                    "Failed to connect service {service_id} to {}: {e}",
                    project.network_name
                ))
            })?;

        let processes = Registry::list_processes(pool, app.id)
            .await
            .map_err(error::ErrorInternalServerError)?;

        if processes.is_empty() {
            if app.container_id.as_deref().is_some_and(|id| !id.is_empty()) {
                scheduler
                    .ensure_container_on_network(
                        &project.network_name,
                        &app_container_name(app.project_id, &app.name),
                    )
                    .await
                    .map_err(|e| {
                        error::ErrorInternalServerError(format!(
                            "Failed to connect {} to project network: {e}",
                            app.name
                        ))
                    })?;
            }
            continue;
        }

        for process in processes {
            if process
                .container_id
                .as_deref()
                .is_some_and(|id| !id.is_empty())
            {
                let container_name =
                    app_process_container_name(app.project_id, &app.name, &process.name);
                scheduler
                    .ensure_container_on_network(&project.network_name, &container_name)
                    .await
                    .map_err(|e| {
                        error::ErrorInternalServerError(format!(
                            "Failed to connect {container_name} to {}: {e}",
                            project.network_name
                        ))
                    })?;
            }
        }
    }

    Ok(())
}

#[utoipa::path(
    get,
    path = "/app/{app_name}/env",
    params(("app_name" = String, Path, description = "Application name")),
    responses(
        (status = 200, description = "Environment variables as key-value map"),
        (status = 404, description = "Application not found"),
        (status = 500, description = "Internal server error"),
    ),
    tag = "apps"
)]
#[get("/app/{app_name}/env")]
async fn get_app_env(pool: web::Data<PgPool>, path: web::Path<String>) -> impl Responder {
    let app_name = path.into_inner();
    let app = match Registry::get(&pool, &app_name).await {
        Ok(Some(app)) => app,
        Ok(None) => return HttpResponse::NotFound().finish(),
        Err(e) => {
            eprintln!("registry: get failed for {app_name}: {e}");
            return HttpResponse::InternalServerError().finish();
        }
    };

    let rows = match sqlx::query!(
        "SELECT key, value FROM application_env_vars WHERE application_id = $1 ORDER BY key",
        app.id,
    )
    .fetch_all(pool.get_ref())
    .await
    {
        Ok(rows) => rows,
        Err(e) => {
            eprintln!("registry: failed to fetch env vars for {app_name}: {e}");
            return HttpResponse::InternalServerError().finish();
        }
    };

    let env_map: HashMap<String, String> = rows.into_iter().map(|r| (r.key, r.value)).collect();
    HttpResponse::Ok().json(env_map)
}

#[get("/project/{project}/app/{app_name}/env")]
async fn get_project_app_env(
    pool: web::Data<PgPool>,
    path: web::Path<ProjectAppPath>,
) -> impl Responder {
    let path = path.into_inner();
    let app_name = path.app_name;
    let app = match app_in_project_path(&pool, &path.project, &app_name).await {
        Ok(Some(app)) => app,
        Ok(None) => return HttpResponse::NotFound().finish(),
        Err(e) => return e.error_response(),
    };

    let rows = match sqlx::query!(
        "SELECT key, value FROM application_env_vars WHERE application_id = $1 ORDER BY key",
        app.id,
    )
    .fetch_all(pool.get_ref())
    .await
    {
        Ok(rows) => rows,
        Err(e) => {
            eprintln!("registry: failed to fetch env vars for {app_name}: {e}");
            return HttpResponse::InternalServerError().finish();
        }
    };

    let env_map: HashMap<String, String> = rows.into_iter().map(|r| (r.key, r.value)).collect();
    HttpResponse::Ok().json(env_map)
}

#[derive(Deserialize)]
struct EnvSetPayload {
    key: String,
    value: String,
}

#[utoipa::path(
    post,
    path = "/app/{app_name}/env",
    params(("app_name" = String, Path, description = "Application name")),
    request_body(
        content_type = "application/json",
        description = "Single environment variable to set or update: {\"key\": \"K\", \"value\": \"V\"}"
    ),
    responses(
        (status = 200, description = "Environment variable set"),
        (status = 404, description = "Application not found"),
        (status = 500, description = "Internal server error"),
    ),
    tag = "apps"
)]
#[post("/app/{app_name}/env")]
async fn set_app_env(
    pool: web::Data<PgPool>,
    path: web::Path<String>,
    payload: web::Json<EnvSetPayload>,
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

    let result = sqlx::query!(
        "INSERT INTO application_env_vars (application_id, key, value) VALUES ($1, $2, $3) \
         ON CONFLICT (application_id, key) DO UPDATE SET value = EXCLUDED.value",
        app.id,
        payload.key,
        payload.value,
    )
    .execute(pool.get_ref())
    .await;

    match result {
        Ok(_) => HttpResponse::Ok().finish(),
        Err(e) => {
            eprintln!("registry: failed to set env var for {app_name}: {e}");
            HttpResponse::InternalServerError().finish()
        }
    }
}

#[post("/project/{project}/app/{app_name}/env")]
async fn set_project_app_env(
    pool: web::Data<PgPool>,
    path: web::Path<ProjectAppPath>,
    payload: web::Json<EnvSetPayload>,
) -> impl Responder {
    let path = path.into_inner();
    let app_name = path.app_name;
    let app = match app_in_project_path(&pool, &path.project, &app_name).await {
        Ok(Some(app)) => app,
        Ok(None) => return HttpResponse::NotFound().finish(),
        Err(e) => return e.error_response(),
    };

    let result = sqlx::query!(
        "INSERT INTO application_env_vars (application_id, key, value) VALUES ($1, $2, $3) \
         ON CONFLICT (application_id, key) DO UPDATE SET value = EXCLUDED.value",
        app.id,
        payload.key,
        payload.value,
    )
    .execute(pool.get_ref())
    .await;

    match result {
        Ok(_) => HttpResponse::Ok().finish(),
        Err(e) => {
            eprintln!("registry: failed to set env var for {app_name}: {e}");
            HttpResponse::InternalServerError().finish()
        }
    }
}

#[utoipa::path(
    put,
    path = "/app/{app_name}/env",
    params(("app_name" = String, Path, description = "Application name")),
    request_body(
        content_type = "application/json",
        description = "Environment variables as key-value map (replaces all existing variables)"
    ),
    responses(
        (status = 200, description = "Environment variables updated"),
        (status = 404, description = "Application not found"),
        (status = 500, description = "Internal server error"),
    ),
    tag = "apps"
)]
#[put("/app/{app_name}/env")]
async fn update_app_env(
    pool: web::Data<PgPool>,
    path: web::Path<String>,
    payload: web::Json<HashMap<String, String>>,
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

    let mut tx = match pool.begin().await {
        Ok(tx) => tx,
        Err(e) => {
            eprintln!("registry: failed to begin transaction for {app_name}: {e}");
            return HttpResponse::InternalServerError().finish();
        }
    };

    if let Err(e) = sqlx::query!(
        "DELETE FROM application_env_vars WHERE application_id = $1",
        app.id,
    )
    .execute(&mut *tx)
    .await
    {
        eprintln!("registry: failed to delete env vars for {app_name}: {e}");
        return HttpResponse::InternalServerError().finish();
    }

    for (key, value) in payload.iter() {
        if let Err(e) = sqlx::query!(
            "INSERT INTO application_env_vars (application_id, key, value) VALUES ($1, $2, $3)",
            app.id,
            key,
            value,
        )
        .execute(&mut *tx)
        .await
        {
            eprintln!("registry: failed to insert env var for {app_name}: {e}");
            return HttpResponse::InternalServerError().finish();
        }
    }

    if let Err(e) = tx.commit().await {
        eprintln!("registry: failed to commit env vars for {app_name}: {e}");
        return HttpResponse::InternalServerError().finish();
    }

    HttpResponse::Ok()
        .body("Environment variables updated. Restart the application to apply changes.")
}

#[put("/project/{project}/app/{app_name}/env")]
async fn update_project_app_env(
    pool: web::Data<PgPool>,
    path: web::Path<ProjectAppPath>,
    payload: web::Json<HashMap<String, String>>,
) -> impl Responder {
    let path = path.into_inner();
    let app_name = path.app_name;
    let app = match app_in_project_path(&pool, &path.project, &app_name).await {
        Ok(Some(app)) => app,
        Ok(None) => return HttpResponse::NotFound().finish(),
        Err(e) => return e.error_response(),
    };

    let mut tx = match pool.begin().await {
        Ok(tx) => tx,
        Err(e) => {
            eprintln!("registry: failed to begin transaction for {app_name}: {e}");
            return HttpResponse::InternalServerError().finish();
        }
    };

    if let Err(e) = sqlx::query!(
        "DELETE FROM application_env_vars WHERE application_id = $1",
        app.id,
    )
    .execute(&mut *tx)
    .await
    {
        eprintln!("registry: failed to delete env vars for {app_name}: {e}");
        return HttpResponse::InternalServerError().finish();
    }

    for (key, value) in payload.iter() {
        if let Err(e) = sqlx::query!(
            "INSERT INTO application_env_vars (application_id, key, value) VALUES ($1, $2, $3)",
            app.id,
            key,
            value,
        )
        .execute(&mut *tx)
        .await
        {
            eprintln!("registry: failed to insert env var for {app_name}: {e}");
            return HttpResponse::InternalServerError().finish();
        }
    }

    if let Err(e) = tx.commit().await {
        eprintln!("registry: failed to commit env vars for {app_name}: {e}");
        return HttpResponse::InternalServerError().finish();
    }

    HttpResponse::Ok()
        .body("Environment variables updated. Restart the application to apply changes.")
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
    let project = default_project(&pool).await?;
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
    let attachments = resource_attachments_from_payload(
        &payload.name,
        payload.application_id.as_deref(),
        payload.connection_profile.as_deref(),
        payload.attachments.as_ref(),
    )?;

    let mut tx = pool
        .begin()
        .await
        .map_err(error::ErrorInternalServerError)?;

    sqlx::query(
        "INSERT INTO services (id, project_id, display_name, name, version) VALUES ($1, $2, $3, $4, $5)",
    )
    .bind(id)
    .bind(project.id)
    .bind(&payload.display_name)
    .bind(&payload.name)
    .bind(&payload.version)
    .execute(&mut *tx)
    .await
    .map_err(error::ErrorInternalServerError)?;

    for attachment in &attachments {
        let app_uuid = Uuid::parse_str(&attachment.application_id)
            .map_err(|_| error::ErrorBadRequest("Invalid application_id"))?;
        sqlx::query(
            "INSERT INTO application_services (application_id, service_id, connection_profile) VALUES ($1, $2, $3)",
        )
        .bind(app_uuid)
        .bind(id)
        .bind(&attachment.connection_profile)
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
            &project.network_name,
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

    for attachment in &attachments {
        let app_uuid = Uuid::parse_str(&attachment.application_id)
            .map_err(|_| error::ErrorBadRequest("Invalid application_id"))?;
        let service_env_map: HashMap<String, String> = default_env.iter().cloned().collect();
        inject_service_env_into_app(
            &mut tx,
            app_uuid,
            &payload.name,
            &id.to_string(),
            container_port,
            &service_env_map,
            Some(attachment.connection_profile.as_str()),
        )
        .await
        .map_err(error::ErrorInternalServerError)?;
    }

    tx.commit().await.map_err(error::ErrorInternalServerError)?;

    for attachment in &attachments {
        let app_uuid = Uuid::parse_str(&attachment.application_id)
            .map_err(|_| error::ErrorBadRequest("Invalid application_id"))?;
        ensure_attached_service_network(pool.get_ref(), &scheduler, id, &[app_uuid]).await?;
    }

    Ok(HttpResponse::Created().json(Resource {
        id: id.to_string(),
        project_id: project.id.to_string(),
        display_name: payload.display_name.clone(),
        name: payload.name.clone(),
        version: payload.version.clone(),
        status: "running".to_string(),
        application_ids: attachments
            .iter()
            .map(|attachment| attachment.application_id.clone())
            .collect(),
    }))
}

#[post("/project/{project}/resource")]
async fn create_project_resource(
    pool: web::Data<PgPool>,
    client: web::Data<Client>,
    scheduler: web::Data<Scheduler>,
    path: web::Path<String>,
    payload: web::Json<CreateResourcePayload>,
) -> Result<impl Responder, Error> {
    let project = project_by_name(&pool, &path.into_inner()).await?;
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

    sqlx::query(
        "INSERT INTO services (id, project_id, display_name, name, version) VALUES ($1, $2, $3, $4, $5)",
    )
    .bind(id)
    .bind(project.id)
    .bind(&payload.display_name)
    .bind(&payload.name)
    .bind(&payload.version)
    .execute(&mut *tx)
    .await
    .map_err(error::ErrorInternalServerError)?;

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
            &project.network_name,
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
        project_id: project.id.to_string(),
        display_name: payload.display_name.clone(),
        name: payload.name.clone(),
        version: payload.version.clone(),
        status: "running".to_string(),
        application_ids: Vec::new(),
    }))
}

#[get("/project/{project}/resource")]
async fn get_project_resources(
    pool: web::Data<PgPool>,
    path: web::Path<String>,
) -> Result<impl Responder, Error> {
    let project = project_by_name(&pool, &path.into_inner()).await?;
    let resources = sqlx::query_as::<_, Resource>(
        r#"SELECT
            s.id::text as id,
            s.project_id::text as project_id,
            s.display_name,
            s.name,
            s.version,
            s.status,
            COALESCE(array_agg(aps.application_id::text) FILTER (WHERE aps.application_id IS NOT NULL), '{}') as application_ids
        FROM services s
        LEFT JOIN application_services aps ON s.id = aps.service_id
        WHERE s.project_id = $1
        GROUP BY s.id, s.project_id, s.display_name, s.name, s.version, s.status"#
    )
    .bind(project.id)
    .fetch_all(pool.get_ref())
    .await
    .map_err(error::ErrorInternalServerError)?;

    Ok(HttpResponse::Ok().json(resources))
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
    let project = default_project(&pool).await?;
    let resources = sqlx::query_as::<_, Resource>(
        r#"SELECT
            s.id::text as id,
            s.project_id::text as project_id,
            s.display_name,
            s.name,
            s.version,
            s.status,
            COALESCE(array_agg(aps.application_id::text) FILTER (WHERE aps.application_id IS NOT NULL), '{}') as application_ids
        FROM services s
        LEFT JOIN application_services aps ON s.id = aps.service_id
        WHERE s.project_id = $1
        GROUP BY s.id, s.project_id, s.display_name, s.name, s.version, s.status"#
    )
    .bind(project.id)
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

    let resource = sqlx::query_as::<_, Resource>(
        r#"SELECT
            s.id::text as id,
            s.project_id::text as project_id,
            s.display_name,
            s.name,
            s.version,
            s.status,
            COALESCE(array_agg(aps.application_id::text) FILTER (WHERE aps.application_id IS NOT NULL), '{}') as application_ids
        FROM services s
        LEFT JOIN application_services aps ON s.id = aps.service_id
        WHERE s.id = $1
        GROUP BY s.id, s.project_id, s.display_name, s.name, s.version, s.status"#
    )
    .bind(uuid)
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
    scheduler: web::Data<Scheduler>,
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

    let mut attached_app_uuids = Vec::new();

    if payload.application_ids.is_some() || payload.attachments.is_some() {
        let service_info = sqlx::query("SELECT name, container_id FROM services WHERE id = $1")
            .bind(uuid)
            .fetch_one(&mut *tx)
            .await
            .map_err(error::ErrorInternalServerError)?;

        let service_name: String = service_info
            .try_get("name")
            .map_err(error::ErrorInternalServerError)?;
        let container_id: Option<String> = service_info
            .try_get("container_id")
            .map_err(error::ErrorInternalServerError)?;

        let attachments = if let Some(attachments) = &payload.attachments {
            resource_attachments_from_payload(&service_name, None, None, Some(attachments))?
        } else if let Some(application_ids) = &payload.application_ids {
            application_ids
                .iter()
                .map(|application_id| {
                    attachment_from_legacy_app_id(application_id, None, &service_name)
                })
                .collect::<Result<Vec<_>, _>>()?
        } else {
            Vec::new()
        };

        let app_uuids: Vec<(Uuid, String)> = attachments
            .iter()
            .map(|attachment| {
                Ok((
                    Uuid::parse_str(&attachment.application_id)
                        .map_err(|_| error::ErrorBadRequest("Invalid application_id"))?,
                    attachment.connection_profile.clone(),
                ))
            })
            .collect::<Result<_, Error>>()?;
        attached_app_uuids = app_uuids.iter().map(|(app_uuid, _)| *app_uuid).collect();

        sqlx::query!(
            "DELETE FROM application_services WHERE service_id = $1",
            uuid
        )
        .execute(&mut *tx)
        .await
        .map_err(error::ErrorInternalServerError)?;

        for (app_uuid, connection_profile) in &app_uuids {
            sqlx::query(
                "INSERT INTO application_services (application_id, service_id, connection_profile) VALUES ($1, $2, $3)",
            )
            .bind(app_uuid)
            .bind(uuid)
            .bind(connection_profile)
            .execute(&mut *tx)
            .await
            .map_err(error::ErrorInternalServerError)?;
        }

        if container_id.as_deref().is_some_and(|s| !s.is_empty()) {
            let service_env: HashMap<String, String> = sqlx::query!(
                "SELECT key, value FROM service_env_vars WHERE service_id = $1 ORDER BY key",
                uuid
            )
            .fetch_all(&mut *tx)
            .await
            .map_err(error::ErrorInternalServerError)?
            .into_iter()
            .map(|r| (r.key, r.value))
            .collect();
            let service_port = service_port_for_service(&service_name);

            for (app_uuid, connection_profile) in &app_uuids {
                inject_service_env_into_app(
                    &mut tx,
                    *app_uuid,
                    &service_name,
                    &uuid.to_string(),
                    service_port,
                    &service_env,
                    Some(connection_profile.as_str()),
                )
                .await
                .map_err(error::ErrorInternalServerError)?;
            }
        }
    }

    tx.commit().await.map_err(error::ErrorInternalServerError)?;

    if !attached_app_uuids.is_empty() {
        ensure_attached_service_network(pool.get_ref(), &scheduler, uuid, &attached_app_uuids)
            .await?;
    }

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

    let service =
        sqlx::query("SELECT project_id, name, version, status, port FROM services WHERE id = $1")
            .bind(uuid)
            .fetch_optional(pool.get_ref())
            .await
            .map_err(error::ErrorInternalServerError)?
            .ok_or_else(|| error::ErrorNotFound("Resource not found"))?;
    let project_id: Uuid = service
        .try_get("project_id")
        .map_err(error::ErrorInternalServerError)?;
    let service_name: String = service
        .try_get("name")
        .map_err(error::ErrorInternalServerError)?;
    let version: String = service
        .try_get("version")
        .map_err(error::ErrorInternalServerError)?;
    let status: String = service
        .try_get("status")
        .map_err(error::ErrorInternalServerError)?;
    let port: Option<i32> = service
        .try_get("port")
        .map_err(error::ErrorInternalServerError)?;
    let project = Registry::get_project_by_id(&pool, project_id)
        .await
        .map_err(error::ErrorInternalServerError)?
        .ok_or_else(|| error::ErrorInternalServerError("Project not found"))?;

    if status == "running" {
        return Err(error::ErrorConflict("Resource is already running"));
    }

    let image = format!("{}:{}", container_image_for_service(&service_name), version);
    let container_port = service_port_for_service(&service_name);
    let existing_port = port.map(|p| p as u16);
    let env_vars = fetch_resource_env_vars(pool.get_ref(), uuid).await?;
    let binds = prepare_config_for_service(&service_name, &uuid.to_string())
        .map_err(error::ErrorInternalServerError)?;
    let (container_id, host_port) = scheduler
        .start_service(
            &uuid.to_string(),
            &project.network_name,
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
    scheduler: web::Data<Scheduler>,
    id: web::Path<String>,
) -> Result<impl Responder, Error> {
    let uuid = Uuid::parse_str(&id).map_err(|_| error::ErrorBadRequest("Invalid resource id"))?;

    let service = sqlx::query!("SELECT status FROM services WHERE id = $1", uuid)
        .fetch_optional(pool.get_ref())
        .await
        .map_err(error::ErrorInternalServerError)?
        .ok_or_else(|| error::ErrorNotFound("Resource not found"))?;

    if service.status == "running" {
        scheduler
            .stop_service(&uuid.to_string())
            .await
            .map_err(|e| error::ErrorInternalServerError(format!("Failed to stop service: {e}")))?;
    }

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
