use actix_web::{
    App, HttpServer, HttpRequest, HttpResponse, Responder, http::header, web, error::ResponseError, get, http, middleware::Logger, Result,
};
use reqwest::Client;
use rust_embed::Embed as RustEmbedTrait;
use rust_embed::RustEmbed;
use mime_guess::MimeGuess;

#[derive(RustEmbed)]
#[folder = "frontend/dist/"]
struct Assets;

async fn serve_index() -> impl Responder {
    HttpResponse::Ok().body("Hello World!")
}

fn content_type(path: &str) -> header::HeaderValue {
    let mime = MimeGuess::from_path(path).first_or_octet_stream();
    header::HeaderValue::from_str(mime.as_ref()).unwrap()
}

async fn frontend(req: HttpRequest) -> Result<HttpResponse> {
    let tail = req.match_info().query("tail");
    let mut path = tail.trim_start_matches('/');

    if path.is_empty() {
        path = "index.html";
    }

    // Try exact asset first
    if let Some(file) = Assets::get(path) {
        let body = file.data.into_owned();

        // For Trunk/Vite-style hashed assets, long caching is fine.
        let cache = if path == "index.html" {
            "no-cache"
        } else {
            "public, max-age=31536000, immutable"
        };

        return Ok(HttpResponse::Ok()
            .insert_header((header::CONTENT_TYPE, content_type(path)))
            .insert_header((header::CACHE_CONTROL, cache))
            .body(body));
    }

    // SPA fallback: serve index.html for unknown routes (client-side routing)
    if let Some(index) = Assets::get("index.html") {
        return Ok(HttpResponse::Ok()
            .insert_header((header::CONTENT_TYPE, header::HeaderValue::from_static("text/html; charset=utf-8")))
            .insert_header((header::CACHE_CONTROL, "no-cache"))
            .body(index.data.into_owned()));
    }

    Err(actix_web::error::ErrorNotFound("asset not found"))
}


#[actix_web::main]
async fn main() -> std::io::Result<()> {

    let port = 8089;

    println!("→ Starting server on http://127.0.0.1:{}", port);

    HttpServer::new(move || {
        App::new()
            .wrap(Logger::default())
            // Serve root index
            .route("/api/health", web::get().to(|| async { "ok" }))
            // Serve static assets
            .route("/{tail:.*}", web::get().to(frontend))
    })
    .bind(("127.0.0.1", port))?
    .run()
    .await
}
