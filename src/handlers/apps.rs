use actix_multipart::Multipart;
use actix_web::{HttpRequest, HttpResponse, Responder, delete, error, get, post, put, web};
use futures_util::StreamExt;
use reqwest::Client;
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
use crate::scheduler::{Scheduler, app_process_container_name};

#[derive(Deserialize)]
pub struct LogsQuery {
    pub tail: Option<usize>,
    pub process: Option<String>,
    pub follow: Option<bool>,
}

#[derive(Deserialize)]
pub struct ProcessQuery {
    pub process: Option<String>,
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

fn process_name_from_request(req: &HttpRequest) -> String {
    req.match_info()
        .get("process_name")
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

async fn mark_processes_status(
    pool: &PgPool,
    processes: &[crate::registry::AppProcess],
    process_status: &str,
) {
    for process in processes {
        if let Err(e) = Registry::update_process_status(pool, process.id, process_status).await {
            eprintln!(
                "registry: failed to update process {} status to {process_status}: {e}",
                process.name
            );
        }
    }
}

async fn handle_upload(
    pool: web::Data<PgPool>,
    scheduler: web::Data<Scheduler>,
    client: web::Data<Client>,
    project_id: Uuid,
    project_network: String,
    data: MultipartData,
) -> Result<impl Responder, actix_web::Error> {
    let zip_filepath = match data.file_path {
        Some(path) => path,
        None => return Ok(HttpResponse::BadRequest().body("provide file in payload")),
    };

    let internal_port = match data.fields.get("internal_port") {
        Some(v) => match v.trim().parse::<u16>() {
            Ok(p) => Some(p),
            Err(_) => {
                return Ok(HttpResponse::BadRequest()
                    .json(json!({"error": format!("invalid internal_port: {v:?}")})));
            }
        },
        None => None,
    };

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
                Ok(processes) => {
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

    let app = match Registry::save_in_project(&pool, project_id, &name, None).await {
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
            process.replica_group.as_deref(),
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
            mark_processes_status(&pool, &process_rows, "failed").await;
            return Err(e);
        }
    }

    let pool_bg = pool.clone();
    let pool_panic = pool.clone();
    let scheduler_bg = scheduler.clone();
    let name_bg = name.clone();
    let name_panic = name.clone();
    let extracted_folder_panic = extracted_folder.clone();
    let handle = tokio::spawn(async move {
        for (process, row) in processes.into_iter().zip(process_rows) {
            let image_name = format!("{}-{}", name_bg, process.name);
            let context = extracted_folder.join(&process.path);
            let env_vars =
                match Registry::merged_process_env_vars(&pool_bg, project_id, row.id).await {
                    Ok(env_vars) => env_vars,
                    Err(e) => {
                        eprintln!(
                            "registry: failed to load env vars for {} process {}: {e}",
                            name_bg, process.name
                        );
                        let _ = Registry::update_process_status(&pool_bg, row.id, "failed").await;
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
                    process.replica_group.as_deref(),
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
                    break;
                }
            }
        }

        let _ = fs::remove_dir_all(&extracted_folder).await;
    });
    tokio::spawn(async move {
        if let Err(e) = handle.await {
            eprintln!("deploy task panicked for {name_panic}: {e}");
            let _ = fs::remove_dir_all(&extracted_folder_panic).await;
            if let Ok(Some(app)) =
                Registry::get_in_project(&pool_panic, project_id, &name_panic).await
            {
                mark_processes_status(&pool_panic, &app.processes, "failed").await;
            }
        }
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
    client: web::Data<Client>,
    payload: Multipart,
) -> Result<impl Responder, actix_web::Error> {
    let data = save_multipart_file(payload).await?;
    handle_upload(
        scope.pool,
        scheduler,
        client,
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
    path = "/app/{app_name}/update",
    params(("app_name" = String, Path, description = "Application name")),
    request_body(content_type = "multipart/form-data", description = "Zip archive of the application. Form fields: file (zip), internal_port (container port to expose)"),
    responses(
        (status = 202, description = "Rolling update accepted, build running in background"),
        (status = 400, description = "Missing file or invalid manifest"),
        (status = 404, description = "Application not found"),
        (status = 500, description = "Internal server error"),
    ),
    tag = "apps"
)]
#[post("/{app_name}/update")]
pub async fn update(
    scope: ProjectScope,
    scheduler: web::Data<Scheduler>,
    client: web::Data<Client>,
    req: HttpRequest,
    payload: Multipart,
) -> Result<impl Responder, actix_web::Error> {
    let app_name = app_name_from_request(&req);
    let app = match Registry::get_in_project(&scope.pool, scope.project.id, &app_name).await {
        Ok(Some(app)) => app,
        Ok(None) => return Ok(HttpResponse::NotFound().finish()),
        Err(e) => {
            eprintln!("registry: get failed for {app_name}: {e}");
            return Ok(HttpResponse::InternalServerError().finish());
        }
    };

    let data = save_multipart_file(payload).await?;
    let zip_filepath = match data.file_path {
        Some(path) => path,
        None => return Ok(HttpResponse::BadRequest().body("provide file in payload")),
    };
    let internal_port = match data.fields.get("internal_port") {
        Some(v) => match v.trim().parse::<u16>() {
            Ok(p) => Some(p),
            Err(_) => {
                return Ok(HttpResponse::BadRequest()
                    .json(json!({"error": format!("invalid internal_port: {v:?}")})));
            }
        },
        None => None,
    };

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

    let existing_processes = match Registry::list_processes(&scope.pool, app.id).await {
        Ok(processes) => processes,
        Err(e) => {
            let _ = fs::remove_dir_all(&extracted_folder).await;
            eprintln!("registry: failed to list processes for {app_name}: {e}");
            return Ok(HttpResponse::InternalServerError().finish());
        }
    };
    let mut existing_by_name: HashMap<_, _> = existing_processes
        .into_iter()
        .map(|process| (process.name.clone(), process))
        .collect();

    let mut process_rows = Vec::new();
    for process in &processes {
        let existed = existing_by_name.contains_key(&process.name);
        let row = if let Some(row) = existing_by_name.remove(&process.name) {
            if let Err(e) = Registry::update_process_definition(
                &scope.pool,
                row.id,
                process.process_type.as_str(),
                &process.path,
                process.public_host.as_deref(),
                json!(process.build_env),
                process.port.map(|p| p as i32),
                "building",
                process.replica_group.as_deref(),
            )
            .await
            {
                let _ = fs::remove_dir_all(&extracted_folder).await;
                eprintln!(
                    "registry: failed to update process definition {} for {app_name}: {e}",
                    process.name
                );
                return Ok(HttpResponse::InternalServerError().finish());
            }
            row
        } else {
            match Registry::create_process(
                &scope.pool,
                app.id,
                &process.name,
                process.process_type.as_str(),
                &process.path,
                process.public_host.as_deref(),
                json!(process.build_env),
                process.port.map(|p| p as i32),
                "building",
                process.replica_group.as_deref(),
            )
            .await
            {
                Ok(row) => row,
                Err(e) => {
                    let _ = fs::remove_dir_all(&extracted_folder).await;
                    eprintln!(
                        "registry: failed to create process {} for {app_name}: {e}",
                        process.name
                    );
                    return Ok(HttpResponse::InternalServerError().finish());
                }
            }
        };
        process_rows.push((process.clone(), row, existed));
    }
    let removed_processes: Vec<_> = existing_by_name.into_values().collect();

    for resource in &resources {
        if let Err(e) = ensure_manifest_resource_attached(
            &scope.pool,
            &client,
            scheduler.get_ref(),
            scope.project.id,
            &scope.project.network_name,
            app.id,
            resource,
        )
        .await
        {
            let _ = fs::remove_dir_all(&extracted_folder).await;
            let rows: Vec<_> = process_rows.iter().map(|(_, row, _)| row.clone()).collect();
            mark_processes_status(&scope.pool, &rows, crate::status::FAILED).await;
            return Err(e);
        }
    }

    let pool_bg = scope.pool.clone();
    let pool_panic = scope.pool.clone();
    let scheduler_bg = scheduler.clone();
    let project_id = scope.project.id;
    let project_network = scope.project.network_name.clone();
    let app_name_bg = app_name.clone();
    let app_name_panic = app_name.clone();
    let base_domain = app.base_domain.clone();
    let extracted_folder_panic = extracted_folder.clone();
    let handle = tokio::spawn(async move {
        let mut failed = false;
        let mut rolled_back = false;

        for (process, row, existed) in process_rows {
            let image_name = format!("{}-{}", app_name_bg, process.name);
            let context = extracted_folder.join(&process.path);
            let env_vars =
                match Registry::merged_process_env_vars(&pool_bg, project_id, row.id).await {
                    Ok(env_vars) => env_vars,
                    Err(e) => {
                        eprintln!(
                            "registry: failed to load env vars for {} process {}: {e}",
                            app_name_bg, process.name
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
                    app_name_bg, process.name
                );
                let _ = Registry::update_process_status(&pool_bg, row.id, "failed").await;
                failed = true;
                break;
            }

            let start_result = match process.process_type {
                ProcessType::Web if existed => {
                    match scheduler_bg
                        .rolling_update_process(
                            &pool_bg,
                            project_id,
                            &project_network,
                            &app_name_bg,
                            row.id,
                            &process.name,
                            &image_name,
                            process.port,
                            env_vars,
                            process.public_host.as_deref(),
                            base_domain.as_deref(),
                            process.replica_group.as_deref(),
                        )
                        .await
                    {
                        Ok(()) => continue,
                        Err(e) => Err(e),
                    }
                }
                ProcessType::Web => {
                    scheduler_bg
                        .start_process(
                            project_id,
                            &project_network,
                            &app_name_bg,
                            &process.name,
                            &process.process_type,
                            &image_name,
                            process.port,
                            base_domain.as_deref(),
                            process.public_host.as_deref(),
                            env_vars,
                            process.replica_group.as_deref(),
                        )
                        .await
                }
                ProcessType::Worker => {
                    if existed {
                        scheduler_bg
                            .stop(&app_process_container_name(
                                project_id,
                                &app_name_bg,
                                &process.name,
                            ))
                            .await;
                    }
                    scheduler_bg
                        .start_process(
                            project_id,
                            &project_network,
                            &app_name_bg,
                            &process.name,
                            &process.process_type,
                            &image_name,
                            process.port,
                            base_domain.as_deref(),
                            process.public_host.as_deref(),
                            env_vars,
                            process.replica_group.as_deref(),
                        )
                        .await
                }
            };

            match start_result {
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
                        "update: failed for {} process {}: {e}",
                        app_name_bg, process.name
                    );
                    if matches!(e, crate::scheduler::DeployError::RolledBack(_)) {
                        rolled_back = true;
                    } else {
                        let _ = Registry::update_process_status(&pool_bg, row.id, "failed").await;
                        failed = true;
                    }
                    break;
                }
            }
        }

        if !failed && !rolled_back {
            for process in removed_processes {
                scheduler_bg
                    .stop(&app_process_container_name(
                        project_id,
                        &app_name_bg,
                        &process.name,
                    ))
                    .await;
                if let Err(e) = Registry::delete_process(&pool_bg, process.id).await {
                    eprintln!(
                        "registry: failed to delete removed process {} for {}: {e}",
                        process.name, app_name_bg
                    );
                }
            }
        }

        let _ = fs::remove_dir_all(&extracted_folder).await;
    });
    tokio::spawn(async move {
        if let Err(e) = handle.await {
            eprintln!("update task panicked for {app_name_panic}: {e}");
            let _ = fs::remove_dir_all(&extracted_folder_panic).await;
            if let Ok(Some(app)) =
                Registry::get_in_project(&pool_panic, project_id, &app_name_panic).await
            {
                mark_processes_status(&pool_panic, &app.processes, crate::status::FAILED).await;
            }
        }
    });

    Ok(HttpResponse::Accepted().json(json!({"name": app_name})))
}

#[utoipa::path(
    post,
    path = "/app/{app_name}/stop",
    params(
        ("app_name" = String, Path, description = "Application name"),
        ("process" = Option<String>, Query, description = "Process name to stop; stops all processes when omitted"),
    ),
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
    query: web::Query<ProcessQuery>,
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
    let processes: Vec<_> = match query.process.as_deref() {
        Some(process_name) => {
            let Some(process) = app.processes.into_iter().find(|p| p.name == process_name) else {
                return HttpResponse::NotFound().body("process not found");
            };
            vec![process]
        }
        None => app.processes,
    };

    for process in processes {
        scheduler
            .stop(&app_process_container_name(
                app.project_id,
                &app_name,
                &process.name,
            ))
            .await;
        if let Err(e) = Registry::update_process_status(&scope.pool, process.id, "stopped").await {
            eprintln!(
                "registry: failed to update process {} for {app_name}: {e}",
                process.name
            );
        }
    }
    HttpResponse::Ok().finish()
}

#[utoipa::path(
    post,
    path = "/app/{app_name}/restart",
    params(
        ("app_name" = String, Path, description = "Application name"),
        ("process" = Option<String>, Query, description = "Process name to restart; restarts all processes when omitted"),
    ),
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
    query: web::Query<ProcessQuery>,
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

    let project = match Registry::get_project_by_id(&scope.pool, app.project_id).await {
        Ok(Some(project)) => project,
        Ok(None) => return HttpResponse::InternalServerError().body("project not found"),
        Err(e) => {
            eprintln!("registry: failed to load project for {app_name}: {e}");
            return HttpResponse::InternalServerError().finish();
        }
    };

    let processes: Vec<_> = match query.process.as_deref() {
        Some(process_name) => {
            let Some(process) = app.processes.into_iter().find(|p| p.name == process_name) else {
                return HttpResponse::NotFound().body("process not found");
            };
            vec![process]
        }
        None => app.processes,
    };

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

        let env_vars = match Registry::merged_process_env_vars(
            &scope.pool,
            app.project_id,
            process.id,
        )
        .await
        {
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
                process.replica_group.as_deref(),
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
                let _ = Registry::update_process_status(&scope.pool, process.id, "failed").await;
                return HttpResponse::InternalServerError().body(e.to_string());
            }
        }
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
    _scheduler: web::Data<Scheduler>,
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

