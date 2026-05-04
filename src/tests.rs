use super::*;
use crate::registry::Registry;
use actix_web::test;
use serde_json::json;
use sqlx::PgPool;

fn build_scheduler() -> web::Data<Scheduler> {
    web::Data::new(Scheduler::new())
}

async fn build_pool() -> web::Data<PgPool> {
    dotenvy::dotenv().ok();
    let url = std::env::var("DATABASE_URL")
        .unwrap_or_else(|_| "postgres://paastech:paastech@localhost:5433/paastech".to_string());
    web::Data::new(
        PgPool::connect(&url)
            .await
            .expect("Failed to connect to test DB"),
    )
}

#[actix_web::test]
async fn test_list_apps() {
    let pool = build_pool().await;
    let app = test::init_service(
        App::new()
            .app_data(build_scheduler())
            .app_data(pool)
            .service(list_apps),
    )
    .await;

    let req = test::TestRequest::get().uri("/app").to_request();
    let resp = test::call_service(&app, req).await;
    assert_eq!(resp.status(), 200);
}

#[actix_web::test]
async fn test_deploy_app() {
    let scheduler = build_scheduler();
    let pool = build_pool().await;
    let app_name = "test-deploy-app";

    let app = test::init_service(
        App::new()
            .app_data(scheduler.clone())
            .app_data(pool.clone())
            .service(deploy_app)
            .service(stop_app),
    )
    .await;

    let req = test::TestRequest::post()
        .uri("/app/deploy")
        .set_json(json!({"name": app_name, "image": "hello-world", "port": 8080}))
        .to_request();
    let resp = test::call_service(&app, req).await;
    assert_eq!(resp.status(), 200);

    let req = test::TestRequest::post()
        .uri(&format!("/app/{}/stop", app_name))
        .to_request();
    test::call_service(&app, req).await;
}

#[actix_web::test]
async fn test_stop_app() {
    let scheduler = build_scheduler();
    let pool = build_pool().await;
    let app_name = "test-stop-app";

    if let Some(container_id) = scheduler.deploy(app_name, "hello-world").await {
        Registry::save(
            &pool,
            app_name,
            "hello-world",
            &container_id,
            8081,
            "running",
            None,
        )
        .await
        .ok();
    }

    let app = test::init_service(
        App::new()
            .app_data(scheduler)
            .app_data(pool)
            .service(stop_app),
    )
    .await;

    let req = test::TestRequest::post()
        .uri(&format!("/app/{}/stop", app_name))
        .to_request();
    let resp = test::call_service(&app, req).await;
    assert_eq!(resp.status(), 200);
}

#[actix_web::test]
async fn test_restart_app() {
    let scheduler = build_scheduler();
    let pool = build_pool().await;
    let app_name = "test-restart-app";

    if let Some(container_id) = scheduler.deploy(app_name, "hello-world").await {
        Registry::save(
            &pool,
            app_name,
            "hello-world",
            &container_id,
            8082,
            "running",
            None,
        )
        .await
        .ok();
    }

    let app = test::init_service(
        App::new()
            .app_data(scheduler.clone())
            .app_data(pool.clone())
            .service(restart_app)
            .service(stop_app),
    )
    .await;

    let req = test::TestRequest::post()
        .uri(&format!("/app/{}/restart", app_name))
        .to_request();
    let resp = test::call_service(&app, req).await;
    assert_eq!(resp.status(), 200);

    let req = test::TestRequest::post()
        .uri(&format!("/app/{}/stop", app_name))
        .to_request();
    test::call_service(&app, req).await;
}

#[actix_web::test]
async fn test_status_app() {
    let scheduler = build_scheduler();
    let pool = build_pool().await;
    let app_name = "test-status-app";

    if let Some(container_id) = scheduler.deploy(app_name, "hello-world").await {
        Registry::save(
            &pool,
            app_name,
            "hello-world",
            &container_id,
            8083,
            "running",
            None,
        )
        .await
        .ok();
    }

    let app = test::init_service(
        App::new()
            .app_data(scheduler.clone())
            .app_data(pool.clone())
            .service(status_app)
            .service(stop_app),
    )
    .await;

    let req = test::TestRequest::get()
        .uri(&format!("/app/{}/status", app_name))
        .to_request();
    let resp = test::call_service(&app, req).await;
    assert_eq!(resp.status(), 200);

    let body = test::read_body(resp).await;
    let status = String::from_utf8(body.to_vec()).unwrap();
    assert!(!status.is_empty());

    let req = test::TestRequest::post()
        .uri(&format!("/app/{}/stop", app_name))
        .to_request();
    test::call_service(&app, req).await;
}
