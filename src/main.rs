use actix_web::{
    App, HttpRequest, HttpResponse, HttpServer, Responder,
    http::header, middleware::Logger, web,
};
use rust_embed::RustEmbed;
use mime_guess::MimeGuess;
use serde::Serialize;
use std::path::{Path, PathBuf};
use tokio::process::Command;
use uuid::Uuid;
use walkdir::WalkDir;

// ── Embedded frontend assets ─────────────────────────────────────────────────

#[derive(RustEmbed)]
#[folder = "frontend/dist/"]
struct Assets;

// ── Models ───────────────────────────────────────────────────────────────────

/// Matches the `Element` struct used by the frontend.
#[derive(Clone, Serialize)]
struct VideoItem {
    id: String,
    title: String,
    description: String,
    genre: String,
    tags: Vec<String>,
    rating: f64,
    year: u16,
    duration_secs: u32,
    director: String,
}

// ── App state ────────────────────────────────────────────────────────────────

struct AppState {
    library_path: PathBuf,
    cache_dir: PathBuf,
}

// ── Helpers ──────────────────────────────────────────────────────────────────

/// Stable, deterministic video ID derived from the relative path.
fn video_id(rel_path: &str) -> String {
    Uuid::new_v5(&Uuid::NAMESPACE_URL, rel_path.as_bytes()).to_string()
}

/// Returns `true` for file extensions we treat as video.
fn is_video(path: &Path) -> bool {
    const EXTS: &[&str] = &[
        "mp4", "mkv", "avi", "mov", "webm", "m4v", "flv", "wmv", "ts", "m2ts",
    ];
    path.extension()
        .and_then(|e| e.to_str())
        .map(|e| EXTS.contains(&e.to_lowercase().as_str()))
        .unwrap_or(false)
}

// ── ffprobe metadata ─────────────────────────────────────────────────────────

#[derive(Default)]
struct FfprobeMeta {
    title: Option<String>,
    genre: Option<String>,
    year: Option<u16>,
    director: Option<String>,
}

/// Run `ffprobe` to extract duration (seconds) and embedded tags.
/// Silently returns defaults if `ffprobe` is not installed.
async fn probe_video(path: &Path) -> (u32, FfprobeMeta) {
    let output = Command::new("ffprobe")
        .args([
            "-v",
            "quiet",
            "-print_format",
            "json",
            "-show_entries",
            "format=duration:format_tags=title,genre,date,artist,director",
            path.to_str().unwrap_or(""),
        ])
        .output()
        .await;

    let Ok(output) = output else {
        return (0, FfprobeMeta::default());
    };

    let json: serde_json::Value =
        serde_json::from_slice(&output.stdout).unwrap_or(serde_json::Value::Null);

    let duration = json["format"]["duration"]
        .as_str()
        .and_then(|s| s.parse::<f64>().ok())
        .map(|d| d as u32)
        .unwrap_or(0);

    let tags = &json["format"]["tags"];
    let meta = FfprobeMeta {
        title: tags["title"].as_str().map(str::to_owned),
        genre: tags["genre"].as_str().map(str::to_owned),
        year: tags["date"]
            .as_str()
            .and_then(|s| s.get(..4))
            .and_then(|s| s.parse().ok()),
        director: tags["director"]
            .as_str()
            .or_else(|| tags["artist"].as_str())
            .map(str::to_owned),
    };

    (duration, meta)
}

// ── Library scanning ─────────────────────────────────────────────────────────

