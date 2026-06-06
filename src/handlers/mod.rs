pub mod apps;
pub mod projects;
pub mod resources;
pub mod services;

use actix_web::web;

pub fn configure(cfg: &mut web::ServiceConfig) {
    cfg.service(services::get_service_versions)
        .service(projects::create)
        .service(projects::list)
        .service(projects::get)
        .service(projects::delete)
        .service(projects::get_env)
        .service(projects::update_env)
        // App handlers registered under two scope prefixes (ProjectScope handles routing)
        .service(
            web::scope("/app")
                .service(apps::upload)
                .service(apps::list)
                .service(apps::deploy)
                .service(apps::update)
                .service(apps::stop)
                .service(apps::restart)
                .service(apps::status)
                .service(apps::delete)
                .service(apps::logs)
                .service(apps::get_env)
                .service(apps::set_env)
                .service(apps::update_env),
        )
        .service(
            web::scope("/project/{project}/app")
                .service(apps::upload)
                .service(apps::list)
                .service(apps::deploy)
                .service(apps::update)
                .service(apps::stop)
                .service(apps::restart)
                .service(apps::status)
                .service(apps::delete)
                .service(apps::logs)
                .service(apps::get_env)
                .service(apps::set_env)
                .service(apps::update_env),
        )
        // Resource create/list scoped by project
        .service(
            web::scope("/resource")
                .service(resources::create)
                .service(resources::list),
        )
        .service(
            web::scope("/project/{project}/resource")
                .service(resources::create)
                .service(resources::list),
        )
        // Per-resource operations (UUID-addressed, no project scope)
        .service(resources::get)
        .service(resources::update)
        .service(resources::delete)
        .service(resources::start)
        .service(resources::stop)
        .service(resources::logs)
        .service(resources::get_env)
        .service(resources::update_env);
}
