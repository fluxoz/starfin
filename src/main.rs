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

/// Segment duration in seconds for on-demand HLS generation.
const SEGMENT_DURATION: f64 = 6.0;

/// `GET /api/videos/{id}/playlist.m3u8`
///
/// Generates an HLS playlist dynamically based on video duration.
/// The init segment is generated quickly (just codec info, no media).
/// Media segments are transcoded on-demand when requested.
async fn get_playlist(
    id: web::Path<String>,
    state: web::Data<AppState>,
) -> impl Responder {
    let (abs_path, _) = match find_video(&state, &id).await {
        Some(v) => v,
        None => return HttpResponse::NotFound().body("video not found"),
    };

    // Get video duration via ffprobe (metadata is not needed for playlist generation)
    let (duration_secs, _metadata) = probe_video(&abs_path).await;
    if duration_secs == 0 {
        return HttpResponse::ServiceUnavailable()
            .body("Could not determine video duration. Ensure ffprobe is installed and the video file is valid.");
    }

    let hls_dir = state.cache_dir.join(id.as_str());
    if let Err(e) = tokio::fs::create_dir_all(&hls_dir).await {
        return HttpResponse::InternalServerError()
            .body(format!("cache dir error: {e}"));
    }

    // Generate init segment if it doesn't exist
    // This creates a tiny fMP4 file with just the moov atom (codec info)
    let init_path = hls_dir.join("init.mp4");
    if !init_path.exists() {
        let resolved_path = match abs_path.canonicalize() {
            Ok(p) => p,
            Err(e) => {
                return HttpResponse::InternalServerError()
                    .body(format!("failed to resolve video path: {e}"))
            }
        };
        let abs_str = match resolved_path.to_str() {
            Some(s) => s.to_owned(),
            None => return HttpResponse::BadRequest().body("path is not valid UTF-8"),
        };

        // Generate a very short fMP4 segment to extract codec info
        // Using -frames:v 1 to encode only one video frame (faster than -t 0.001)
        let output = Command::new("ffmpeg")
            .current_dir(&hls_dir)
            .stdin(std::process::Stdio::null())  // Don't read from stdin
            .args([
                "-y",
                "-nostdin",  // Disable stdin interaction
                "-i", &abs_str,
                "-frames:v", "1",  // Only encode 1 video frame
                "-c:v", "libx264",
                "-pix_fmt", "yuv420p",  // Ensure baseline-compatible pixel format
                "-profile:v", "baseline",
                "-level", "3.1",
                "-preset", "ultrafast",  // Fastest preset for init segment
                "-c:a", "aac",
                "-f", "mp4",
                "-movflags", "frag_keyframe+empty_moov+default_base_moof",
                "init.mp4",
            ])
            .output()
            .await;

        match output {
            Ok(out) if out.status.success() => {
                // Init segment generated successfully
            }
            Ok(out) => {
                let stderr = String::from_utf8_lossy(&out.stderr);
                eprintln!("ffmpeg init segment failed: {}", stderr);
                return HttpResponse::ServiceUnavailable()
                    .body("failed to generate init segment");
            }
            Err(e) => {
                return HttpResponse::ServiceUnavailable()
                    .body(format!("failed to generate init segment: {e}"));
            }
        }
    }

    // Calculate number of segments based on duration
    let duration = duration_secs as f64;
    let num_segments = (duration / SEGMENT_DURATION).ceil() as usize;

    // Build the playlist dynamically
    let mut playlist = String::new();
    playlist.push_str("#EXTM3U\n");
    playlist.push_str("#EXT-X-VERSION:7\n");
    playlist.push_str(&format!("#EXT-X-TARGETDURATION:{}\n", SEGMENT_DURATION.ceil() as u32));
    playlist.push_str("#EXT-X-MEDIA-SEQUENCE:0\n");
    playlist.push_str("#EXT-X-PLAYLIST-TYPE:VOD\n");
    playlist.push_str(&format!(
        "#EXT-X-MAP:URI=\"/api/videos/{}/segments/init.mp4\"\n",
        id
    ));

    for i in 0..num_segments {
        let seg_start = i as f64 * SEGMENT_DURATION;
        let seg_duration = if i == num_segments - 1 {
            // Last segment may be shorter
            duration - seg_start
        } else {
            SEGMENT_DURATION
        };

        playlist.push_str(&format!("#EXTINF:{:.3},\n", seg_duration));
        playlist.push_str(&format!(
            "/api/videos/{}/segments/seg_{:05}.m4s\n",
            id, i
        ));
    }

    playlist.push_str("#EXT-X-ENDLIST\n");

    HttpResponse::Ok()
        .content_type("application/vnd.apple.mpegurl")
        .insert_header((header::CACHE_CONTROL, "no-cache"))
        .body(playlist)
}

