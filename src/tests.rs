use actix_web::test;
use actix_web::{App, web};
use reqwest::Client;
use serde_json::json;
use sqlx::PgPool;
use uuid::Uuid;

use crate::handlers;
use crate::registry::Registry;
use crate::scheduler::Scheduler;

fn build_scheduler() -> web::Data<Scheduler> {
    web::Data::new(Scheduler::new())
}

fn build_client() -> web::Data<Client> {
    web::Data::new(Client::new())
}

async fn build_pool() -> web::Data<PgPool> {
    dotenvy::dotenv().ok();
    let url = std::env::var("DATABASE_URL")
        .unwrap_or_else(|_| "postgres://paastech:paastech@localhost:5433/paastech".to_string());
    let pool = PgPool::connect(&url)
        .await
        .expect("Failed to connect to test DB");
    sqlx::query("SELECT pg_advisory_lock(747070)")
        .execute(&pool)
        .await
        .expect("Failed to lock test schema setup");
    sqlx::query(
        r#"
        CREATE TABLE IF NOT EXISTS process_env_vars (
            process_id UUID NOT NULL REFERENCES application_processes(id) ON DELETE CASCADE,
            key VARCHAR(255) NOT NULL,
            value TEXT NOT NULL,
            PRIMARY KEY (process_id, key)
        )
        "#,
    )
    .execute(&pool)
    .await
    .expect("Failed to prepare test process_env_vars table");
    sqlx::query("SELECT pg_advisory_unlock(747070)")
        .execute(&pool)
        .await
        .expect("Failed to unlock test schema setup");
    web::Data::new(pool)
}

async fn cleanup_app(pool: &PgPool, name: &str) {
    sqlx::query("DELETE FROM applications WHERE name = $1")
        .bind(name)
        .execute(pool)
        .await
        .ok();
}

async fn insert_test_app_process(pool: &PgPool, name: &str, status: &str) -> Uuid {
    cleanup_app(pool, name).await;
    let app = Registry::save_in_project(pool, crate::registry::DEFAULT_PROJECT_ID, name, None)
        .await
        .expect("Failed to insert test app");
    let process = Registry::create_process(
        pool,
        app.id,
        "web",
        "web",
        ".",
        None,
        json!({}),
        Some(8080),
        status,
    )
    .await
    .expect("Failed to insert test process");
    process.id
}

async fn cleanup_resource_by_display_name(pool: &PgPool, display_name: &str) {
    sqlx::query("DELETE FROM services WHERE display_name = $1")
        .bind(display_name)
        .execute(pool)
        .await
        .ok();
}

async fn insert_test_resource(
    pool: &PgPool,
    display_name: &str,
    service_name: &str,
    version: &str,
) -> Uuid {
    cleanup_resource_by_display_name(pool, display_name).await;
    let id = Uuid::new_v4();
    sqlx::query(
        "INSERT INTO services (id, display_name, name, version, status) VALUES ($1, $2, $3, $4, 'stopped')",
    )
    .bind(id)
    .bind(display_name)
    .bind(service_name)
    .bind(version)
    .execute(pool)
    .await
    .expect("Failed to insert test resource");
    id
}

async fn cleanup_resource(pool: &PgPool, id: Uuid) {
    sqlx::query("DELETE FROM services WHERE id = $1")
        .bind(id)
        .execute(pool)
        .await
        .ok();
}

// ── App tests ────────────────────────────────────────────────────────────────

#[actix_web::test]
async fn test_db_connection() {
    let pool = build_pool().await;
    let result = sqlx::query("SELECT 1").fetch_one(pool.get_ref()).await;
    assert!(
        result.is_ok(),
        "Database connection failed: {:?}",
        result.err()
    );
}

#[actix_web::test]
async fn test_list_apps() {
    let pool = build_pool().await;
    let app = test::init_service(
        App::new()
            .app_data(build_scheduler())
            .app_data(pool)
            .service(web::scope("/app").service(handlers::apps::list)),
    )
    .await;

    let req = test::TestRequest::get().uri("/app").to_request();
    let resp = test::call_service(&app, req).await;
    assert_eq!(resp.status(), 200);
}

