#![allow(dead_code)]

use chrono::NaiveDateTime;
use serde_json::Value;
use sqlx::PgPool;
use uuid::Uuid;

#[derive(Debug, sqlx::FromRow, serde::Serialize)]
pub struct App {
    pub id: Uuid,
    pub name: String,
    pub image_id: Option<String>,
    pub container_id: Option<String>,
    pub port: Option<i32>,
    pub status: Option<String>,
    pub env: Option<Value>,
    pub created_at: Option<NaiveDateTime>,
}

pub struct Registry;

impl Registry {
    pub async fn save(
        pool: &PgPool,
        name: &str,
        image_id: &str,
        container_id: &str,
        port: i32,
        status: &str,
        env: Option<Value>,
    ) -> Result<App, sqlx::Error> {
        let app = sqlx::query_as!(
            App,
            r#"
            INSERT INTO applications (id, name, image_id, container_id, port, status, env, created_at)
            VALUES ($1, $2, $3, $4, $5, $6, $7, NOW())
            RETURNING id, name, image_id, container_id, port, status, env, created_at
            "#,
            Uuid::new_v4(),
            name,
            image_id,
            container_id,
            port,
            status,
            env as Option<Value>,
        )
        .fetch_one(pool)
        .await?;

        Ok(app)
    }

    pub async fn get(pool: &PgPool, name: &str) -> Result<Option<App>, sqlx::Error> {
        let app = sqlx::query_as!(
            App,
            r#"
            SELECT id, name, image_id, container_id, port, status, env, created_at
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
            SELECT id, name, image_id, container_id, port, status, env, created_at
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
}
