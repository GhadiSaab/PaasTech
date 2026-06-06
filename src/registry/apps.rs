use sqlx::PgPool;
use uuid::Uuid;

use super::{models::DEFAULT_PROJECT_ID, App, Registry};

impl Registry {
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
        sqlx::query_as::<_, App>(
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
        .await
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
        sqlx::query_as::<_, App>(
            r#"
            SELECT id, project_id, name, image_id, container_id, internal_port, port, status, base_domain, created_at
            FROM applications
            WHERE project_id = $1 AND name = $2
            "#,
        )
        .bind(project_id)
        .bind(name)
        .fetch_optional(pool)
        .await
    }

    pub async fn get(pool: &PgPool, name: &str) -> Result<Option<App>, sqlx::Error> {
        Self::ensure_default_project(pool).await?;
        Self::get_in_project(pool, DEFAULT_PROJECT_ID, name).await
    }

    pub async fn list_in_project(pool: &PgPool, project_id: Uuid) -> Result<Vec<App>, sqlx::Error> {
        sqlx::query_as::<_, App>(
            r#"
            SELECT id, project_id, name, image_id, container_id, internal_port, port, status, base_domain, created_at
            FROM applications
            WHERE project_id = $1
            ORDER BY created_at ASC
            "#,
        )
        .bind(project_id)
        .fetch_all(pool)
        .await
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
        sqlx::query_as::<_, App>(
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
        .await
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
}
