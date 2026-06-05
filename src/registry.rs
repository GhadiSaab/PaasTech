#![allow(dead_code)]

use chrono::NaiveDateTime;
use serde_json::Value;
use sqlx::{PgPool, Row};
use uuid::Uuid;

use crate::engine::ProcessType;

pub const DEFAULT_PROJECT_ID: Uuid = Uuid::from_u128(1);
pub const DEFAULT_PROJECT_NAME: &str = "default";
pub const DEFAULT_PROJECT_NETWORK: &str = "paastech-default";

#[derive(Debug, sqlx::FromRow, serde::Serialize, utoipa::ToSchema)]
pub struct Project {
    #[schema(value_type = String)]
    pub id: Uuid,
    pub name: String,
    pub network_name: String,
    #[schema(value_type = Option<String>)]
    pub created_at: Option<NaiveDateTime>,
}

#[derive(Debug, sqlx::FromRow, serde::Serialize, utoipa::ToSchema)]
pub struct App {
    #[schema(value_type = String)]
    pub id: Uuid,
    #[schema(value_type = String)]
    pub project_id: Uuid,
    pub name: String,
    pub image_id: Option<String>,
    pub container_id: Option<String>,
    pub internal_port: Option<i32>,
    pub port: Option<i32>,
    pub status: Option<String>,
    pub base_domain: Option<String>,
    #[schema(value_type = Option<String>)]
    pub created_at: Option<NaiveDateTime>,
}

#[derive(Debug, sqlx::FromRow, serde::Serialize, utoipa::ToSchema)]
pub struct AppProcess {
    #[schema(value_type = String)]
    pub id: Uuid,
    #[schema(value_type = String)]
    pub application_id: Uuid,
    pub name: String,
    #[sqlx(try_from = "String")]
    #[schema(value_type = String)]
    pub process_type: ProcessType,
    pub build_context: String,
    pub public_host: Option<String>,
    pub build_env: Option<Value>,
    pub image_id: Option<String>,
    pub container_id: Option<String>,
    pub internal_port: Option<i32>,
    pub host_port: Option<i32>,
    pub status: String,
    #[schema(value_type = Option<String>)]
    pub created_at: Option<NaiveDateTime>,
}

#[derive(Debug, sqlx::FromRow)]
pub struct ActiveAppProcess {
    pub id: Uuid,
    pub project_id: Uuid,
    pub project_network: String,
    pub application_id: Uuid,
    pub app_name: String,
    pub process_name: String,
    #[sqlx(try_from = "String")]
    pub process_type: ProcessType,
    pub build_context: String,
    pub public_host: Option<String>,
    pub build_env: Option<Value>,
    pub image_id: Option<String>,
    pub container_id: Option<String>,
    pub internal_port: Option<i32>,
    pub host_port: Option<i32>,
    pub status: String,
    pub base_domain: Option<String>,
}

pub struct Registry;

impl Registry {
    pub async fn ensure_default_project(pool: &PgPool) -> Result<Project, sqlx::Error> {
        sqlx::query_as::<_, Project>(
            r#"
            INSERT INTO projects (id, name, network_name, created_at)
            VALUES ($1, $2, $3, NOW())
            ON CONFLICT (name) DO UPDATE SET name = EXCLUDED.name
            RETURNING id, name, network_name, created_at
            "#,
        )
        .bind(DEFAULT_PROJECT_ID)
        .bind(DEFAULT_PROJECT_NAME)
        .bind(DEFAULT_PROJECT_NETWORK)
        .fetch_one(pool)
        .await
    }

    pub async fn create_project(pool: &PgPool, name: &str) -> Result<Project, sqlx::Error> {
        let id = Uuid::new_v4();
        let network_name = format!("paastech-{}", id.simple());
        sqlx::query_as::<_, Project>(
            r#"
            INSERT INTO projects (id, name, network_name, created_at)
            VALUES ($1, $2, $3, NOW())
            RETURNING id, name, network_name, created_at
            "#,
        )
        .bind(id)
        .bind(name)
        .bind(network_name)
        .fetch_one(pool)
        .await
    }

    pub async fn get_project(pool: &PgPool, name: &str) -> Result<Option<Project>, sqlx::Error> {
        sqlx::query_as::<_, Project>(
            "SELECT id, name, network_name, created_at FROM projects WHERE name = $1",
        )
        .bind(name)
        .fetch_optional(pool)
        .await
    }

