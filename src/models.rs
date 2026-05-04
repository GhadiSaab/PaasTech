use serde::{Deserialize, Serialize};
use utoipa::ToSchema;

// Resource models
#[derive(Serialize, Deserialize, ToSchema)]
pub struct Resource {
    pub id: String,
    pub display_name: String,
    pub name: String,
    pub version: String,
    pub status: String,
    pub application_ids: Vec<String>,
}

#[derive(Deserialize, ToSchema)]
pub struct CreateResourcePayload {
    pub display_name: String,
    pub name: String,
    pub version: String,
    pub application_id: Option<String>,
}

#[derive(Deserialize, ToSchema)]
pub struct UpdateResourcePayload {
    pub display_name: Option<String>,
    pub version: Option<String>,
    pub application_ids: Option<Vec<String>>,
}