async fn scan_library(library_path: &Path) -> Vec<VideoItem> {
    let entries: Vec<_> = WalkDir::new(library_path)
        .into_iter()
        .filter_map(|e| e.ok())
        .filter(|e| e.file_type().is_file() && is_video(e.path()))
        .collect();

    let mut items = Vec::new();
    for entry in entries {
        let abs = entry.path().to_path_buf();
        let rel = abs
            .strip_prefix(library_path)
            .unwrap_or(&abs)
            .to_string_lossy()
            .to_string();

        // Humanise filename as a fallback title
        let fallback_title = abs
            .file_stem()
            .and_then(|s| s.to_str())
            .unwrap_or("Unknown")
            .replace(['.', '_', '-'], " ");

        let id = video_id(&rel);
        let (duration_secs, meta) = probe_video(&abs).await;

        items.push(VideoItem {
            id,
            title: meta.title.unwrap_or(fallback_title),
            description: String::new(),
            genre: meta.genre.unwrap_or_default(),
            tags: vec![],
            rating: 0.0,
            year: meta.year.unwrap_or(0),
            duration_secs,
            director: meta.director.unwrap_or_default(),
        });
    }
    items
}

/// Walk the library to locate a video by its stable ID.
/// Returns `(absolute_path, relative_path)` when found.
async fn find_video(state: &AppState, id: &str) -> Option<(PathBuf, String)> {
    WalkDir::new(&state.library_path)
        .into_iter()
        .filter_map(|e| e.ok())
        .filter(|e| e.file_type().is_file() && is_video(e.path()))
        .find_map(|e| {
            let abs = e.path().to_path_buf();
            let rel = abs
                .strip_prefix(&state.library_path)
                .unwrap_or(&abs)
                .to_string_lossy()
                .to_string();
            if video_id(&rel) == id {
                Some((abs, rel))
            } else {
                None
            }
        })
}

// ── API handlers ─────────────────────────────────────────────────────────────

/// `GET /api/videos` — list all videos with metadata.
async fn list_videos(state: web::Data<AppState>) -> impl Responder {
    let items = scan_library(&state.library_path).await;
    HttpResponse::Ok().json(serde_json::json!({ "items": items }))
}

/// `GET /api/videos/{id}/thumbnail` — JPEG thumbnail via ffmpeg.
async fn get_thumbnail(
    id: web::Path<String>,
    state: web::Data<AppState>,
) -> impl Responder {
    let (abs_path, _) = match find_video(&state, &id).await {
        Some(v) => v,
        None => return HttpResponse::NotFound().body("video not found"),
    };

    let thumb_path = state.cache_dir.join(format!("{}.jpg", *id));
    if !thumb_path.exists() {
        let abs_str = match abs_path.to_str() {
            Some(s) => s.to_owned(),
            None => return HttpResponse::BadRequest().body("path is not valid UTF-8"),
        };
        let thumb_str = match thumb_path.to_str() {
            Some(s) => s.to_owned(),
            None => return HttpResponse::InternalServerError().body("cache path is not valid UTF-8"),
        };
        let status = Command::new("ffmpeg")
            .args([
                "-y",
                "-i",
                &abs_str,
                "-ss",
                "00:00:05",
                "-vframes",
                "1",
                "-q:v",
                "2",
                "-vf",
                "scale=640:-1",
                &thumb_str,
            ])
            .status()
            .await;
        if status.map(|s| !s.success()).unwrap_or(true) {
            return HttpResponse::ServiceUnavailable().body("ffmpeg thumbnail failed");
        }
    }

    match tokio::fs::read(&thumb_path).await {
        Ok(data) => HttpResponse::Ok()
            .content_type("image/jpeg")
            .insert_header((header::CACHE_CONTROL, "public, max-age=86400"))
            .body(data),
        Err(_) => HttpResponse::NotFound().body("thumbnail not found"),
    }
}