    pub async fn get_project_by_id(
        pool: &PgPool,
        id: Uuid,
    ) -> Result<Option<Project>, sqlx::Error> {
        sqlx::query_as::<_, Project>(
            "SELECT id, name, network_name, created_at FROM projects WHERE id = $1",
        )
        .bind(id)
        .fetch_optional(pool)
        .await
    }

    pub async fn list_projects(pool: &PgPool) -> Result<Vec<Project>, sqlx::Error> {
        sqlx::query_as::<_, Project>(
            "SELECT id, name, network_name, created_at FROM projects ORDER BY created_at ASC",
        )
        .fetch_all(pool)
        .await
    }

    pub async fn delete_project(pool: &PgPool, name: &str) -> Result<u64, sqlx::Error> {
        let result = sqlx::query("DELETE FROM projects WHERE name = $1")
            .bind(name)
            .execute(pool)
            .await?;
        Ok(result.rows_affected())
    }

    #[allow(clippy::too_many_arguments)]
    pub async fn save_in_project(
        pool: &PgPool,
        project_id: Uuid,
        name: &str,
        image_id: &str,
        container_id: &str,
        internal_port: Option<i32>,
        port: i32,
        status: &str,
        base_domain: Option<&str>,
    ) -> Result<App, sqlx::Error> {
        let app = sqlx::query_as::<_, App>(
            r#"
            INSERT INTO applications (id, project_id, name, image_id, container_id, internal_port, port, status, base_domain, created_at)
            VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, NOW())
            RETURNING id, project_id, name, image_id, container_id, internal_port, port, status, base_domain, created_at
            "#,
        )
        .bind(Uuid::new_v4())
        .bind(project_id)
        .bind(name)
        .bind(image_id)
        .bind(container_id)
        .bind(internal_port)
        .bind(port)
        .bind(status)
        .bind(base_domain)
        .fetch_one(pool)
        .await?;

