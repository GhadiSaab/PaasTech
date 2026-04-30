use actix_web::{App, HttpResponse, HttpServer, Responder, get, post};

/* boilerplates for reference */
#[get("/")]
async fn hello() -> impl Responder {
    HttpResponse::Ok().body("Hello world!")
}

#[post("/echo")]
async fn echo(req_body: String) -> impl Responder {
    HttpResponse::Ok().body(req_body)
}

// real stuff

#[post("/app/upload")]
async fn upload_app(_req_body: String) -> impl Responder {
    HttpResponse::Ok()
}

#[actix_web::main]
async fn main() -> std::io::Result<()> {
    dotenvy::dotenv().ok();

    let host: String = std::env::var("HOST").unwrap_or_else(|_| "127.0.0.1".to_string());
    let port: u16 = std::env::var("PORT")
        .unwrap_or_else(|_| "8080".to_string())
        .parse()
        .expect("PORT must be a valid number");

    println!("Running on http://{}:{}", host, port);
    HttpServer::new(|| {
        App::new()
            .service(hello)
            .service(upload_app)
    })
    .bind((host, port))?
    .run()
    .await
}
