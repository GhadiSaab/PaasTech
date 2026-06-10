use reqwest::Client;
use serde::Deserialize;
use sqlx::{PgPool, Row};
use uuid::Uuid;

use crate::scheduler::Scheduler;

pub const CONTAINER_NAME: &str = "paastech-s3-garage";
pub const S3_PORT: u16 = 3900;
const ADMIN_PORT: u16 = 3903;
const GARAGE_IMAGE_DEFAULT: &str = "dxflrs/garage:v2.3.0";
const INIT_NETWORK: &str = "paastech-s3-net";

pub struct GarageInstance {
    pub admin_port: u16,
    pub admin_token: String,
}

pub struct BucketCredentials {
    pub bucket_id: String,
    pub access_key_id: String,
    pub secret_access_key: String,
}

fn admin_host() -> String {
    std::env::var("GARAGE_ADMIN_HOST").unwrap_or_else(|_| "localhost".to_string())
}

pub async fn ensure_running(
    pool: &PgPool,
    scheduler: &Scheduler,
    http: &Client,
) -> Result<GarageInstance, Box<dyn std::error::Error + Send + Sync>> {
    if let Some(row) = sqlx::query("SELECT admin_port, admin_token FROM s3_instance LIMIT 1")
        .fetch_optional(pool)
        .await?
    {
        let admin_port: i32 = row.try_get("admin_port")?;
        let admin_token: String = row.try_get("admin_token")?;
        if scheduler.inspect(CONTAINER_NAME).await == "running" {
            return Ok(GarageInstance {
                admin_port: admin_port as u16,
                admin_token,
            });
        }
        sqlx::query("DELETE FROM s3_instance").execute(pool).await?;
    }

    let admin_token = format!("{}{}", Uuid::new_v4().simple(), Uuid::new_v4().simple());
    let (container_id, admin_host_port) = scheduler.start_garage_container(&admin_token).await?;

    wait_until_ready(http, admin_host_port, &admin_token).await?;
    initialize_layout(http, admin_host_port, &admin_token).await?;

    sqlx::query(
        "INSERT INTO s3_instance (container_id, admin_port, admin_token) VALUES ($1, $2, $3)",
    )
    .bind(&container_id)
    .bind(admin_host_port as i32)
    .bind(&admin_token)
    .execute(pool)
    .await?;

    Ok(GarageInstance {
        admin_port: admin_host_port,
        admin_token,
    })
}

async fn wait_until_ready(
    http: &Client,
    admin_port: u16,
    admin_token: &str,
) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    let url = format!("http://{}:{}/v1/health", admin_host(), admin_port);
    for _ in 0..30 {
        if let Ok(resp) = http.get(&url).bearer_auth(admin_token).send().await
            && resp.status().is_success()
        {
            return Ok(());
        }
        tokio::time::sleep(std::time::Duration::from_millis(500)).await;
    }
    Err("Garage did not become ready in 15s".into())
}

#[derive(Deserialize)]
struct ClusterStatusResponse {
    nodes: Vec<ClusterNode>,
}

#[derive(Deserialize)]
struct ClusterNode {
    id: String,
}

async fn initialize_layout(
    http: &Client,
    admin_port: u16,
    admin_token: &str,
) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    let base = format!("http://{}:{}", admin_host(), admin_port);

    let status: ClusterStatusResponse = http
        .get(format!("{}/v2/GetClusterStatus", base))
        .bearer_auth(admin_token)
        .send()
        .await?
        .json()
        .await?;

    let node_id = status
        .nodes
        .first()
        .ok_or("No nodes in cluster status")?
        .id
        .clone();

    http.post(format!("{}/v2/UpdateClusterLayout", base))
        .bearer_auth(admin_token)
        .json(&serde_json::json!({
            "roles": [{ "id": node_id, "zone": "dc1", "capacity": 1_000_000_000u64, "tags": [] }]
        }))
        .send()
        .await?
        .error_for_status()?;

    http.post(format!("{}/v2/ApplyClusterLayout", base))
        .bearer_auth(admin_token)
        .json(&serde_json::json!({ "version": 1 }))
        .send()
        .await?
        .error_for_status()?;

    Ok(())
}

#[derive(Deserialize)]
struct CreateBucketResponse {
    id: String,
}

#[derive(Deserialize)]
struct CreateKeyResponse {
    #[serde(rename = "accessKeyId")]
    access_key_id: String,
    #[serde(rename = "secretAccessKey")]
    secret_access_key: String,
}

pub async fn create_bucket_and_key(
    http: &Client,
    instance: &GarageInstance,
    bucket_name: &str,
    key_name: &str,
) -> Result<BucketCredentials, Box<dyn std::error::Error + Send + Sync>> {
    let base = format!("http://{}:{}", admin_host(), instance.admin_port);

    let bucket: CreateBucketResponse = http
        .post(format!("{}/v2/CreateBucket", base))
        .bearer_auth(&instance.admin_token)
        .json(&serde_json::json!({ "globalAlias": bucket_name }))
        .send()
        .await?
        .error_for_status()?
        .json()
        .await?;

    let key: CreateKeyResponse = http
        .post(format!("{}/v2/CreateKey", base))
        .bearer_auth(&instance.admin_token)
        .json(&serde_json::json!({ "name": key_name }))
        .send()
        .await?
        .error_for_status()?
        .json()
        .await?;

    http.post(format!("{}/v2/AllowBucketKey", base))
        .bearer_auth(&instance.admin_token)
        .json(&serde_json::json!({
            "bucketId": bucket.id,
            "accessKeyId": key.access_key_id,
            "permissions": { "read": true, "write": true, "owner": false }
        }))
        .send()
        .await?
        .error_for_status()?;

    Ok(BucketCredentials {
        bucket_id: bucket.id,
        access_key_id: key.access_key_id,
        secret_access_key: key.secret_access_key,
    })
}

pub async fn delete_bucket_and_key(
    http: &Client,
    instance: &GarageInstance,
    bucket_id: &str,
    access_key_id: &str,
) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    let base = format!("http://{}:{}", admin_host(), instance.admin_port);

    let _ = http
        .post(format!("{}/v2/DeleteKey?id={}", base, access_key_id))
        .bearer_auth(&instance.admin_token)
        .send()
        .await;

    let _ = http
        .post(format!("{}/v2/DeleteBucket?id={}", base, bucket_id))
        .bearer_auth(&instance.admin_token)
        .send()
        .await;

    Ok(())
}

pub(crate) fn garage_container_config(admin_token: &str) -> String {
    format!(
        r#"replication_factor = 1
metadata_dir = "/var/lib/garage/meta"
data_dir = "/var/lib/garage/data"
rpc_bind_addr = "[::]:3901"
rpc_secret = "{admin_token}"

[s3_api]
s3_region = "fr-south-1"
api_bind_addr = "0.0.0.0:{S3_PORT}"

[admin]
api_bind_addr = "0.0.0.0:{ADMIN_PORT}"
admin_token = "{admin_token}"
"#
    )
}

pub(crate) fn init_network() -> &'static str {
    INIT_NETWORK
}

pub(crate) fn admin_container_port() -> u16 {
    ADMIN_PORT
}

pub(crate) fn garage_image() -> String {
    std::env::var("GARAGE_IMAGE").unwrap_or_else(|_| GARAGE_IMAGE_DEFAULT.to_string())
}