/// `GET /api/videos/{id}/segments/{filename}` — serve an fMP4 segment on-demand.
/// Segments are transcoded on-demand if they don't exist in the cache.
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

    let hls_dir = state.cache_dir.join(&id);
    let seg_path = hls_dir.join(&filename);

    // If segment exists, serve it immediately
    if let Ok(data) = tokio::fs::read(&seg_path).await {
        return HttpResponse::Ok()
            .content_type("video/mp4")
            .insert_header((
                header::CACHE_CONTROL,
                "public, max-age=31536000, immutable",
            ))
            .body(data);
    }

    // For init.mp4, it should have been created with the playlist request
    if filename == "init.mp4" {
        return HttpResponse::NotFound().body("init segment not found - request playlist first");
    }

    // Parse segment index from filename (e.g., "seg_00042.m4s" -> 42)
    let seg_index: usize = match filename
        .strip_prefix("seg_")
        .and_then(|s| s.strip_suffix(".m4s"))
        .and_then(|s| s.parse().ok())
    {
        Some(idx) => idx,
        None => return HttpResponse::BadRequest().body("invalid segment filename format"),
    };

    // Find the source video
    let (abs_path, _) = match find_video(&state, &id).await {
        Some(v) => v,
        None => return HttpResponse::NotFound().body("video not found"),
    };

    let resolved_path = match abs_path.canonicalize() {
        Ok(p) => p,
        Err(e) => {
            return HttpResponse::InternalServerError()
                .body(format!("failed to resolve video path: {e}"))
        }
    };
    let abs_str = match resolved_path.to_str() {
        Some(s) => s.to_owned(),
        None => return HttpResponse::BadRequest().body("path is not valid UTF-8"),
    };

    // Calculate segment time range
    let start_time = seg_index as f64 * SEGMENT_DURATION;

    // Create cache directory if needed
    if let Err(e) = tokio::fs::create_dir_all(&hls_dir).await {
        return HttpResponse::InternalServerError()
            .body(format!("cache dir error: {e}"));
    }

    // Transcode just this segment on-demand
    // Use -ss before -i for fast seeking, then encode just SEGMENT_DURATION seconds
    // Output as fMP4 segment (compatible with MSE)
    let output = Command::new("ffmpeg")
        .current_dir(&hls_dir)
        .stdin(std::process::Stdio::null())  // Don't read from stdin
        .args([
            "-y",
            "-nostdin",  // Disable stdin interaction
            "-ss", &format!("{:.3}", start_time),
            "-i", &abs_str,
            "-t", &format!("{:.3}", SEGMENT_DURATION),
            "-c:v", "libx264",
            "-pix_fmt", "yuv420p",  // Ensure baseline-compatible pixel format
            "-profile:v", "baseline",
            "-level", "3.1",
            "-preset", "fast",  // Faster encoding for on-demand
            "-c:a", "aac",
            "-f", "mp4",
            "-movflags", "frag_keyframe+empty_moov+default_base_moof",
            &filename,
        ])
        .output()
        .await;

    match output {
        Ok(out) if out.status.success() => {
            // Segment generated successfully, serve it
            match tokio::fs::read(&seg_path).await {
                Ok(data) => HttpResponse::Ok()
                    .content_type("video/mp4")
                    .insert_header((
                        header::CACHE_CONTROL,
                        "public, max-age=31536000, immutable",
                    ))
                    .body(data),
                Err(e) => HttpResponse::InternalServerError()
                    .body(format!("failed to read generated segment: {e}")),
            }
        }
        Ok(out) => {
            let stderr = String::from_utf8_lossy(&out.stderr);
            eprintln!("ffmpeg segment {} failed: {}", seg_index, stderr);
            HttpResponse::ServiceUnavailable()
                .body(format!("segment {} transcoding failed", seg_index))
        }
        Err(e) => {
            eprintln!("failed to execute ffmpeg for segment {}: {}", seg_index, e);
            HttpResponse::ServiceUnavailable()
                .body(format!("failed to execute ffmpeg: {e}"))
        }
    }
}

