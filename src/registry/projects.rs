use sqlx::PgPool;
use uuid::Uuid;

use super::{
    models::{DEFAULT_PROJECT_ID, DEFAULT_PROJECT_NAME, DEFAULT_PROJECT_NETWORK},
    Project, Registry,
};

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
}
