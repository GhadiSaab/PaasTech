use axum::{
    routing::{get, post},
    Router,
};
use tokio::net::TcpListener;

async fn hello() -> &'static str {
    "Hello world!"
}

async fn echo(body: String) -> String {
    body
}

async fn manual_hello() -> &'static str {
    "Hey there!"
}

#[tokio::main]
async fn main() {
    let app = Router::new()
        .route("/", get(hello))
        .route("/echo", post(echo))
        .route("/hey", get(manual_hello));

    let listener = TcpListener::bind("127.0.0.1:8080")
        .await
        .unwrap();

    println!("listening on {}", listener.local_addr().unwrap());

    axum::serve(listener, app).await.unwrap();
}
