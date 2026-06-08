use actix_multipart::Multipart;
use actix_web::{HttpRequest, HttpResponse, Responder, delete, error, get, post, put, web};
use futures_util::StreamExt;
use serde::Deserialize;
use serde_json::json;
use sqlx::{PgPool, Row};
use std::collections::HashMap;
use tokio::fs;
use uuid::Uuid;

use crate::engine::{
    MultipartData, ProcessType, build_image_with_name, extract_zip, load_process_definitions,
    load_resource_definitions, save_multipart_file,
};
use crate::extractor::ProjectScope;
use crate::handlers::resources::ensure_manifest_resource_attached;
use crate::registry::Registry;
use crate::scheduler::{Scheduler, app_container_name, app_process_container_name, find_free_port};

#[derive(Deserialize, utoipa::ToSchema)]
pub struct DeployBody {
    pub name: String,
    pub image: String,
    pub port: Option<u16>,
    pub base_domain: Option<String>,
}

#[derive(Deserialize, utoipa::ToSchema)]
pub struct UpdateBody {
    pub image: String,
    pub port: Option<u16>,
    pub base_domain: Option<String>,
}

#[derive(Deserialize)]
pub struct LogsQuery {
    pub tail: Option<usize>,
    pub process: Option<String>,
    pub follow: Option<bool>,
}

#[derive(Deserialize)]
pub struct EnvSetPayload {
    pub key: String,
    pub value: String,
}

fn app_name_from_request(req: &HttpRequest) -> String {
    req.match_info()
        .get("app_name")
        .unwrap_or_default()
        .to_string()
}

fn validate_app_name(name: &str) -> Result<(), String> {
    if name.trim().is_empty() {
        return Err("app name cannot be empty".to_string());
    }
    if name
        .chars()
        .any(|c| !(c.is_ascii_alphanumeric() || c == '-'))
    {
        return Err("app name must only contain letters, numbers, and hyphens".to_string());
    }
    Ok(())
}