    HttpResponse::Ok().body(app.status)
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

    let containers: Vec<_> = app
        .processes
        .iter()
        .map(|process| app_process_container_name(app.project_id, &app_name, &process.name))
        .collect();

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

    for container in containers {
        scheduler.stop(&container).await;
    }

    HttpResponse::NoContent().finish()
}

#[utoipa::path(
    get,
    path = "/app/{app_name}/logs",
    params(
        ("app_name" = String, Path, description = "Application name"),
        ("tail" = Option<usize>, Query, description = "Number of lines to return from the end (default: all)"),
        ("process" = Option<String>, Query, description = "Process name to read logs from"),
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

    let selected_name = match query.process.as_deref() {
        Some(name) => app
            .processes
            .iter()
            .find(|process| process.name == name)
            .map(|process| process.name.clone()),
        None => app
            .processes
            .iter()
            .find(|process| process.process_type == ProcessType::Web)
            .or_else(|| app.processes.first())
            .map(|process| process.name.clone()),
    };
    let Some(process_name) = selected_name else {
        return HttpResponse::NotFound().body("process not found");
    };
    let container_name = app_process_container_name(app.project_id, &app_name, &process_name);

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
    path = "/app/{app_name}/process/{process_name}/env",
    params(
        ("app_name" = String, Path, description = "Application name"),
        ("process_name" = String, Path, description = "Process name")
    ),
    responses(
        (status = 200, description = "Environment variables as key-value map"),
        (status = 404, description = "Application or process not found"),
        (status = 500, description = "Internal server error"),
    ),
    tag = "apps"
)]
#[get("/{app_name}/process/{process_name}/env")]
pub async fn get_env(scope: ProjectScope, req: HttpRequest) -> impl Responder {
    let app_name = app_name_from_request(&req);
    let process_name = process_name_from_request(&req);
    let app = match Registry::get_in_project(&scope.pool, scope.project.id, &app_name).await {
        Ok(Some(app)) => app,
        Ok(None) => return HttpResponse::NotFound().finish(),
        Err(e) => {
            eprintln!("registry: get failed for {app_name}: {e}");
            return HttpResponse::InternalServerError().finish();
        }
    };
    let process = match Registry::get_process_by_name(&scope.pool, app.id, &process_name).await {
        Ok(Some(process)) => process,
        Ok(None) => return HttpResponse::NotFound().finish(),
        Err(e) => {
            eprintln!("registry: get process failed for {app_name}/{process_name}: {e}");
            return HttpResponse::InternalServerError().finish();
        }
    };

    let rows = match sqlx::query(
        "SELECT key, value FROM process_env_vars WHERE process_id = $1 ORDER BY key",
    )
    .bind(process.id)
    .fetch_all(scope.pool.get_ref())
    .await
    {
        Ok(rows) => rows,
        Err(e) => {
            eprintln!("registry: failed to fetch env vars for {app_name}/{process_name}: {e}");
            return HttpResponse::InternalServerError().finish();
        }
    };

    let mut env_map = HashMap::new();
    for row in rows {
        let key: String = match row.try_get("key") {
            Ok(key) => key,
            Err(e) => {
                eprintln!("registry: failed to decode env key for {app_name}/{process_name}: {e}");
                return HttpResponse::InternalServerError().finish();
            }
        };
        let value: String = match row.try_get("value") {
            Ok(value) => value,
            Err(e) => {
                eprintln!(
                    "registry: failed to decode env value for {app_name}/{process_name}: {e}"
                );
                return HttpResponse::InternalServerError().finish();
            }
        };
        env_map.insert(key, value);
    }
    HttpResponse::Ok().json(env_map)
}