/// `GET /api/videos/{id}/playlist.m3u8`
///
/// Transcodes the source file to fMP4-HLS on first request (result is cached).
/// Segment URIs in the playlist are rewritten to absolute API paths so the
/// Rust/WASM frontend never has to resolve relative URLs.
async fn get_playlist(
    id: web::Path<String>,
    state: web::Data<AppState>,
) -> impl Responder {
    let (abs_path, _) = match find_video(&state, &id).await {
        Some(v) => v,
        None => return HttpResponse::NotFound().body("video not found"),
    };

    let hls_dir = state.cache_dir.join(id.as_str());
    let playlist_path = hls_dir.join("playlist.m3u8");

    if !playlist_path.exists() {
        if let Err(e) = tokio::fs::create_dir_all(&hls_dir).await {
            return HttpResponse::InternalServerError()
                .body(format!("cache dir error: {e}"));
        }

        // Transcode to fragmented-MP4 HLS (MSE-compatible in all modern browsers).
        // -profile:v baseline  → codec string "avc1.42E01E" – the most compatible H.264 variant.
        // When the playlist path is absolute, ffmpeg writes init/segment files
        // in the same directory as the playlist.
        let init_file = "init.mp4";
        let seg_pattern = "seg_%05d.m4s";

        let abs_str = match abs_path.to_str() {
            Some(s) => s.to_owned(),
            None => return HttpResponse::BadRequest().body("path is not valid UTF-8"),
        };
        let playlist_str = match playlist_path.to_str() {
            Some(s) => s.to_owned(),
            None => return HttpResponse::InternalServerError().body("cache path is not valid UTF-8"),
        };

        let output = Command::new("ffmpeg")
            .args([
                "-y",
                "-i",
                &abs_str,
                "-c:v",
                "libx264",
                "-profile:v",
                "baseline",
                "-level",
                "3.1",
                "-c:a",
                "aac",
                "-f",
                "hls",
                "-hls_segment_type",
                "fmp4",
                "-hls_time",
                "6",
                "-hls_list_size",
                "0",
                "-hls_fmp4_init_filename",
                init_file,
                "-hls_segment_filename",
                seg_pattern,
                &playlist_str,
            ])
            .output()
            .await;

        match output {
            Ok(out) if out.status.success() => {
                // Transcoding succeeded
            }
            Ok(out) => {
                let stderr = String::from_utf8_lossy(&out.stderr);
                eprintln!("ffmpeg failed: {}", stderr);
                // Return last 10 lines of stderr for better debugging
                let last_lines: Vec<&str> = stderr.lines().rev().take(10).collect();
                let error_summary = if last_lines.is_empty() {
                    "unknown error".to_string()
                } else {
                    last_lines.into_iter().rev().collect::<Vec<_>>().join("\n")
                };
                return HttpResponse::ServiceUnavailable()
                    .body(format!("transcoding failed:\n{}", error_summary));
            }
            Err(e) => {
                eprintln!("failed to execute ffmpeg: {}", e);
                return HttpResponse::ServiceUnavailable()
                    .body(format!("failed to execute ffmpeg: {}", e));
            }
        }
    }

    let raw = match tokio::fs::read_to_string(&playlist_path).await {
        Ok(c) => c,
        Err(_) => return HttpResponse::InternalServerError().body("failed to read playlist"),
    };

    // Rewrite every relative segment URI to an absolute API path.
    let rewritten: String = raw
        .lines()
        .map(|line| {
            let trimmed = line.trim();
            if let Some(inner) = trimmed
                .strip_prefix("#EXT-X-MAP:URI=\"")
                .and_then(|s| s.strip_suffix('"'))
            {
                // e.g. #EXT-X-MAP:URI="init.mp4"
                return format!(
                    "#EXT-X-MAP:URI=\"/api/videos/{}/segments/{}\"",
                    id, inner
                );
            }
            if !trimmed.starts_with('#') && !trimmed.is_empty() {
                return format!("/api/videos/{}/segments/{}", id, trimmed);
            }
            line.to_owned()
        })
        .collect::<Vec<_>>()
        .join("\n");

    HttpResponse::Ok()
        .content_type("application/vnd.apple.mpegurl")
        .insert_header((header::CACHE_CONTROL, "no-cache"))
        .body(rewritten)
}