// ── Thumbnail sprite generation ──────────────────────────────────────────────

/// Thumbnail sprite configuration
const THUMBNAIL_INTERVAL: f64 = 10.0; // Generate thumbnail every 10 seconds
const THUMBNAIL_WIDTH: u32 = 160;
const THUMBNAIL_HEIGHT: u32 = 90;
const THUMBNAILS_PER_ROW: u32 = 10;

/// Response for thumbnail sprite info
#[derive(Clone, Serialize)]
struct ThumbnailInfo {
    url: String,
    sprite_width: u32,
    sprite_height: u32,
    thumb_width: u32,
    thumb_height: u32,
    columns: u32,
    rows: u32,
    interval: f64,
}

/// `GET /api/videos/{id}/thumbnails/info` — get thumbnail sprite info
async fn get_thumbnail_info(
    id: web::Path<String>,
    state: web::Data<AppState>,
) -> impl Responder {
    let (abs_path, _) = match find_video(&state, &id).await {
        Some(v) => v,
        None => return HttpResponse::NotFound().body("video not found"),
    };

    // Get video duration
    let (duration_secs, _) = probe_video(&abs_path).await;
    if duration_secs == 0 {
        return HttpResponse::ServiceUnavailable().body("Could not determine video duration");
    }

    let duration = duration_secs as f64;
    let num_thumbnails = ((duration / THUMBNAIL_INTERVAL).ceil() as u32).max(1);
    let columns = THUMBNAILS_PER_ROW.min(num_thumbnails);
    let rows = (num_thumbnails as f64 / columns as f64).ceil() as u32;

    let info = ThumbnailInfo {
        url: format!("/api/videos/{}/thumbnails/sprite.jpg", *id),
        sprite_width: columns * THUMBNAIL_WIDTH,
        sprite_height: rows * THUMBNAIL_HEIGHT,
        thumb_width: THUMBNAIL_WIDTH,
        thumb_height: THUMBNAIL_HEIGHT,
        columns,
        rows,
        interval: THUMBNAIL_INTERVAL,
    };

    HttpResponse::Ok().json(info)
}

