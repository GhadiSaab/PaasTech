use serde::{Deserialize, Serialize};
use utoipa::ToSchema;

#[derive(Deserialize, ToSchema)]
pub struct CreateProjectPayload {
    pub name: String,
}

// Resource models
#[derive(Serialize, Deserialize, sqlx::FromRow, ToSchema)]
pub struct Resource {
    pub id: String,
    pub project_id: String,
    pub display_name: String,
    pub name: String,
    pub version: String,
    pub status: String,
    pub application_ids: Vec<String>,
}

#[derive(Deserialize, ToSchema)]
pub struct ResourceAttachment {
    pub application_id: String,
    pub connection_profile: String,
}

#[derive(Deserialize, ToSchema)]
pub struct CreateResourcePayload {
    pub display_name: String,
    pub name: String,
    pub version: Option<String>,
    pub application_id: Option<String>,
    pub connection_profile: Option<String>,
    pub attachments: Option<Vec<ResourceAttachment>>,
}

#[derive(Deserialize, ToSchema)]
pub struct UpdateResourcePayload {
    pub display_name: Option<String>,
    pub version: Option<String>,
    pub application_ids: Option<Vec<String>>,
    pub attachments: Option<Vec<ResourceAttachment>>,
}
