use chrono::NaiveDateTime;
use serde_json::Value;
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