#[actix_web::test]
async fn test_stop_app() {
    let scheduler = build_scheduler();
    let pool = build_pool().await;
    let app_name = "test-stop-app";

    insert_test_app_process(pool.get_ref(), app_name, "running").await;

    let app = test::init_service(
        App::new()
            .app_data(scheduler)
            .app_data(pool.clone())
            .service(web::scope("/app").service(handlers::apps::stop)),
    )
    .await;

    let req = test::TestRequest::post()
        .uri(&format!("/app/{}/stop", app_name))
        .to_request();
    let resp = test::call_service(&app, req).await;
    assert_eq!(resp.status(), 200);

    cleanup_app(pool.get_ref(), app_name).await;
}

#[actix_web::test]
async fn test_stop_app_not_found() {
    let scheduler = build_scheduler();
    let pool = build_pool().await;

    let app = test::init_service(
        App::new()
            .app_data(scheduler)
            .app_data(pool)
            .service(web::scope("/app").service(handlers::apps::stop)),
    )
    .await;

    let req = test::TestRequest::post()
        .uri("/app/nonexistent-app-xyz/stop")
        .to_request();
    let resp = test::call_service(&app, req).await;
    assert_eq!(resp.status(), 404);
}

#[actix_web::test]
async fn test_restart_app() {
    let scheduler = build_scheduler();
    let pool = build_pool().await;
    let app_name = "test-restart-app";

    insert_test_app_process(pool.get_ref(), app_name, "running").await;

    let app = test::init_service(
        App::new()
            .app_data(scheduler.clone())
            .app_data(pool.clone())
            .service(web::scope("/app").service(handlers::apps::restart)),
    )
    .await;

    let req = test::TestRequest::post()
        .uri(&format!("/app/{}/restart", app_name))
        .to_request();
    let resp = test::call_service(&app, req).await;
    assert_eq!(resp.status(), 400);

    cleanup_app(pool.get_ref(), app_name).await;
}

#[actix_web::test]
async fn test_restart_app_not_found() {
    let scheduler = build_scheduler();
    let pool = build_pool().await;

    let app = test::init_service(
        App::new()
            .app_data(scheduler)
            .app_data(pool)
            .service(web::scope("/app").service(handlers::apps::restart)),
    )
    .await;

    let req = test::TestRequest::post()
        .uri("/app/nonexistent-app-xyz/restart")
        .to_request();
    let resp = test::call_service(&app, req).await;
    assert_eq!(resp.status(), 404);
}

#[actix_web::test]
async fn test_status_app() {
    let scheduler = build_scheduler();
    let pool = build_pool().await;
    let app_name = "test-status-app";

    insert_test_app_process(pool.get_ref(), app_name, "running").await;

    let app = test::init_service(
        App::new()
            .app_data(scheduler.clone())
            .app_data(pool.clone())
            .service(web::scope("/app").service(handlers::apps::status)),
    )
    .await;

    let req = test::TestRequest::get()
        .uri(&format!("/app/{}/status", app_name))
        .to_request();
    let resp = test::call_service(&app, req).await;
    assert_eq!(resp.status(), 200);

    let body = test::read_body(resp).await;
    let status_str = String::from_utf8(body.to_vec()).unwrap();
    assert_eq!(status_str, "running");

    cleanup_app(pool.get_ref(), app_name).await;
}