#[utoipa::path(
    post,
    path = "/app/{app_name}/process/{process_name}/env",
    params(
        ("app_name" = String, Path, description = "Application name"),
        ("process_name" = String, Path, description = "Process name")
    ),
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
#[post("/{app_name}/process/{process_name}/env")]
pub async fn set_env(
    scope: ProjectScope,
    req: HttpRequest,
    payload: web::Json<EnvSetPayload>,
) -> impl Responder {
    let app_name = app_name_from_request(&req);
    let process_name = process_name_from_request(&req);
    let app = match Registry::get_in_project(&scope.pool, scope.project.id, &app_name).await {
        Ok(Some(app)) => app,
        Ok(None) => return HttpResponse::NotFound().finish(),
        Err(e) => {
            eprintln!("registry: get failed for {app_name}: {e}");
            return HttpResponse::InternalServerError().finish();
        }
    };
    let process = match Registry::get_process_by_name(&scope.pool, app.id, &process_name).await {
        Ok(Some(process)) => process,
        Ok(None) => return HttpResponse::NotFound().finish(),
        Err(e) => {
            eprintln!("registry: get process failed for {app_name}/{process_name}: {e}");
            return HttpResponse::InternalServerError().finish();
        }
    };

    let result = sqlx::query(
        "INSERT INTO process_env_vars (process_id, key, value) VALUES ($1, $2, $3) \
         ON CONFLICT (process_id, key) DO UPDATE SET value = EXCLUDED.value",
    )
    .bind(process.id)
    .bind(&payload.key)
    .bind(&payload.value)
    .execute(scope.pool.get_ref())
    .await;

    match result {
        Ok(_) => HttpResponse::Ok().finish(),
        Err(e) => {
            eprintln!("registry: failed to set env var for {app_name}/{process_name}: {e}");
            HttpResponse::InternalServerError().finish()
        }
    }
}