/// `GET /api/videos/{id}/segments/{filename}` — serve an fMP4 segment or init file.
async fn get_segment(
    params: web::Path<(String, String)>,
    state: web::Data<AppState>,
) -> impl Responder {
    let (id, filename) = params.into_inner();

    // Reject path traversal and unexpected extensions.
    if filename.contains('/') || filename.contains('\\') || filename.contains("..") {
        return HttpResponse::BadRequest().body("invalid filename");
    }
    if !filename.ends_with(".m4s") && filename != "init.mp4" {
        return HttpResponse::BadRequest().body("invalid segment type");
    }

    let seg_path = state.cache_dir.join(&id).join(&filename);
    match tokio::fs::read(&seg_path).await {
        Ok(data) => HttpResponse::Ok()
            .content_type("video/mp4")
            .insert_header((
                header::CACHE_CONTROL,
                "public, max-age=31536000, immutable",
            ))
            .body(data),
        Err(_) => HttpResponse::NotFound().body("segment not found"),
    }
}

// ── Static asset serving ─────────────────────────────────────────────────────

fn content_type(path: &str) -> header::HeaderValue {
    let mime = MimeGuess::from_path(path).first_or_octet_stream();
    header::HeaderValue::from_str(mime.as_ref()).unwrap()
}

async fn frontend(req: HttpRequest) -> actix_web::Result<HttpResponse> {
    let tail = req.match_info().query("tail");
    let mut path = tail.trim_start_matches('/');
    if path.is_empty() {
        path = "index.html";
    }

    if let Some(file) = Assets::get(path) {
        let cache = if path == "index.html" {
            "no-cache"
        } else {
            "public, max-age=31536000, immutable"
        };
        return Ok(HttpResponse::Ok()
            .insert_header((header::CONTENT_TYPE, content_type(path)))
            .insert_header((header::CACHE_CONTROL, cache))
            .body(file.data.into_owned()));
    }

    if let Some(index) = Assets::get("index.html") {
        return Ok(HttpResponse::Ok()
            .insert_header((
                header::CONTENT_TYPE,
                header::HeaderValue::from_static("text/html; charset=utf-8"),
            ))
            .insert_header((header::CACHE_CONTROL, "no-cache"))
            .body(index.data.into_owned()));
    }

    Err(actix_web::error::ErrorNotFound("asset not found"))
}

// ── Entry point ──────────────────────────────────────────────────────────────

#[actix_web::main]
async fn main() -> std::io::Result<()> {
    let port = std::env::var("PORT")
        .ok()
        .and_then(|p| p.parse::<u16>().ok())
        .unwrap_or(8089);

    let library_path = PathBuf::from(
        std::env::var("VIDEO_LIBRARY_PATH").unwrap_or_else(|_| "./videos".into()),
    );
    let cache_dir = std::env::temp_dir().join("starfin_cache");

    if !library_path.exists() {
        std::fs::create_dir_all(&library_path)?;
    }
    std::fs::create_dir_all(&cache_dir)?;

    let state = web::Data::new(AppState {
        library_path: library_path.clone(),
        cache_dir: cache_dir.clone(),
    });

    println!("→ Library : {}", library_path.display());
    println!("→ Cache   : {}", cache_dir.display());
    // Bind to loopback by default; set BIND_ADDR=0.0.0.0 to expose to the network.
    let bind_addr = std::env::var("BIND_ADDR").unwrap_or_else(|_| "127.0.0.1".into());
    println!("→ Listening on http://{bind_addr}:{port}");

    HttpServer::new(move || {
        App::new()
            .app_data(state.clone())
            .wrap(Logger::default())
            .route("/api/health", web::get().to(|| async { "ok" }))
            .route("/api/videos", web::get().to(list_videos))
            .route("/api/videos/{id}/thumbnail", web::get().to(get_thumbnail))
            .route(
                "/api/videos/{id}/playlist.m3u8",
                web::get().to(get_playlist),
            )
            .route(
                "/api/videos/{id}/segments/{filename}",
                web::get().to(get_segment),
            )
            .route("/{tail:.*}", web::get().to(frontend))
    })
    .bind((bind_addr.as_str(), port))?
    .run()
    .await
}
