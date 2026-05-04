use super::*;
use actix_web::test;
use serde_json::json;

fn build_scheduler() -> web::Data<Scheduler> {
    web::Data::new(Scheduler::new())
}

#[actix_web::test]
async fn test_list_apps() {
    let app = test::init_service(
        App::new()
            .app_data(build_scheduler())
            .service(list_apps),
    )
    .await;

    let req = test::TestRequest::get().uri("/app/list").to_request();
    let resp = test::call_service(&app, req).await;
    assert_eq!(resp.status(), 200);
}

#[actix_web::test]
async fn test_deploy_returns_container_id() {
    let scheduler = build_scheduler();
    let app = test::init_service(
        App::new()
            .app_data(scheduler.clone())
            .service(deploy_app)
            .service(stop_app),
    )
    .await;

    let req = test::TestRequest::post()
        .uri("/app/deploy")
        .set_json(json!({"container_id": "", "image": "hello-world"}))
        .to_request();
    let resp = test::call_service(&app, req).await;
    assert_eq!(resp.status(), 200);

    let body = test::read_body(resp).await;
    let container_id = String::from_utf8(body.to_vec()).unwrap();
    assert!(!container_id.is_empty());

    let req = test::TestRequest::post()
        .uri(&format!("/app/{}/stop", container_id))
        .to_request();
    test::call_service(&app, req).await;
}

#[actix_web::test]
async fn test_stop_app() {
    let scheduler = build_scheduler();
    let container_id = scheduler.deploy("hello-world").await;

    let app = test::init_service(
        App::new()
            .app_data(scheduler)
            .service(stop_app),
    )
    .await;

    let req = test::TestRequest::post()
        .uri(&format!("/app/{}/stop", container_id))
        .to_request();
    let resp = test::call_service(&app, req).await;
    assert_eq!(resp.status(), 200);
}

#[actix_web::test]
async fn test_restart_app() {
    let scheduler = build_scheduler();
    let container_id = scheduler.deploy("hello-world").await;

    let app = test::init_service(
        App::new()
            .app_data(scheduler.clone())
            .service(restart_app)
            .service(stop_app),
    )
    .await;

    let req = test::TestRequest::post()
        .uri(&format!("/app/{}/restart", container_id))
        .to_request();
    let resp = test::call_service(&app, req).await;
    assert_eq!(resp.status(), 200);

    let req = test::TestRequest::post()
        .uri(&format!("/app/{}/stop", container_id))
        .to_request();
    test::call_service(&app, req).await;
}

#[actix_web::test]
async fn test_status_app() {
    let scheduler = build_scheduler();
    let container_id = scheduler.deploy("hello-world").await;

    let app = test::init_service(
        App::new()
            .app_data(scheduler.clone())
            .service(status_app)
            .service(stop_app),
    )
    .await;

    let req = test::TestRequest::get()
        .uri(&format!("/app/{}/status", container_id))
        .to_request();
    let resp = test::call_service(&app, req).await;
    assert_eq!(resp.status(), 200);

    let body = test::read_body(resp).await;
    let status = String::from_utf8(body.to_vec()).unwrap();
    assert!(!status.is_empty());

    let req = test::TestRequest::post()
        .uri(&format!("/app/{}/stop", container_id))
        .to_request();
    test::call_service(&app, req).await;
}