        Ok(app)
    }

    #[allow(clippy::too_many_arguments)]
    pub async fn save(
        pool: &PgPool,
        name: &str,
        image_id: &str,
        container_id: &str,
        internal_port: Option<i32>,
        port: i32,
        status: &str,
        base_domain: Option<&str>,
    ) -> Result<App, sqlx::Error> {
        Self::ensure_default_project(pool).await?;
        Self::save_in_project(
            pool,
            DEFAULT_PROJECT_ID,
            name,
            image_id,
            container_id,
            internal_port,
            port,
            status,
            base_domain,
        )
        .await
    }

    pub async fn get_in_project(
        pool: &PgPool,
        project_id: Uuid,
        name: &str,
    ) -> Result<Option<App>, sqlx::Error> {
        let app = sqlx::query_as::<_, App>(
            r#"
            SELECT id, project_id, name, image_id, container_id, internal_port, port, status, base_domain, created_at
            FROM applications
            WHERE project_id = $1 AND name = $2
            "#,
        )
        .bind(project_id)
        .bind(name)
        .fetch_optional(pool)
        .await?;

        Ok(app)
    }

    pub async fn get(pool: &PgPool, name: &str) -> Result<Option<App>, sqlx::Error> {
        Self::ensure_default_project(pool).await?;
        Self::get_in_project(pool, DEFAULT_PROJECT_ID, name).await
    }

    pub async fn list_in_project(pool: &PgPool, project_id: Uuid) -> Result<Vec<App>, sqlx::Error> {
        let apps = sqlx::query_as::<_, App>(
            r#"
            SELECT id, project_id, name, image_id, container_id, internal_port, port, status, base_domain, created_at
            FROM applications
            WHERE project_id = $1
            ORDER BY created_at ASC
            "#,
        )
        .bind(project_id)
        .fetch_all(pool)
        .await?;

        Ok(apps)
    }

    pub async fn list(pool: &PgPool) -> Result<Vec<App>, sqlx::Error> {
        Self::ensure_default_project(pool).await?;
        Self::list_in_project(pool, DEFAULT_PROJECT_ID).await
    }

    pub async fn list_all(pool: &PgPool) -> Result<Vec<App>, sqlx::Error> {
        sqlx::query_as::<_, App>(
            r#"
            SELECT id, project_id, name, image_id, container_id, internal_port, port, status, base_domain, created_at
            FROM applications
            ORDER BY created_at ASC
            "#,
        )
        .fetch_all(pool)
        .await
    }

    pub async fn delete(pool: &PgPool, name: &str) -> Result<(), sqlx::Error> {
        Self::ensure_default_project(pool).await?;
        sqlx::query(r#"DELETE FROM applications WHERE project_id = $1 AND name = $2"#)
            .bind(DEFAULT_PROJECT_ID)
            .bind(name)
            .execute(pool)
            .await?;

        Ok(())
    }

    pub async fn update_status(
        pool: &PgPool,
        project_id: Uuid,
        name: &str,
        status: &str,
    ) -> Result<(), sqlx::Error> {
        sqlx::query(r#"UPDATE applications SET status = $1 WHERE name = $2 AND project_id = $3"#)
            .bind(status)
            .bind(name)
            .bind(project_id)
            .execute(pool)
            .await?;

        Ok(())
    }

    #[allow(clippy::too_many_arguments)]
    pub async fn upsert_in_project(
        pool: &PgPool,
        project_id: Uuid,
        name: &str,
        image_id: &str,
        container_id: &str,
        internal_port: Option<i32>,
        port: i32,
        status: &str,
        base_domain: Option<&str>,
    ) -> Result<App, sqlx::Error> {
        let app = sqlx::query_as::<_, App>(
            r#"
            INSERT INTO applications (id, project_id, name, image_id, container_id, internal_port, port, status, base_domain, created_at)
            VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, NOW())
            ON CONFLICT (project_id, name) DO UPDATE SET
                image_id = EXCLUDED.image_id,
                container_id = EXCLUDED.container_id,
                internal_port = EXCLUDED.internal_port,
                port = EXCLUDED.port,
                status = EXCLUDED.status,
                base_domain = EXCLUDED.base_domain
            RETURNING id, project_id, name, image_id, container_id, internal_port, port, status, base_domain, created_at
            "#,
        )
        .bind(Uuid::new_v4())
        .bind(project_id)
        .bind(name)
        .bind(image_id)
        .bind(container_id)
        .bind(internal_port)
        .bind(port)
        .bind(status)
        .bind(base_domain)
        .fetch_one(pool)
        .await?;

        Ok(app)
    }

    #[allow(clippy::too_many_arguments)]
    pub async fn upsert(
        pool: &PgPool,
        name: &str,
        image_id: &str,
        container_id: &str,
        internal_port: Option<i32>,
        port: i32,
        status: &str,
        base_domain: Option<&str>,
    ) -> Result<App, sqlx::Error> {
        Self::ensure_default_project(pool).await?;
        Self::upsert_in_project(
            pool,
            DEFAULT_PROJECT_ID,
            name,
            image_id,
            container_id,
            internal_port,
            port,
            status,
            base_domain,
        )
        .await
    }

    pub async fn update_container_id(
        pool: &PgPool,
        project_id: Uuid,
        name: &str,
        container_id: &str,
    ) -> Result<(), sqlx::Error> {
        sqlx::query(
            r#"UPDATE applications SET container_id = $1 WHERE name = $2 AND project_id = $3"#,
        )
        .bind(container_id)
        .bind(name)
        .bind(project_id)
        .execute(pool)
        .await?;

        Ok(())
    }

    #[allow(clippy::too_many_arguments)]
    pub async fn create_process(
        pool: &PgPool,
        application_id: Uuid,
        name: &str,
        process_type: &str,
        build_context: &str,
        public_host: Option<&str>,
        build_env: Value,
        internal_port: Option<i32>,
        status: &str,
    ) -> Result<AppProcess, sqlx::Error> {
        sqlx::query_as::<_, AppProcess>(
            r#"
            INSERT INTO application_processes (
                id, application_id, name, process_type, build_context, public_host, build_env,
                internal_port, status, created_at
            )
            VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, NOW())
            RETURNING id, application_id, name, process_type, build_context, public_host, build_env,
                image_id, container_id, internal_port, host_port, status, created_at
            "#,
        )
        .bind(Uuid::new_v4())
        .bind(application_id)
        .bind(name)
        .bind(process_type)
        .bind(build_context)
        .bind(public_host)
        .bind(build_env)
        .bind(internal_port)
        .bind(status)
        .fetch_one(pool)
        .await
    }

    pub async fn list_processes(
        pool: &PgPool,
        application_id: Uuid,
    ) -> Result<Vec<AppProcess>, sqlx::Error> {
        sqlx::query_as::<_, AppProcess>(
            r#"
            SELECT id, application_id, name, process_type, build_context, public_host, build_env,
                image_id, container_id, internal_port, host_port, status, created_at
            FROM application_processes
            WHERE application_id = $1
            ORDER BY created_at ASC
            "#,
        )
        .bind(application_id)
        .fetch_all(pool)
        .await
    }

    pub async fn list_active_processes(
        pool: &PgPool,
    ) -> Result<Vec<ActiveAppProcess>, sqlx::Error> {
        sqlx::query_as::<_, ActiveAppProcess>(
            r#"
            SELECT
                p.id,
                a.project_id,
                pr.network_name AS project_network,
                p.application_id,
                a.name AS app_name,
                p.name AS process_name,
                p.process_type,
                p.build_context,
                p.public_host,
                p.build_env,
                p.image_id,
                p.container_id,
                p.internal_port,
                p.host_port,
                p.status,
                a.base_domain
            FROM application_processes p
            JOIN applications a ON a.id = p.application_id
            JOIN projects pr ON pr.id = a.project_id
            WHERE p.status IN ('running', 'crashed')
            ORDER BY p.created_at ASC
            "#,
        )
        .fetch_all(pool)
        .await
    }

    pub async fn update_process_running(
        pool: &PgPool,
        process_id: Uuid,
        image_id: &str,
        container_id: &str,
        internal_port: Option<i32>,
        host_port: Option<i32>,
    ) -> Result<(), sqlx::Error> {
        sqlx::query(
            r#"
            UPDATE application_processes
            SET image_id = $1,
                container_id = $2,
                internal_port = $3,
                host_port = $4,
                status = 'running'
            WHERE id = $5
            "#,
        )
        .bind(image_id)
        .bind(container_id)
        .bind(internal_port)
        .bind(host_port)
        .bind(process_id)
        .execute(pool)
        .await?;

        Ok(())
    }

    pub async fn update_process_status(
        pool: &PgPool,
        process_id: Uuid,
        status: &str,
    ) -> Result<(), sqlx::Error> {
        sqlx::query("UPDATE application_processes SET status = $1 WHERE id = $2")
            .bind(status)
            .bind(process_id)
            .execute(pool)
            .await?;

        Ok(())
    }

    pub async fn get_app_env(
        pool: &PgPool,
        application_id: Uuid,
    ) -> Result<Vec<String>, sqlx::Error> {
        let rows = sqlx::query(
            r#"
            SELECT key, value
            FROM application_env_vars
            WHERE application_id = $1
            ORDER BY key
            "#,
        )
        .bind(application_id)
        .fetch_all(pool)
        .await?;

        rows.into_iter()
            .map(|row| {
                let key: String = row.try_get("key")?;
                let value: String = row.try_get("value")?;
                Ok(format!("{key}={value}"))
            })
            .collect()
    }

    pub async fn merged_env_vars(
        pool: &PgPool,
        project_id: Uuid,
        app_id: Uuid,
    ) -> Result<Vec<String>, sqlx::Error> {
        use std::collections::HashMap;

        let project_rows = sqlx::query(
            r#"
            SELECT key, value
            FROM project_env_vars
            WHERE project_id = $1
            ORDER BY key
            "#,
        )
        .bind(project_id)
        .fetch_all(pool)
        .await?;

        let app_rows = sqlx::query(
            r#"
            SELECT key, value
            FROM application_env_vars
            WHERE application_id = $1
            ORDER BY key
            "#,
        )
        .bind(app_id)
        .fetch_all(pool)
        .await?;

        let mut map: HashMap<String, String> = HashMap::new();
        for row in project_rows {
            let key: String = row.try_get("key")?;
            let value: String = row.try_get("value")?;
            map.insert(key, value);
        }
        for row in app_rows {
            let key: String = row.try_get("key")?;
            let value: String = row.try_get("value")?;
            map.insert(key, value);
        }

        Ok(map.into_iter().map(|(k, v)| format!("{k}={v}")).collect())
    }
}
