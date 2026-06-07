use actix_web::{Error, HttpResponse, Responder, error, get, web};
use reqwest::Client;

use crate::docker::{fetch_service_versions, is_valid_service, valid_services};

#[utoipa::path(
    get,
    path = "/service/{name}/versions",
    params(("name" = String, Path, description = "Service name (postgres, redis, s3)")),
    responses(
        (status = 200, description = "Available versions", body = Vec<String>),
        (status = 400, description = "Invalid service name"),
        (status = 500, description = "Internal server error"),
    ),
    tag = "services"
)]
#[get("/service/{name}/versions")]
pub async fn get_service_versions(
    client: web::Data<Client>,
    name: web::Path<String>,
) -> Result<impl Responder, Error> {
    if !is_valid_service(&name) {
        return Err(error::ErrorBadRequest(format!(
            "Invalid service name '{}'. Must be one of: {}",
            name,
            valid_services().join(", ")
        )));
    }
    let versions = fetch_service_versions(&client, &name).await?;
    Ok(HttpResponse::Ok().json(versions))
}