#[actix_web::test]
async fn test_status_app_crashed_is_derived_from_process() {
    let scheduler = build_scheduler();
    let pool = build_pool().await;
    let app_name = "test-status-crashed-app";

    insert_test_app_process(pool.get_ref(), app_name, "crashed").await;

    let app = test::init_service(
        App::new()
            .app_data(scheduler.clone())
            .app_data(pool.clone())
            .service(web::scope("/app").service(handlers::apps::status)),
    )
    .await;

    let req = test::TestRequest::get()
        .uri(&format!("/app/{}/status", app_name))
        .to_request();
    let resp = test::call_service(&app, req).await;
    assert_eq!(resp.status(), 200);
    let body = test::read_body(resp).await;
    let status_str = String::from_utf8(body.to_vec()).unwrap();
    assert_eq!(status_str, "crashed");

    cleanup_app(pool.get_ref(), app_name).await;
}

#[actix_web::test]
async fn test_status_app_failed_is_derived_from_process() {
    let scheduler = build_scheduler();
    let pool = build_pool().await;
    let app_name = "test-status-failed-app";

    insert_test_app_process(pool.get_ref(), app_name, "failed").await;

    let app = test::init_service(
        App::new()
            .app_data(scheduler.clone())
            .app_data(pool.clone())
            .service(web::scope("/app").service(handlers::apps::status)),
    )
    .await;

    let req = test::TestRequest::get()
        .uri(&format!("/app/{}/status", app_name))
        .to_request();
    let resp = test::call_service(&app, req).await;
    assert_eq!(resp.status(), 200);
    let body = test::read_body(resp).await;
    let status_str = String::from_utf8(body.to_vec()).unwrap();
    assert_eq!(status_str, "failed");

    cleanup_app(pool.get_ref(), app_name).await;
}

#[actix_web::test]
async fn test_status_app_building_is_derived_from_process() {
    let scheduler = build_scheduler();
    let pool = build_pool().await;
    let app_name = "test-status-building-app";

    insert_test_app_process(pool.get_ref(), app_name, "building").await;

    let app = test::init_service(
        App::new()
            .app_data(scheduler.clone())
            .app_data(pool.clone())
            .service(web::scope("/app").service(handlers::apps::status)),
    )
    .await;

    let req = test::TestRequest::get()
        .uri(&format!("/app/{}/status", app_name))
        .to_request();
    let resp = test::call_service(&app, req).await;
    assert_eq!(resp.status(), 200);
    let body = test::read_body(resp).await;
    let status_str = String::from_utf8(body.to_vec()).unwrap();
    assert_eq!(status_str, "building");

    cleanup_app(pool.get_ref(), app_name).await;
}

#[actix_web::test]
async fn test_status_app_not_found() {
    let scheduler = build_scheduler();
    let pool = build_pool().await;

    let app = test::init_service(
        App::new()
            .app_data(scheduler)
            .app_data(pool)
            .service(web::scope("/app").service(handlers::apps::status)),
    )
    .await;

    let req = test::TestRequest::get()
        .uri("/app/nonexistent-app-xyz/status")
        .to_request();
    let resp = test::call_service(&app, req).await;
    assert_eq!(resp.status(), 404);
}

// ── Resource tests ────────────────────────────────────────────────────────────

#[actix_web::test]
async fn test_get_resources() {
    let pool = build_pool().await;
    let app = test::init_service(
        App::new()
            .app_data(pool)
            .service(web::scope("/resource").service(handlers::resources::list)),
    )
    .await;

    let req = test::TestRequest::get().uri("/resource").to_request();
    let resp = test::call_service(&app, req).await;
    assert_eq!(resp.status(), 200);
}

#[actix_web::test]
async fn test_create_resource_invalid_service() {
    let pool = build_pool().await;

    let app = test::init_service(
        App::new()
            .app_data(pool)
            .app_data(build_client())
            .app_data(build_scheduler())
            .service(web::scope("/resource").service(handlers::resources::create)),
    )
    .await;

    let req = test::TestRequest::post()
        .uri("/resource")
        .set_json(json!({"display_name": "My DB", "name": "mysql", "version": "8.0"}))
        .to_request();
    let resp = test::call_service(&app, req).await;
    assert_eq!(resp.status(), 400);
}