pub(super) async fn fetch_project_env_vars_db(
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

pub(super) async fn merged_app_env_vars(
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

async fn handle_upload(
    pool: web::Data<PgPool>,
    scheduler: web::Data<Scheduler>,
    project_id: Uuid,
    project_network: String,
    data: MultipartData,
) -> Result<impl Responder, actix_web::Error> {
    let zip_filepath = match data.file_path {
        Some(path) => path,
        None => return Ok(HttpResponse::BadRequest().body("provide file in payload")),
    };

    let internal_port = data
        .fields
        .get("internal_port")
        .and_then(|p| p.trim().parse::<u16>().ok());

    let name = data
        .fields
        .get("name")
        .map(|name| name.trim().to_string())
        .filter(|name| !name.is_empty())
        .unwrap_or_else(|| format!("paastech-{}", Uuid::new_v4()));
    if let Err(e) = validate_app_name(&name) {
        return Ok(HttpResponse::BadRequest().json(json!({"error": e})));
    }

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
    let resources = match load_resource_definitions(&extracted_folder) {
        Ok(resources) => resources,
        Err(e) => {
            let _ = fs::remove_dir_all(&extracted_folder).await;
            return Ok(HttpResponse::BadRequest().json(json!({"error": e})));
        }
    };

    match Registry::get_in_project(&pool, project_id, &name).await {
        Ok(Some(existing)) => {
            match Registry::list_processes(&pool, existing.id).await {
                Ok(processes) if !processes.is_empty() => {
                    for process in processes {
                        scheduler
                            .stop(&app_process_container_name(
                                existing.project_id,
                                &name,
                                &process.name,
                            ))
                            .await;
                    }
                }
                Ok(_) => {
                    if existing
                        .container_id
                        .as_deref()
                        .is_some_and(|id| !id.is_empty())
                    {
                        scheduler
                            .stop(&app_container_name(existing.project_id, &name))
                            .await;
                    }
                }
                Err(e) => {
                    let _ = fs::remove_dir_all(&extracted_folder).await;
                    return Ok(
                        HttpResponse::InternalServerError().json(json!({"error": e.to_string()}))
                    );
                }
            }
            if let Err(e) = Registry::delete_processes(&pool, existing.id).await {
                let _ = fs::remove_dir_all(&extracted_folder).await;
                return Ok(
                    HttpResponse::InternalServerError().json(json!({"error": e.to_string()}))
                );
            }
        }
        Ok(None) => {}
        Err(e) => {
            let _ = fs::remove_dir_all(&extracted_folder).await;
            return Ok(HttpResponse::InternalServerError().json(json!({"error": e.to_string()})));
        }
    }

    let app = match Registry::upsert_in_project(
        &pool,
        project_id,
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

    let client = reqwest::Client::new();
    for resource in &resources {
        if let Err(e) = ensure_manifest_resource_attached(
            &pool,
            &client,
            scheduler.get_ref(),
            project_id,
            &project_network,
            app.id,
            resource,
        )
        .await
        {
            let _ = fs::remove_dir_all(&extracted_folder).await;
            let _ = Registry::update_status(&pool, project_id, &name, "failed").await;
            return Err(e);
        }
    }

    let pool_bg = pool.clone();
    let scheduler_bg = scheduler.clone();
    let name_bg = name.clone();
    tokio::spawn(async move {
        let mut failed = false;

        for (process, row) in processes.into_iter().zip(process_rows) {
            let image_name = format!("{}-{}", name_bg, process.name);
            let context = extracted_folder.join(&process.path);
            let env_vars = match merged_app_env_vars(&pool_bg, project_id, row.application_id).await
            {
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
                    project_id,
                    &project_network,
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
        let deploy_status = if failed { "failed" } else { "running" };
        let _ = Registry::update_status(&pool_bg, project_id, &name_bg, deploy_status).await;
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
#[post("/upload")]
pub async fn upload(
    scope: ProjectScope,
    scheduler: web::Data<Scheduler>,
    payload: Multipart,
) -> Result<impl Responder, actix_web::Error> {
    let data = save_multipart_file(payload).await?;
    handle_upload(
        scope.pool,
        scheduler,
        scope.project.id,
        scope.project.network_name,
        data,
    )
    .await
}

#[utoipa::path(
    get,
    path = "/app",
    responses(
        (status = 200, description = "List of deployed applications", body = Vec<crate::registry::App>),
        (status = 500, description = "Internal server error"),
    ),
    tag = "apps"
)]
#[get("")]
pub async fn list(scope: ProjectScope) -> impl Responder {
    match Registry::list_in_project(&scope.pool, scope.project.id).await {
        Ok(apps) => HttpResponse::Ok().json(apps),
        Err(e) => {
            eprintln!("registry: list_apps failed: {e}");
            HttpResponse::InternalServerError().finish()
        }
    }
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
#[post("/deploy")]
pub async fn deploy(
    scope: ProjectScope,
    scheduler: web::Data<Scheduler>,
    body: web::Json<DeployBody>,
) -> HttpResponse {
    let existing_app =
        match Registry::get_in_project(&scope.pool, scope.project.id, &body.name).await {
            Ok(app) => app,
            Err(e) => {
                eprintln!("deploy: failed to look up {}: {e}", body.name);
                return HttpResponse::InternalServerError().finish();
            }
        };
    let env_vars = if let Some(app) = existing_app {
        match merged_app_env_vars(&scope.pool, scope.project.id, app.id).await {
            Ok(env) => env,
            Err(e) => {
                eprintln!("deploy: failed to load env vars for {}: {e}", body.name);
                return HttpResponse::InternalServerError().finish();
            }
        }
    } else {
        match fetch_project_env_vars_db(&scope.pool, scope.project.id).await {
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
    use crate::scheduler::DeployError;
    match scheduler
        .deploy(
            &scope.pool,
            scope.project.id,
            &scope.project.network_name,
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
#[post("/{app_name}/update")]
pub async fn update(
    scope: ProjectScope,
    scheduler: web::Data<Scheduler>,
    req: HttpRequest,
    body: web::Json<UpdateBody>,
) -> impl Responder {
    let app_name = app_name_from_request(&req);
    let app = match Registry::get_in_project(&scope.pool, scope.project.id, &app_name).await {
        Ok(Some(app)) => app,
        Ok(None) => return HttpResponse::NotFound().finish(),
        Err(e) => {
            eprintln!("registry: get failed for {app_name}: {e}");
            return HttpResponse::InternalServerError().finish();
        }
    };
    let env_vars = match merged_app_env_vars(&scope.pool, scope.project.id, app.id).await {
        Ok(env_vars) => env_vars,
        Err(e) => {
            eprintln!("registry: failed to merge env vars for {app_name}: {e}");
            return HttpResponse::InternalServerError().finish();
        }
    };
    use crate::scheduler::DeployError;
    match scheduler
        .rolling_update(
            &scope.pool,
            scope.project.id,
            &scope.project.network_name,
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
#[post("/{app_name}/stop")]
pub async fn stop(
    scope: ProjectScope,
    scheduler: web::Data<Scheduler>,
    req: HttpRequest,
) -> impl Responder {
    let app_name = app_name_from_request(&req);
    let app = match Registry::get_in_project(&scope.pool, scope.project.id, &app_name).await {
        Ok(Some(app)) => app,
        Ok(None) => return HttpResponse::NotFound().finish(),
        Err(e) => {
            eprintln!("registry: get failed for {app_name}: {e}");
            return HttpResponse::InternalServerError().finish();
        }
    };
    match Registry::list_processes(&scope.pool, app.id).await {
        Ok(processes) if !processes.is_empty() => {
            for process in processes {
                scheduler
                    .stop(&app_process_container_name(
                        app.project_id,
                        &app_name,
                        &process.name,
                    ))
                    .await;
                if let Err(e) =
                    Registry::update_process_status(&scope.pool, process.id, "stopped").await
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

    if let Err(e) = Registry::update_status(&scope.pool, app.project_id, &app_name, "stopped").await
    {
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
#[post("/{app_name}/restart")]
pub async fn restart(
    scope: ProjectScope,
    scheduler: web::Data<Scheduler>,
    req: HttpRequest,
) -> impl Responder {
    let app_name = app_name_from_request(&req);
    let app = match Registry::get_in_project(&scope.pool, scope.project.id, &app_name).await {
        Ok(Some(app)) => app,
        Ok(None) => return HttpResponse::NotFound().finish(),
        Err(e) => {
            eprintln!("registry: get failed for {app_name}: {e}");
            return HttpResponse::InternalServerError().finish();
        }
    };

    let processes = match Registry::list_processes(&scope.pool, app.id).await {
        Ok(processes) => processes,
        Err(e) => {
            eprintln!("registry: failed to list processes for {app_name}: {e}");
            return HttpResponse::InternalServerError().finish();
        }
    };
    let project = match Registry::get_project_by_id(&scope.pool, app.project_id).await {
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

            let env_vars = match merged_app_env_vars(&scope.pool, app.project_id, app.id).await {
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
                        &scope.pool,
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
                    let _ =
                        Registry::update_process_status(&scope.pool, process.id, "failed").await;
                    return HttpResponse::InternalServerError().body(e.to_string());
                }
            }
        }

        if let Err(e) =
            Registry::update_status(&scope.pool, app.project_id, &app_name, "running").await
        {
            eprintln!("registry: failed to update status for {app_name}: {e}");
        }

        return HttpResponse::Ok().finish();
    }

    let image = match app.image_id.as_deref().filter(|s| !s.is_empty()) {
        Some(img) => img.to_string(),
        None => {
            if let Err(e) =
                Registry::update_status(&scope.pool, app.project_id, &app_name, "running").await
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
    let env_vars = match merged_app_env_vars(&scope.pool, app.project_id, app.id).await {
        Ok(env_vars) => env_vars,
        Err(e) => {
            eprintln!("registry: failed to merge env vars for {app_name}: {e}");
            return HttpResponse::InternalServerError().finish();
        }
    };

    if let Err(e) = scheduler
        .redeploy(
            &scope.pool,
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
#[get("/{app_name}/status")]
pub async fn status(
    scope: ProjectScope,
    scheduler: web::Data<Scheduler>,
    req: HttpRequest,
) -> impl Responder {
    let app_name = app_name_from_request(&req);
    let app = match Registry::get_in_project(&scope.pool, scope.project.id, &app_name).await {
        Ok(Some(app)) => app,
        Ok(None) => return HttpResponse::NotFound().finish(),
        Err(e) => {
            eprintln!("registry: get failed for {app_name}: {e}");
            return HttpResponse::InternalServerError().finish();
        }
    };

    let processes = match Registry::list_processes(&scope.pool, app.id).await {
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
#[delete("/{app_name}")]
pub async fn delete(
    scope: ProjectScope,
    scheduler: web::Data<Scheduler>,
    req: HttpRequest,
) -> impl Responder {
    let app_name = app_name_from_request(&req);
    let app = match Registry::get_in_project(&scope.pool, scope.project.id, &app_name).await {
        Ok(Some(app)) => app,
        Ok(None) => return HttpResponse::NotFound().finish(),
        Err(e) => {
            eprintln!("registry: get failed for {app_name}: {e}");
            return HttpResponse::InternalServerError().finish();
        }
    };

    match Registry::list_processes(&scope.pool, app.id).await {
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

    let mut tx = match scope.pool.begin().await {
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
#[get("/{app_name}/logs")]
pub async fn logs(
    scope: ProjectScope,
    scheduler: web::Data<Scheduler>,
    req: HttpRequest,
    query: web::Query<LogsQuery>,
) -> impl Responder {
    let app_name = app_name_from_request(&req);
    let app = match Registry::get_in_project(&scope.pool, scope.project.id, &app_name).await {
        Ok(Some(app)) => app,
        Ok(None) => return HttpResponse::NotFound().finish(),
        Err(e) => {
            eprintln!("registry: get failed for {app_name}: {e}");
            return HttpResponse::InternalServerError().finish();
        }
    };

    let container_name = match Registry::list_processes(&scope.pool, app.id).await {
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

    if query.follow.unwrap_or(false) {
        let stream = scheduler
            .get_logs_stream(container_name, query.tail)
            .map(|r| match r {
                Ok(s) => Ok(web::Bytes::from(s)),
                Err(e) => Err(error::ErrorInternalServerError(e)),
            });
        return HttpResponse::Ok()
            .content_type("text/plain; charset=utf-8")
            .streaming(stream);
    }

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
    path = "/app/{app_name}/env",
    params(("app_name" = String, Path, description = "Application name")),
    responses(
        (status = 200, description = "Environment variables as key-value map"),
        (status = 404, description = "Application not found"),
        (status = 500, description = "Internal server error"),
    ),
    tag = "apps"
)]
#[get("/{app_name}/env")]
pub async fn get_env(scope: ProjectScope, req: HttpRequest) -> impl Responder {
    let app_name = app_name_from_request(&req);
    let app = match Registry::get_in_project(&scope.pool, scope.project.id, &app_name).await {
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
    .fetch_all(scope.pool.get_ref())
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
#[post("/{app_name}/env")]
pub async fn set_env(
    scope: ProjectScope,
    req: HttpRequest,
    payload: web::Json<EnvSetPayload>,
) -> impl Responder {
    let app_name = app_name_from_request(&req);
    let app = match Registry::get_in_project(&scope.pool, scope.project.id, &app_name).await {
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
    .execute(scope.pool.get_ref())
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
#[put("/{app_name}/env")]
pub async fn update_env(
    scope: ProjectScope,
    req: HttpRequest,
    payload: web::Json<HashMap<String, String>>,
) -> impl Responder {
    let app_name = app_name_from_request(&req);
    let app = match Registry::get_in_project(&scope.pool, scope.project.id, &app_name).await {
        Ok(Some(app)) => app,
        Ok(None) => return HttpResponse::NotFound().finish(),
        Err(e) => {
            eprintln!("registry: get failed for {app_name}: {e}");
            return HttpResponse::InternalServerError().finish();
        }
    };

    let mut tx = match scope.pool.begin().await {
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
