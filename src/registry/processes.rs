use serde_json::Value;
use sqlx::PgPool;
use uuid::Uuid;

use super::{ActiveAppProcess, AppProcess, Registry};

impl Registry {
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
}