#[actix_web::test]
async fn test_get_resource_not_found() {
    let pool = build_pool().await;

    let app = test::init_service(App::new().app_data(pool).service(handlers::resources::get)).await;

    let req = test::TestRequest::get()
        .uri(&format!("/resource/{}", Uuid::new_v4()))
        .to_request();
    let resp = test::call_service(&app, req).await;
    assert_eq!(resp.status(), 404);
}

#[actix_web::test]
async fn test_get_resource_invalid_uuid() {
    let pool = build_pool().await;

    let app = test::init_service(App::new().app_data(pool).service(handlers::resources::get)).await;

    let req = test::TestRequest::get()
        .uri("/resource/not-a-uuid")
        .to_request();
    let resp = test::call_service(&app, req).await;
    assert_eq!(resp.status(), 400);
}

#[actix_web::test]
async fn test_get_resource() {
    let pool = build_pool().await;
    let id = insert_test_resource(pool.get_ref(), "Test Postgres", "postgres", "16").await;

    let app = test::init_service(
        App::new()
            .app_data(pool.clone())
            .service(handlers::resources::get),
    )
    .await;

    let req = test::TestRequest::get()
        .uri(&format!("/resource/{}", id))
        .to_request();
    let resp = test::call_service(&app, req).await;
    assert_eq!(resp.status(), 200);

    let body: serde_json::Value = test::read_body_json(resp).await;
    assert_eq!(body["name"], "postgres");
    assert_eq!(body["version"], "16");

    cleanup_resource(pool.get_ref(), id).await;
}

#[actix_web::test]
async fn test_update_resource_display_name() {
    let pool = build_pool().await;
    let id = insert_test_resource(pool.get_ref(), "Old Name", "redis", "7").await;

    let app = test::init_service(
        App::new()
            .app_data(pool.clone())
            .app_data(build_client())
            .app_data(build_scheduler())
            .service(handlers::resources::update),
    )
    .await;

    let req = test::TestRequest::patch()
        .uri(&format!("/resource/{}", id))
        .set_json(json!({"display_name": "New Name"}))
        .to_request();
    let resp = test::call_service(&app, req).await;
    assert_eq!(resp.status(), 200);

    cleanup_resource(pool.get_ref(), id).await;
}

#[actix_web::test]
async fn test_update_resource_not_found() {
    let pool = build_pool().await;

    let app = test::init_service(
        App::new()
            .app_data(pool)
            .app_data(build_client())
            .app_data(build_scheduler())
            .service(handlers::resources::update),
    )
    .await;

    let req = test::TestRequest::patch()
        .uri(&format!("/resource/{}", Uuid::new_v4()))
        .set_json(json!({"display_name": "Ghost"}))
        .to_request();
    let resp = test::call_service(&app, req).await;
    assert_eq!(resp.status(), 404);
}

#[actix_web::test]
async fn test_delete_resource() {
    let pool = build_pool().await;
    let id = insert_test_resource(pool.get_ref(), "To Delete", "postgres", "15").await;

    let app = test::init_service(
        App::new()
            .app_data(pool)
            .app_data(build_client())
            .app_data(build_scheduler())
            .service(handlers::resources::delete),
    )
    .await;

    let req = test::TestRequest::delete()
        .uri(&format!("/resource/{}", id))
        .to_request();
    let resp = test::call_service(&app, req).await;
    assert_eq!(resp.status(), 204);
}

