use actix_web::{App, HttpResponse, HttpServer, Responder, get, post, web};

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
async fn upload_app(req_body: String) -> impl Responder {
    HttpResponse::Ok()
}

#[actix_web::main]
async fn main() -> std::io::Result<()> {
    const IP: &str = "127.0.0.1";
    const PORT: u16 = 8080;

    println!("Running on http://{}:{}", IP, PORT);
    HttpServer::new(|| {
        App::new()
            .service(hello)
            .service(upload_app)
    })
    .bind((IP, PORT))?
    .run()
    .await
}
