use sqlx::PgPool;
use uuid::Uuid;

use super::{App, Registry, models::DEFAULT_PROJECT_ID, models::derived_status};

impl Registry {
    async fn attach_processes(pool: &PgPool, mut app: App) -> Result<App, sqlx::Error> {
        app.processes = Self::list_processes(pool, app.id).await?;
        app.status = derived_status(&app.processes).to_string();
        Ok(app)
    }

    async fn attach_processes_to_apps(
        pool: &PgPool,
        apps: Vec<App>,
    ) -> Result<Vec<App>, sqlx::Error> {
        let mut hydrated = Vec::with_capacity(apps.len());
        for app in apps {
            hydrated.push(Self::attach_processes(pool, app).await?);
        }
        Ok(hydrated)
    }

    pub async fn save_in_project(
        pool: &PgPool,
        project_id: Uuid,
        name: &str,
        base_domain: Option<&str>,
    ) -> Result<App, sqlx::Error> {
        let app = sqlx::query_as::<_, App>(
            r#"
            INSERT INTO applications (id, project_id, name, base_domain, created_at)
            VALUES ($1, $2, $3, $4, NOW())
            ON CONFLICT (project_id, name) DO UPDATE SET
                base_domain = EXCLUDED.base_domain
            RETURNING id, project_id, name, base_domain, created_at
            "#,
        )
        .bind(Uuid::new_v4())
        .bind(project_id)
        .bind(name)
        .bind(base_domain)
        .fetch_one(pool)
        .await?;
        Self::attach_processes(pool, app).await
    }

    pub async fn get_in_project(
        pool: &PgPool,
        project_id: Uuid,
        name: &str,
    ) -> Result<Option<App>, sqlx::Error> {
        let app = sqlx::query_as::<_, App>(
            "SELECT id, project_id, name, base_domain, created_at FROM applications WHERE project_id = $1 AND name = $2",
        )
        .bind(project_id)
        .bind(name)
        .fetch_optional(pool)
        .await?;
        match app {
            Some(app) => Self::attach_processes(pool, app).await.map(Some),
            None => Ok(None),
        }
    }

    pub async fn get_by_id(pool: &PgPool, id: Uuid) -> Result<Option<App>, sqlx::Error> {
        let app = sqlx::query_as::<_, App>(
            "SELECT id, project_id, name, base_domain, created_at FROM applications WHERE id = $1",
        )
        .bind(id)
        .fetch_optional(pool)
        .await?;
        match app {
            Some(app) => Self::attach_processes(pool, app).await.map(Some),
            None => Ok(None),
        }
    }

    pub async fn get(pool: &PgPool, name: &str) -> Result<Option<App>, sqlx::Error> {
        Self::ensure_default_project(pool).await?;
        Self::get_in_project(pool, DEFAULT_PROJECT_ID, name).await
    }

    pub async fn list_in_project(pool: &PgPool, project_id: Uuid) -> Result<Vec<App>, sqlx::Error> {
        let apps = sqlx::query_as::<_, App>(
            "SELECT id, project_id, name, base_domain, created_at FROM applications WHERE project_id = $1 ORDER BY created_at ASC",
        )
        .bind(project_id)
        .fetch_all(pool)
        .await?;
        Self::attach_processes_to_apps(pool, apps).await
    }

    pub async fn list(pool: &PgPool) -> Result<Vec<App>, sqlx::Error> {
        Self::ensure_default_project(pool).await?;
        Self::list_in_project(pool, DEFAULT_PROJECT_ID).await
    }

    pub async fn list_all(pool: &PgPool) -> Result<Vec<App>, sqlx::Error> {
        let apps = sqlx::query_as::<_, App>(
            "SELECT id, project_id, name, base_domain, created_at FROM applications ORDER BY created_at ASC",
        )
        .fetch_all(pool)
        .await?;
        Self::attach_processes_to_apps(pool, apps).await
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
}