#[actix_web::test]
async fn test_delete_running_resource_removes_container() {
    let pool = build_pool().await;
    let scheduler = build_scheduler();
    let id = Uuid::new_v4();

    cleanup_resource_by_display_name(pool.get_ref(), "Running Redis").await;

    let (container_id, host_port) = scheduler
        .start_service(
            &id.to_string(),
            crate::registry::DEFAULT_PROJECT_NETWORK,
            "redis:7",
            6379,
            None,
            vec![],
            vec![],
        )
        .await
        .unwrap();

    sqlx::query(
        "INSERT INTO services (id, display_name, name, version, container_id, port, status) VALUES ($1, 'Running Redis', 'redis', '7', $2, $3, 'running')",
    )
    .bind(id)
    .bind(container_id)
    .bind(host_port as i32)
    .execute(pool.get_ref())
    .await
    .unwrap();

    let app = test::init_service(
        App::new()
            .app_data(pool.clone())
            .app_data(build_client())
            .app_data(scheduler.clone())
            .service(handlers::resources::delete),
    )
    .await;

    let req = test::TestRequest::delete()
        .uri(&format!("/resource/{}", id))
        .to_request();
    let resp = test::call_service(&app, req).await;
    assert_eq!(resp.status(), 204);
    assert_eq!(scheduler.inspect(&id.to_string()).await, "unknown");

    cleanup_resource(pool.get_ref(), id).await;
}

#[actix_web::test]
async fn test_delete_resource_not_found() {
    let pool = build_pool().await;

    let app = test::init_service(
        App::new()
            .app_data(pool)
            .app_data(build_client())
            .app_data(build_scheduler())
            .service(handlers::resources::delete),
    )
    .await;

    let req = test::TestRequest::delete()
        .uri(&format!("/resource/{}", Uuid::new_v4()))
        .to_request();
    let resp = test::call_service(&app, req).await;
    assert_eq!(resp.status(), 404);
}

#[actix_web::test]
async fn test_get_resource_env() {
    let pool = build_pool().await;
    let id = insert_test_resource(pool.get_ref(), "Env Test", "redis", "7").await;

    let app = test::init_service(
        App::new()
            .app_data(pool.clone())
            .service(handlers::resources::get_env),
    )
    .await;

    let req = test::TestRequest::get()
        .uri(&format!("/resource/{}/env", id))
        .to_request();
    let resp = test::call_service(&app, req).await;
    assert_eq!(resp.status(), 200);

    let body: serde_json::Value = test::read_body_json(resp).await;
    assert!(body.is_object());

    cleanup_resource(pool.get_ref(), id).await;
}

#[actix_web::test]
async fn test_update_and_get_resource_env() {
    let pool = build_pool().await;
    let id = insert_test_resource(pool.get_ref(), "Env Update Test", "postgres", "16").await;

    let app = test::init_service(
        App::new()
            .app_data(pool.clone())
            .service(handlers::resources::update_env)
            .service(handlers::resources::get_env),
    )
    .await;

    let req = test::TestRequest::put()
        .uri(&format!("/resource/{}/env", id))
        .set_json(json!({"MY_VAR": "hello", "OTHER": "world"}))
        .to_request();
    let resp = test::call_service(&app, req).await;
    assert_eq!(resp.status(), 200);

    let req = test::TestRequest::get()
        .uri(&format!("/resource/{}/env", id))
        .to_request();
    let resp = test::call_service(&app, req).await;
    let body: serde_json::Value = test::read_body_json(resp).await;
    assert_eq!(body["MY_VAR"], "hello");
    assert_eq!(body["OTHER"], "world");

    cleanup_resource(pool.get_ref(), id).await;
}

#[actix_web::test]
async fn test_stop_resource_already_stopped() {
    let pool = build_pool().await;
    let scheduler = build_scheduler();
    let id = insert_test_resource(pool.get_ref(), "Already Stopped", "redis", "7").await;

    let app = test::init_service(
        App::new()
            .app_data(pool.clone())
            .app_data(scheduler)
            .service(handlers::resources::stop),
    )
    .await;

    let req = test::TestRequest::post()
        .uri(&format!("/resource/{}/stop", id))
        .to_request();
    let resp = test::call_service(&app, req).await;
    assert_eq!(resp.status(), 409);

    cleanup_resource(pool.get_ref(), id).await;
}

