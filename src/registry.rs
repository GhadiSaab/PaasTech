#![allow(dead_code)]

use chrono::NaiveDateTime;
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

pub struct Registry;

impl Registry {
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
}