#[utoipa::path(
    put,
    path = "/app/{app_name}/process/{process_name}/env",
    params(
        ("app_name" = String, Path, description = "Application name"),
        ("process_name" = String, Path, description = "Process name")
    ),
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
#[put("/{app_name}/process/{process_name}/env")]
pub async fn update_env(
    scope: ProjectScope,
    req: HttpRequest,
    payload: web::Json<HashMap<String, String>>,
) -> impl Responder {
    let app_name = app_name_from_request(&req);
    let process_name = process_name_from_request(&req);
    let app = match Registry::get_in_project(&scope.pool, scope.project.id, &app_name).await {
        Ok(Some(app)) => app,
        Ok(None) => return HttpResponse::NotFound().finish(),
        Err(e) => {
            eprintln!("registry: get failed for {app_name}: {e}");
            return HttpResponse::InternalServerError().finish();
        }
    };
    let process = match Registry::get_process_by_name(&scope.pool, app.id, &process_name).await {
        Ok(Some(process)) => process,
        Ok(None) => return HttpResponse::NotFound().finish(),
        Err(e) => {
            eprintln!("registry: get process failed for {app_name}/{process_name}: {e}");
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

    if let Err(e) = sqlx::query("DELETE FROM process_env_vars WHERE process_id = $1")
        .bind(process.id)
        .execute(&mut *tx)
        .await
    {
        eprintln!("registry: failed to delete env vars for {app_name}/{process_name}: {e}");
        return HttpResponse::InternalServerError().finish();
    }

    for (key, value) in payload.iter() {
        if let Err(e) =
            sqlx::query("INSERT INTO process_env_vars (process_id, key, value) VALUES ($1, $2, $3)")
                .bind(process.id)
                .bind(key)
                .bind(value)
                .execute(&mut *tx)
                .await
        {
            eprintln!("registry: failed to insert env var for {app_name}/{process_name}: {e}");
            return HttpResponse::InternalServerError().finish();
        }
    }

    if let Err(e) = tx.commit().await {
        eprintln!("registry: failed to commit env vars for {app_name}/{process_name}: {e}");
        return HttpResponse::InternalServerError().finish();
    }

    HttpResponse::Ok()
        .body("Environment variables updated. Restart the application to apply changes.")
}