#[actix_web::test]
async fn test_start_resource_already_running() {
    let pool = build_pool().await;
    let scheduler = build_scheduler();
    let id = Uuid::new_v4();

    cleanup_resource_by_display_name(pool.get_ref(), "Running Redis").await;
    sqlx::query(
        "INSERT INTO services (id, display_name, name, version, status) VALUES ($1, 'Running Redis', 'redis', '7', 'running')",
    )
    .bind(id)
    .execute(pool.get_ref())
    .await
    .unwrap();

    let app = test::init_service(
        App::new()
            .app_data(pool.clone())
            .app_data(scheduler)
            .service(handlers::resources::start),
    )
    .await;

    let req = test::TestRequest::post()
        .uri(&format!("/resource/{}/start", id))
        .to_request();
    let resp = test::call_service(&app, req).await;
    assert_eq!(resp.status(), 409);

    cleanup_resource(pool.get_ref(), id).await;
}

// ── Delete app tests ──────────────────────────────────────────────────────────

#[actix_web::test]
async fn test_delete_app() {
    let scheduler = build_scheduler();
    let pool = build_pool().await;
    let app_name = "test-delete-app";

    insert_test_app_process(pool.get_ref(), app_name, "stopped").await;

    let app = test::init_service(
        App::new()
            .app_data(scheduler)
            .app_data(pool.clone())
            .service(web::scope("/app").service(handlers::apps::delete)),
    )
    .await;

    let req = test::TestRequest::delete()
        .uri(&format!("/app/{}", app_name))
        .to_request();
    let resp = test::call_service(&app, req).await;
    assert_eq!(resp.status(), 204);
}

#[actix_web::test]
async fn test_delete_app_not_found() {
    let scheduler = build_scheduler();
    let pool = build_pool().await;

    let app = test::init_service(
        App::new()
            .app_data(scheduler)
            .app_data(pool)
            .service(web::scope("/app").service(handlers::apps::delete)),
    )
    .await;

    let req = test::TestRequest::delete()
        .uri("/app/nonexistent-app-xyz")
        .to_request();
    let resp = test::call_service(&app, req).await;
    assert_eq!(resp.status(), 404);
}

// ── App env tests ──────────────────────────────────────────────────────────────

#[actix_web::test]
async fn test_get_app_env_not_found() {
    let pool = build_pool().await;

    let app = test::init_service(
        App::new()
            .app_data(pool)
            .service(web::scope("/app").service(handlers::apps::get_env)),
    )
    .await;

    let req = test::TestRequest::get()
        .uri("/app/nonexistent-app-xyz/process/web/env")
        .to_request();
    let resp = test::call_service(&app, req).await;
    assert_eq!(resp.status(), 404);
}

#[actix_web::test]
async fn test_get_app_env() {
    let pool = build_pool().await;
    let app_name = "test-get-app-env";

    insert_test_app_process(pool.get_ref(), app_name, "running").await;

    let app = test::init_service(
        App::new()
            .app_data(pool.clone())
            .service(web::scope("/app").service(handlers::apps::get_env)),
    )
    .await;

    let req = test::TestRequest::get()
        .uri(&format!("/app/{}/process/web/env", app_name))
        .to_request();
    let resp = test::call_service(&app, req).await;
    assert_eq!(resp.status(), 200);

    let body: serde_json::Value = test::read_body_json(resp).await;
    assert!(body.is_object());

    cleanup_app(pool.get_ref(), app_name).await;
}

#[actix_web::test]
async fn test_update_app_env_not_found() {
    let pool = build_pool().await;

    let app = test::init_service(
        App::new()
            .app_data(pool)
            .service(web::scope("/app").service(handlers::apps::update_env)),
    )
    .await;

    let req = test::TestRequest::put()
        .uri("/app/nonexistent-app-xyz/process/web/env")
        .set_json(json!({"MY_VAR": "value"}))
        .to_request();
    let resp = test::call_service(&app, req).await;
    assert_eq!(resp.status(), 404);
}

