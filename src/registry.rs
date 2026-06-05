#![allow(dead_code)]

use chrono::NaiveDateTime;
use serde_json::Value;
use sqlx::PgPool;
use uuid::Uuid;

#[derive(Debug, sqlx::FromRow, serde::Serialize, utoipa::ToSchema)]
pub struct App {
    #[schema(value_type = String)]
    pub id: Uuid,
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
    pub process_type: String,
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
    pub application_id: Uuid,
    pub app_name: String,
    pub process_name: String,
    pub process_type: String,
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
        let app = sqlx::query_as!(
            App,
            r#"
            INSERT INTO applications (id, name, image_id, container_id, internal_port, port, status, base_domain, created_at)
            VALUES ($1, $2, $3, $4, $5, $6, $7, $8, NOW())
            RETURNING id, name, image_id, container_id, internal_port, port, status, base_domain, created_at
            "#,
            Uuid::new_v4(),
            name,
            image_id,
            container_id,
            internal_port,
            port,
            status,
            base_domain,
        )
        .fetch_one(pool)
        .await?;

        Ok(app)
    }

    pub async fn get(pool: &PgPool, name: &str) -> Result<Option<App>, sqlx::Error> {
        let app = sqlx::query_as!(
            App,
            r#"
            SELECT id, name, image_id, container_id, internal_port, port, status, base_domain, created_at
            FROM applications
            WHERE name = $1
            "#,
            name,
        )
        .fetch_optional(pool)
        .await?;

        Ok(app)
    }

    pub async fn list(pool: &PgPool) -> Result<Vec<App>, sqlx::Error> {
        let apps = sqlx::query_as!(
            App,
            r#"
            SELECT id, name, image_id, container_id, internal_port, port, status, base_domain, created_at
            FROM applications
            ORDER BY created_at ASC
            "#,
        )
        .fetch_all(pool)
        .await?;

        Ok(apps)
    }

    pub async fn delete(pool: &PgPool, name: &str) -> Result<(), sqlx::Error> {
        sqlx::query!(r#"DELETE FROM applications WHERE name = $1"#, name,)
            .execute(pool)
            .await?;

        Ok(())
    }

    pub async fn update_status(pool: &PgPool, name: &str, status: &str) -> Result<(), sqlx::Error> {
        sqlx::query!(
            r#"UPDATE applications SET status = $1 WHERE name = $2"#,
            status,
            name,
        )
        .execute(pool)
        .await?;

        Ok(())
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
        let app = sqlx::query_as!(
            App,
            r#"
            INSERT INTO applications (id, name, image_id, container_id, internal_port, port, status, base_domain, created_at)
            VALUES ($1, $2, $3, $4, $5, $6, $7, $8, NOW())
            ON CONFLICT (name) DO UPDATE SET
                image_id = EXCLUDED.image_id,
                container_id = EXCLUDED.container_id,
                internal_port = EXCLUDED.internal_port,
                port = EXCLUDED.port,
                status = EXCLUDED.status,
                base_domain = EXCLUDED.base_domain
            RETURNING id, name, image_id, container_id, internal_port, port, status, base_domain, created_at
            "#,
            Uuid::new_v4(),
            name,
            image_id,
            container_id,
            internal_port,
            port,
            status,
            base_domain,
        )
        .fetch_one(pool)
        .await?;

        Ok(app)
    }

    pub async fn update_container_id(
        pool: &PgPool,
        name: &str,
        container_id: &str,
    ) -> Result<(), sqlx::Error> {
        sqlx::query!(
            r#"UPDATE applications SET container_id = $1 WHERE name = $2"#,
            container_id,
            name,
        )
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
                use sqlx::Row;
                let key: String = row.try_get("key")?;
                let value: String = row.try_get("value")?;
                Ok(format!("{key}={value}"))
            })
            .collect()
    }
}
