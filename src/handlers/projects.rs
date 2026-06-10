use actix_web::{Error, HttpResponse, Responder, delete, error, get, post, put, web};
use sqlx::{PgPool, Row};
use std::collections::HashMap;
use uuid::Uuid;

use crate::models::CreateProjectPayload;
use crate::registry::{DEFAULT_PROJECT_NAME, Registry};
use crate::scheduler::{Scheduler, app_container_name, app_process_container_name};

#[utoipa::path(
    post,
    path = "/project",
    request_body = CreateProjectPayload,
    responses(
        (status = 201, description = "Project created", body = crate::registry::Project),
        (status = 409, description = "Project already exists"),
        (status = 500, description = "Internal server error"),
    ),
    tag = "projects"
)]
#[post("/project")]
pub async fn create(
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
    responses((status = 200, description = "Projects", body = Vec<crate::registry::Project>)),
    tag = "projects"
)]
#[get("/project")]
pub async fn list(pool: web::Data<PgPool>) -> Result<impl Responder, Error> {
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
        (status = 200, description = "Project", body = crate::registry::Project),
        (status = 404, description = "Project not found"),
    ),
    tag = "projects"
)]
#[get("/project/{project}")]
pub async fn get(
    pool: web::Data<PgPool>,
    path: web::Path<String>,
) -> Result<impl Responder, Error> {
    let project = Registry::get_project(&pool, &path.into_inner())
        .await
        .map_err(error::ErrorInternalServerError)?
        .ok_or_else(|| error::ErrorNotFound("Project not found"))?;
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
pub async fn delete(
    pool: web::Data<PgPool>,
    scheduler: web::Data<Scheduler>,
    path: web::Path<String>,
) -> Result<impl Responder, Error> {
    let name = path.into_inner();
    if name == DEFAULT_PROJECT_NAME {
        return Err(error::ErrorBadRequest("Default project cannot be deleted"));
    }

    let project = Registry::get_project(&pool, &name)
        .await
        .map_err(error::ErrorInternalServerError)?
        .ok_or_else(|| error::ErrorNotFound("Project not found"))?;

    let apps = Registry::list_in_project(&pool, project.id)
        .await
        .map_err(error::ErrorInternalServerError)?;
    for app in apps {
        let processes = Registry::list_processes(&pool, app.id)
            .await
            .map_err(error::ErrorInternalServerError)?;
        if !processes.is_empty() {
            for process in processes {
                scheduler
                    .stop(&app_process_container_name(
                        project.id,
                        &app.name,
                        &process.name,
                    ))
                    .await;
            }
        } else {
            scheduler
                .stop(&app_container_name(project.id, &app.name))
                .await;
        }
    }

    let services = sqlx::query("SELECT id FROM services WHERE project_id = $1")
        .bind(project.id)
        .fetch_all(pool.get_ref())
        .await
        .map_err(error::ErrorInternalServerError)?;
    for service in services {
        let service_id: Uuid = service.get("id");
        let _ = scheduler.stop_service(&service_id.to_string()).await;
    }

    let traefik_container =
        std::env::var("TRAEFIK_CONTAINER").unwrap_or_else(|_| "paastech-traefik-1".to_string());
    scheduler
        .disconnect_from_network(&project.network_name, &traefik_container)
        .await;
    scheduler.remove_network(&project.network_name).await;

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
pub async fn get_env(
    pool: web::Data<PgPool>,
    path: web::Path<String>,
) -> Result<impl Responder, Error> {
    let project = Registry::get_project(&pool, &path.into_inner())
        .await
        .map_err(error::ErrorInternalServerError)?
        .ok_or_else(|| error::ErrorNotFound("Project not found"))?;
    let rows =
        sqlx::query("SELECT key, value FROM project_env_vars WHERE project_id = $1 ORDER BY key")
            .bind(project.id)
            .fetch_all(pool.get_ref())
            .await
            .map_err(error::ErrorInternalServerError)?;
    let env: HashMap<String, String> = rows
        .into_iter()
        .map(|r| {
            use sqlx::Row;
            (r.get("key"), r.get("value"))
        })
        .collect();
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
pub async fn update_env(
    pool: web::Data<PgPool>,
    path: web::Path<String>,
    payload: web::Json<HashMap<String, String>>,
) -> Result<impl Responder, Error> {
    let project = Registry::get_project(&pool, &path.into_inner())
        .await
        .map_err(error::ErrorInternalServerError)?
        .ok_or_else(|| error::ErrorNotFound("Project not found"))?;
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
