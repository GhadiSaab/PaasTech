use std::collections::HashMap;

use sqlx::{PgPool, Row};
use uuid::Uuid;

use super::Registry;

impl Registry {
    pub async fn get_process_env(
        pool: &PgPool,
        process_id: Uuid,
    ) -> Result<Vec<String>, sqlx::Error> {
        let rows = sqlx::query(
            r#"
            SELECT key, value
            FROM process_env_vars
            WHERE process_id = $1
            ORDER BY key
            "#,
        )
        .bind(process_id)
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

    pub async fn merged_process_env_vars(
        pool: &PgPool,
        project_id: Uuid,
        process_id: Uuid,
    ) -> Result<Vec<String>, sqlx::Error> {
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

        let process_rows = sqlx::query(
            r#"
            SELECT key, value
            FROM process_env_vars
            WHERE process_id = $1
            ORDER BY key
            "#,
        )
        .bind(process_id)
        .fetch_all(pool)
        .await?;

        let mut map: HashMap<String, String> = HashMap::new();
        for row in project_rows {
            let key: String = row.try_get("key")?;
            let value: String = row.try_get("value")?;
            map.insert(key, value);
        }
        for row in process_rows {
            let key: String = row.try_get("key")?;
            let value: String = row.try_get("value")?;
            map.insert(key, value);
        }

        Ok(map.into_iter().map(|(k, v)| format!("{k}={v}")).collect())
    }
}
