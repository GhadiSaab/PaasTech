use actix_web::{HttpResponse, Responder, delete, error, get, patch, post, put, web};
use reqwest::Client;
use serde::Deserialize;
use sqlx::{PgPool, Row};
use std::collections::HashMap;
use uuid::Uuid;

use crate::docker::{
    connection_env_vars_for_service, container_image_for_service, default_env_vars_for_service,
    default_version_for_service, docker_image_for_service, is_valid_service,
    prepare_config_for_service, service_port_for_service, valid_services,
    validate_connection_profile_for_service, validate_docker_tag,
};
use crate::engine::ResourceDefinition;
use crate::extractor::ProjectScope;
use crate::models::{CreateResourcePayload, Resource, ResourceAttachment, UpdateResourcePayload};
use crate::registry::Registry;
use crate::scheduler::{Scheduler, app_container_name, app_process_container_name};

#[derive(Deserialize)]
pub struct LogsQuery {
    pub tail: Option<usize>,
}

async fn fetch_resource_env_vars(
    pool: &PgPool,
    service_id: Uuid,
) -> Result<Vec<String>, actix_web::Error> {
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

fn map_resource_insert_error(err: sqlx::Error, display_name: &str) -> actix_web::Error {
    if let sqlx::Error::Database(db_err) = &err
        && db_err.constraint() == Some("services_project_id_display_name_key")
    {
        return error::ErrorConflict(format!(
            "Resource '{}' already exists in this project",
            display_name
        ));
    }
    error::ErrorInternalServerError(err)
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
    if let Some(profile) = connection_profile {
        connection_env.insert(
            "PAASTECH_CONNECTION_PROFILE".to_string(),
            profile.to_string(),
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
) -> Result<ResourceAttachment, actix_web::Error> {
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
) -> Result<Vec<ResourceAttachment>, actix_web::Error> {
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
            .map(|a| ResourceAttachment {
                application_id: a.application_id.clone(),
                connection_profile: a.connection_profile.clone(),
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
) -> Result<(), actix_web::Error> {
    for app_id in app_ids {
        let Some(app) = sqlx::query_as::<_, crate::registry::App>(
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

pub async fn ensure_manifest_resource_attached(
    pool: &PgPool,
    client: &Client,
    scheduler: &Scheduler,
    project_id: Uuid,
    project_network: &str,
    application_id: Uuid,
    resource: &ResourceDefinition,
) -> Result<(), actix_web::Error> {
    if !is_valid_service(&resource.service_type) {
        return Err(error::ErrorBadRequest(format!(
            "Invalid service name '{}'. Must be one of: {}",
            resource.service_type,
            valid_services().join(", ")
        )));
    }

    validate_connection_profile_for_service(&resource.service_type, resource.connection.as_deref())
        .map_err(error::ErrorBadRequest)?;

    let version = resource
        .version
        .clone()
        .unwrap_or_else(|| default_version_for_service(&resource.service_type).to_string());
    let docker_image = docker_image_for_service(&resource.service_type);
    validate_docker_tag(client, docker_image, &version).await?;

    let existing = sqlx::query(
        "SELECT id, name, version, status, container_id, port FROM services WHERE project_id = $1 AND display_name = $2",
    )
    .bind(project_id)
    .bind(&resource.name)
    .fetch_optional(pool)
    .await
    .map_err(error::ErrorInternalServerError)?;

    let service_id;
    let mut status;
    let port;
    let service_env_map: HashMap<String, String>;

    if let Some(existing) = existing {
        let existing_name: String = existing
            .try_get("name")
            .map_err(error::ErrorInternalServerError)?;
        if existing_name != resource.service_type {
            return Err(error::ErrorConflict(format!(
                "Resource '{}' already exists with type '{}', not '{}'",
                resource.name, existing_name, resource.service_type
            )));
        }
        service_id = existing
            .try_get("id")
            .map_err(error::ErrorInternalServerError)?;
        status = existing
            .try_get("status")
            .map_err(error::ErrorInternalServerError)?;
        port = existing
            .try_get::<Option<i32>, _>("port")
            .map_err(error::ErrorInternalServerError)?;
        service_env_map =
            sqlx::query("SELECT key, value FROM service_env_vars WHERE service_id = $1")
                .bind(service_id)
                .fetch_all(pool)
                .await
                .map_err(error::ErrorInternalServerError)?
                .into_iter()
                .map(|row| {
                    Ok((
                        row.try_get("key")
                            .map_err(error::ErrorInternalServerError)?,
                        row.try_get("value")
                            .map_err(error::ErrorInternalServerError)?,
                    ))
                })
                .collect::<Result<HashMap<String, String>, actix_web::Error>>()?;
    } else {
        service_id = Uuid::new_v4();
        let default_env = default_env_vars_for_service(&resource.service_type);
        service_env_map = default_env.iter().cloned().collect();

        let mut tx = pool
            .begin()
            .await
            .map_err(error::ErrorInternalServerError)?;
        sqlx::query(
            "INSERT INTO services (id, project_id, display_name, name, version) VALUES ($1, $2, $3, $4, $5)",
        )
        .bind(service_id)
        .bind(project_id)
        .bind(&resource.name)
        .bind(&resource.service_type)
        .bind(&version)
        .execute(&mut *tx)
        .await
        .map_err(|e| map_resource_insert_error(e, &resource.name))?;

        for (key, value) in &default_env {
            sqlx::query!(
                "INSERT INTO service_env_vars (service_id, key, value) VALUES ($1, $2, $3)",
                service_id,
                key,
                value,
            )
            .execute(&mut *tx)
            .await
            .map_err(error::ErrorInternalServerError)?;
        }
        tx.commit().await.map_err(error::ErrorInternalServerError)?;
        status = "stopped".to_string();
        port = None;
    }

    if status != "running" {
        let image = format!(
            "{}:{}",
            container_image_for_service(&resource.service_type),
            version
        );
        let container_port = service_port_for_service(&resource.service_type);
        let existing_port = port.map(|p| p as u16);
        let env_vars: Vec<String> = service_env_map
            .iter()
            .map(|(key, value)| format!("{key}={value}"))
            .collect();
        let binds = prepare_config_for_service(&resource.service_type, &service_id.to_string())
            .map_err(error::ErrorInternalServerError)?;
        let (container_id, host_port) = scheduler
            .start_service(
                &service_id.to_string(),
                project_network,
                &image,
                container_port,
                existing_port,
                env_vars,
                binds,
            )
            .await
            .map_err(|e| {
                error::ErrorInternalServerError(format!("Failed to start service: {e}"))
            })?;

        sqlx::query!(
            "UPDATE services SET status = 'running', container_id = $1, port = $2 WHERE id = $3",
            container_id,
            host_port as i32,
            service_id,
        )
        .execute(pool)
        .await
        .map_err(error::ErrorInternalServerError)?;
        status = "running".to_string();
    }

    if status == "running" {
        let mut tx = pool
            .begin()
            .await
            .map_err(error::ErrorInternalServerError)?;
        sqlx::query(
            "INSERT INTO application_services (application_id, service_id, connection_profile) VALUES ($1, $2, $3) ON CONFLICT (application_id, service_id) DO UPDATE SET connection_profile = EXCLUDED.connection_profile",
        )
        .bind(application_id)
        .bind(service_id)
        .bind(resource.connection.as_deref().unwrap_or_default())
        .execute(&mut *tx)
        .await
        .map_err(error::ErrorInternalServerError)?;

        inject_service_env_into_app(
            &mut tx,
            application_id,
            &resource.service_type,
            &service_id.to_string(),
            service_port_for_service(&resource.service_type),
            &service_env_map,
            resource.connection.as_deref(),
        )
        .await
        .map_err(error::ErrorInternalServerError)?;
        tx.commit().await.map_err(error::ErrorInternalServerError)?;
    }

    ensure_attached_service_network(pool, scheduler, service_id, &[application_id]).await?;
    Ok(())
}

const RESOURCE_LIST_QUERY: &str = r#"SELECT
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
GROUP BY s.id, s.project_id, s.display_name, s.name, s.version, s.status"#;

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
#[post("")]
pub async fn create(
    scope: ProjectScope,
    client: web::Data<Client>,
    scheduler: web::Data<Scheduler>,
    payload: web::Json<CreateResourcePayload>,
) -> Result<impl Responder, actix_web::Error> {
    if !is_valid_service(&payload.name) {
        return Err(error::ErrorBadRequest(format!(
            "Invalid service name '{}'. Must be one of: {}",
            payload.name,
            valid_services().join(", ")
        )));
    }

    let version = payload
        .version
        .clone()
        .unwrap_or_else(|| default_version_for_service(&payload.name).to_string());
    let docker_image = docker_image_for_service(&payload.name);
    validate_docker_tag(&client, docker_image, &version).await?;

    let id = Uuid::new_v4();
    let attachments = resource_attachments_from_payload(
        &payload.name,
        payload.application_id.as_deref(),
        payload.connection_profile.as_deref(),
        payload.attachments.as_ref(),
    )?;

    let mut tx = scope
        .pool
        .begin()
        .await
        .map_err(error::ErrorInternalServerError)?;

    sqlx::query(
        "INSERT INTO services (id, project_id, display_name, name, version) VALUES ($1, $2, $3, $4, $5)",
    )
    .bind(id)
    .bind(scope.project.id)
    .bind(&payload.display_name)
    .bind(&payload.name)
    .bind(&version)
    .execute(&mut *tx)
    .await
    .map_err(|e| map_resource_insert_error(e, &payload.display_name))?;

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

    let image = format!("{}:{}", container_image_for_service(&payload.name), version);
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
            &scope.project.network_name,
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
        ensure_attached_service_network(scope.pool.get_ref(), &scheduler, id, &[app_uuid]).await?;
    }

    Ok(HttpResponse::Created().json(Resource {
        id: id.to_string(),
        project_id: scope.project.id.to_string(),
        display_name: payload.display_name.clone(),
        name: payload.name.clone(),
        version,
        status: "running".to_string(),
        application_ids: attachments
            .iter()
            .map(|a| a.application_id.clone())
            .collect(),
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
#[get("")]
pub async fn list(scope: ProjectScope) -> Result<impl Responder, actix_web::Error> {
    let resources = sqlx::query_as::<_, Resource>(RESOURCE_LIST_QUERY)
        .bind(scope.project.id)
        .fetch_all(scope.pool.get_ref())
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
pub async fn get(
    pool: web::Data<PgPool>,
    id: web::Path<String>,
) -> Result<impl Responder, actix_web::Error> {
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
        GROUP BY s.id, s.project_id, s.display_name, s.name, s.version, s.status"#,
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
pub async fn update(
    pool: web::Data<PgPool>,
    client: web::Data<Client>,
    scheduler: web::Data<Scheduler>,
    id: web::Path<String>,
    payload: web::Json<UpdateResourcePayload>,
) -> Result<impl Responder, actix_web::Error> {
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
            .map(|a| {
                Ok((
                    Uuid::parse_str(&a.application_id)
                        .map_err(|_| error::ErrorBadRequest("Invalid application_id"))?,
                    a.connection_profile.clone(),
                ))
            })
            .collect::<Result<_, actix_web::Error>>()?;
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
pub async fn start(
    pool: web::Data<PgPool>,
    scheduler: web::Data<Scheduler>,
    id: web::Path<String>,
) -> Result<impl Responder, actix_web::Error> {
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
pub async fn stop(
    pool: web::Data<PgPool>,
    scheduler: web::Data<Scheduler>,
    id: web::Path<String>,
) -> Result<impl Responder, actix_web::Error> {
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
pub async fn delete(
    pool: web::Data<PgPool>,
    scheduler: web::Data<Scheduler>,
    id: web::Path<String>,
) -> Result<impl Responder, actix_web::Error> {
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
pub async fn logs(
    scheduler: web::Data<Scheduler>,
    pool: web::Data<PgPool>,
    id: web::Path<String>,
    query: web::Query<LogsQuery>,
) -> Result<impl Responder, actix_web::Error> {
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
pub async fn get_env(
    pool: web::Data<PgPool>,
    id: web::Path<String>,
) -> Result<impl Responder, actix_web::Error> {
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
pub async fn update_env(
    pool: web::Data<PgPool>,
    id: web::Path<String>,
    payload: web::Json<HashMap<String, String>>,
) -> Result<impl Responder, actix_web::Error> {
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