#[actix_web::test]
async fn test_update_and_get_app_env() {
    let pool = build_pool().await;
    let app_name = "test-update-app-env";

    insert_test_app_process(pool.get_ref(), app_name, "running").await;

    let app = test::init_service(
        App::new().app_data(pool.clone()).service(
            web::scope("/app")
                .service(handlers::apps::update_env)
                .service(handlers::apps::get_env),
        ),
    )
    .await;

    let req = test::TestRequest::put()
        .uri(&format!("/app/{}/process/web/env", app_name))
        .set_json(json!({"FOO": "bar", "BAZ": "qux"}))
        .to_request();
    let resp = test::call_service(&app, req).await;
    assert_eq!(resp.status(), 200);

    let req = test::TestRequest::get()
        .uri(&format!("/app/{}/process/web/env", app_name))
        .to_request();
    let resp = test::call_service(&app, req).await;
    let body: serde_json::Value = test::read_body_json(resp).await;
    assert_eq!(body["FOO"], "bar");
    assert_eq!(body["BAZ"], "qux");

    cleanup_app(pool.get_ref(), app_name).await;
}

// ── Logs tests ────────────────────────────────────────────────────────────────

#[actix_web::test]
async fn test_logs_app_not_found() {
    let scheduler = build_scheduler();
    let pool = build_pool().await;

    let app = test::init_service(
        App::new()
            .app_data(scheduler)
            .app_data(pool)
            .service(web::scope("/app").service(handlers::apps::logs)),
    )
    .await;

    let req = test::TestRequest::get()
        .uri("/app/nonexistent-app-xyz/logs")
        .to_request();
    let resp = test::call_service(&app, req).await;
    assert_eq!(resp.status(), 404);
}

#[actix_web::test]
async fn test_logs_app_no_container() {
    let scheduler = build_scheduler();
    let pool = build_pool().await;
    let app_name = "test-logs-app-no-container";

    insert_test_app_process(pool.get_ref(), app_name, "stopped").await;

    let app = test::init_service(
        App::new()
            .app_data(scheduler)
            .app_data(pool.clone())
            .service(web::scope("/app").service(handlers::apps::logs)),
    )
    .await;

    let req = test::TestRequest::get()
        .uri(&format!("/app/{}/logs", app_name))
        .to_request();
    let resp = test::call_service(&app, req).await;
    assert_eq!(resp.status(), 500);

    cleanup_app(pool.get_ref(), app_name).await;
}

#[actix_web::test]
async fn test_logs_resource_invalid_uuid() {
    let scheduler = build_scheduler();
    let pool = build_pool().await;

    let app = test::init_service(
        App::new()
            .app_data(scheduler)
            .app_data(pool)
            .service(handlers::resources::logs),
    )
    .await;

    let req = test::TestRequest::get()
        .uri("/resource/not-a-uuid/logs")
        .to_request();
    let resp = test::call_service(&app, req).await;
    assert_eq!(resp.status(), 400);
}

#[actix_web::test]
async fn test_logs_resource_not_found() {
    let scheduler = build_scheduler();
    let pool = build_pool().await;

    let app = test::init_service(
        App::new()
            .app_data(scheduler)
            .app_data(pool)
            .service(handlers::resources::logs),
    )
    .await;

    let req = test::TestRequest::get()
        .uri(&format!("/resource/{}/logs", Uuid::new_v4()))
        .to_request();
    let resp = test::call_service(&app, req).await;
    assert_eq!(resp.status(), 404);
}

#[actix_web::test]
async fn test_logs_resource_no_container() {
    let scheduler = build_scheduler();
    let pool = build_pool().await;
    let id = insert_test_resource(pool.get_ref(), "Logs Test", "redis", "7").await;

    let app = test::init_service(
        App::new()
            .app_data(scheduler)
            .app_data(pool.clone())
            .service(handlers::resources::logs),
    )
    .await;

    let req = test::TestRequest::get()
        .uri(&format!("/resource/{}/logs", id))
        .to_request();
    let resp = test::call_service(&app, req).await;
    assert_eq!(resp.status(), 500);

    cleanup_resource(pool.get_ref(), id).await;
}