/// `GET /api/videos/{id}/thumbnails/sprite.jpg` — get thumbnail sprite image
async fn get_thumbnail_sprite(
    id: web::Path<String>,
    state: web::Data<AppState>,
) -> impl Responder {
    let (abs_path, _) = match find_video(&state, &id).await {
        Some(v) => v,
        None => return HttpResponse::NotFound().body("video not found"),
    };

    let sprite_dir = state.cache_dir.join(format!("{}_thumbs", *id));
    let sprite_path = sprite_dir.join("sprite.jpg");

    // Check if sprite already exists
    if let Ok(data) = tokio::fs::read(&sprite_path).await {
        return HttpResponse::Ok()
            .content_type("image/jpeg")
            .insert_header((header::CACHE_CONTROL, "public, max-age=86400"))
            .body(data);
    }

    // Create sprite directory
    if let Err(e) = tokio::fs::create_dir_all(&sprite_dir).await {
        return HttpResponse::InternalServerError().body(format!("cache dir error: {e}"));
    }

    // Get video duration
    let (duration_secs, _) = probe_video(&abs_path).await;
    if duration_secs == 0 {
        return HttpResponse::ServiceUnavailable().body("Could not determine video duration");
    }

    let resolved_path = match abs_path.canonicalize() {
        Ok(p) => p,
        Err(e) => {
            return HttpResponse::InternalServerError()
                .body(format!("failed to resolve video path: {e}"))
        }
    };
    let abs_str = match resolved_path.to_str() {
        Some(s) => s.to_owned(),
        None => return HttpResponse::BadRequest().body("path is not valid UTF-8"),
    };

    let duration = duration_secs as f64;
    let num_thumbnails = ((duration / THUMBNAIL_INTERVAL).ceil() as u32).max(1);
    let columns = THUMBNAILS_PER_ROW.min(num_thumbnails);
    let rows = (num_thumbnails as f64 / columns as f64).ceil() as u32;

    // Generate thumbnail sprite using ffmpeg
    // This creates a grid of thumbnails using the tile filter
    let fps = 1.0 / THUMBNAIL_INTERVAL;
    let tile_layout = format!("{}x{}", columns, rows);
    let scale = format!("scale={}:{}", THUMBNAIL_WIDTH, THUMBNAIL_HEIGHT);

    let sprite_path_str = match sprite_path.to_str() {
        Some(s) => s.to_owned(),
        None => return HttpResponse::InternalServerError().body("sprite path is not valid UTF-8"),
    };

    let output = Command::new("ffmpeg")
        .stdin(std::process::Stdio::null())
        .args([
            "-y",
            "-nostdin",
            "-i",
            &abs_str,
            "-vf",
            &format!("fps={},{}:force_original_aspect_ratio=decrease,pad={}:{}:(ow-iw)/2:(oh-ih)/2,tile={}", 
                fps, scale, THUMBNAIL_WIDTH, THUMBNAIL_HEIGHT, tile_layout),
            "-frames:v",
            "1",
            "-q:v",
            "5",
            &sprite_path_str,
        ])
        .output()
        .await;

    match output {
        Ok(out) if out.status.success() => {
            match tokio::fs::read(&sprite_path).await {
                Ok(data) => HttpResponse::Ok()
                    .content_type("image/jpeg")
                    .insert_header((header::CACHE_CONTROL, "public, max-age=86400"))
                    .body(data),
                Err(e) => HttpResponse::InternalServerError()
                    .body(format!("failed to read sprite: {e}")),
            }
        }
        Ok(out) => {
            let stderr = String::from_utf8_lossy(&out.stderr);
            eprintln!("ffmpeg sprite generation failed: {}", stderr);
            HttpResponse::ServiceUnavailable().body("sprite generation failed")
        }
        Err(e) => {
            eprintln!("failed to execute ffmpeg for sprite: {}", e);
            HttpResponse::ServiceUnavailable().body(format!("failed to execute ffmpeg: {e}"))
        }
    }
}

// ── Subtitle extraction ──────────────────────────────────────────────────────

/// Response for subtitle tracks info
#[derive(Clone, Serialize)]
struct SubtitleTrack {
    index: u32,
    language: Option<String>,
    title: Option<String>,
    codec: String,
}

#[derive(Clone, Serialize)]
struct SubtitleTracksResponse {
    tracks: Vec<SubtitleTrack>,
}

/// `GET /api/videos/{id}/subtitles` — list available subtitle tracks
async fn list_subtitles(
    id: web::Path<String>,
    state: web::Data<AppState>,
) -> impl Responder {
    let (abs_path, _) = match find_video(&state, &id).await {
        Some(v) => v,
        None => return HttpResponse::NotFound().body("video not found"),
    };

    let resolved_path = match abs_path.canonicalize() {
        Ok(p) => p,
        Err(e) => {
            return HttpResponse::InternalServerError()
                .body(format!("failed to resolve video path: {e}"))
        }
    };
    let abs_str = match resolved_path.to_str() {
        Some(s) => s.to_owned(),
        None => return HttpResponse::BadRequest().body("path is not valid UTF-8"),
    };

    // Use ffprobe to get subtitle stream info
    let output = Command::new("ffprobe")
        .args([
            "-v", "quiet",
            "-print_format", "json",
            "-show_streams",
            "-select_streams", "s",
            &abs_str,
        ])
        .output()
        .await;

    let Ok(output) = output else {
        return HttpResponse::ServiceUnavailable().body("ffprobe not available");
    };

    let json: serde_json::Value =
        serde_json::from_slice(&output.stdout).unwrap_or(serde_json::Value::Null);

    let mut tracks = Vec::new();
    if let Some(streams) = json["streams"].as_array() {
        for (i, stream) in streams.iter().enumerate() {
            let language = stream["tags"]["language"].as_str().map(str::to_owned);
            let title = stream["tags"]["title"].as_str().map(str::to_owned);
            let codec = stream["codec_name"].as_str().unwrap_or("unknown").to_owned();
            
            tracks.push(SubtitleTrack {
                index: i as u32,
                language,
                title,
                codec,
            });
        }
    }

    HttpResponse::Ok().json(SubtitleTracksResponse { tracks })
}

