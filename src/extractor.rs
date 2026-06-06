use actix_web::{Error, FromRequest, HttpRequest, error, web};
use actix_web::dev::Payload;
use sqlx::PgPool;
use std::future::Future;
use std::pin::Pin;

use crate::registry::{Project, Registry};

pub struct ProjectScope {
    pub project: Project,
    pub pool: web::Data<PgPool>,
}

impl FromRequest for ProjectScope {
    type Error = Error;
    type Future = Pin<Box<dyn Future<Output = Result<Self, Self::Error>>>>;

    fn from_request(req: &HttpRequest, _payload: &mut Payload) -> Self::Future {
        let pool = req.app_data::<web::Data<PgPool>>().cloned();
        let project_name = req.match_info().get("project").map(str::to_string);

        Box::pin(async move {
            let pool = pool
                .ok_or_else(|| error::ErrorInternalServerError("pool not configured"))?;
            let project = if let Some(name) = project_name {
                Registry::get_project(&pool, &name)
                    .await
                    .map_err(error::ErrorInternalServerError)?
                    .ok_or_else(|| error::ErrorNotFound("Project not found"))?
            } else {
                Registry::ensure_default_project(&pool)
                    .await
                    .map_err(error::ErrorInternalServerError)?
            };
            Ok(ProjectScope { project, pool })
        })
    }
}