/// `GET /api/videos/{id}/subtitles/{index}.vtt` — get subtitle track as WebVTT
async fn get_subtitle(
    params: web::Path<(String, u32)>,
    state: web::Data<AppState>,
) -> impl Responder {
    let (id, track_index) = params.into_inner();

    let (abs_path, _) = match find_video(&state, &id).await {
        Some(v) => v,
        None => return HttpResponse::NotFound().body("video not found"),
    };

    let sub_dir = state.cache_dir.join(format!("{}_subs", id));
    let vtt_path = sub_dir.join(format!("{}.vtt", track_index));

    // Check if subtitle already exists
    if let Ok(data) = tokio::fs::read_to_string(&vtt_path).await {
        return HttpResponse::Ok()
            .content_type("text/vtt")
            .insert_header((header::CACHE_CONTROL, "public, max-age=86400"))
            .body(data);
    }

    // Create cache directory
    if let Err(e) = tokio::fs::create_dir_all(&sub_dir).await {
        return HttpResponse::InternalServerError().body(format!("cache dir error: {e}"));
    }

    let resolved_path = match abs_path.canonicalize() {
        Ok(p) => p,
        Err(e) => {
            return HttpResponse::InternalServerError()
                .body(format!("failed to resolve video path: {e}"))
        }
    };
    let abs_str = match resolved_path.to_str() {
        Some(s) => s.to_owned(),
        None => return HttpResponse::BadRequest().body("path is not valid UTF-8"),
    };

    let vtt_path_str = match vtt_path.to_str() {
        Some(s) => s.to_owned(),
        None => return HttpResponse::InternalServerError().body("vtt path is not valid UTF-8"),
    };

    // Extract and convert subtitle to WebVTT using ffmpeg
    let output = Command::new("ffmpeg")
        .stdin(std::process::Stdio::null())
        .args([
            "-y",
            "-nostdin",
            "-i", &abs_str,
            "-map", &format!("0:s:{}", track_index),
            "-c:s", "webvtt",
            &vtt_path_str,
        ])
        .output()
        .await;

    match output {
        Ok(out) if out.status.success() => {
            match tokio::fs::read_to_string(&vtt_path).await {
                Ok(data) => HttpResponse::Ok()
                    .content_type("text/vtt")
                    .insert_header((header::CACHE_CONTROL, "public, max-age=86400"))
                    .body(data),
                Err(e) => HttpResponse::InternalServerError()
                    .body(format!("failed to read subtitle: {e}")),
            }
        }
        Ok(out) => {
            let stderr = String::from_utf8_lossy(&out.stderr);
            eprintln!("ffmpeg subtitle extraction failed: {}", stderr);
            HttpResponse::ServiceUnavailable().body("subtitle extraction failed")
        }
        Err(e) => {
            eprintln!("failed to execute ffmpeg for subtitle: {}", e);
            HttpResponse::ServiceUnavailable().body(format!("failed to execute ffmpeg: {e}"))
        }
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
        std::env::var("VIDEO_LIBRARY_PATH").unwrap_or_else(|_| "./test_videos".into()),
    );

    let cache_dir = PathBuf::new().join("starfin_cache");

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
                "/api/videos/{id}/thumbnails/info",
                web::get().to(get_thumbnail_info),
            )
            .route(
                "/api/videos/{id}/thumbnails/sprite.jpg",
                web::get().to(get_thumbnail_sprite),
            )
            .route(
                "/api/videos/{id}/subtitles",
                web::get().to(list_subtitles),
            )
            .route(
                "/api/videos/{id}/subtitles/{index}.vtt",
                web::get().to(get_subtitle),
            )
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
