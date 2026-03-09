use actix_web::{
    App, HttpRequest, HttpResponse, HttpServer, Responder,
    http::header, middleware::Logger, web,
};
use rust_embed::RustEmbed;
use mime_guess::MimeGuess;
use serde::Serialize;
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::{Arc, RwLock};
use std::sync::RwLock;
use std::time::{Duration, Instant};
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

// ── Cache eviction constants ─────────────────────────────────────────────────

/// How long a video's segments may sit in cache without a new request before
/// they are automatically removed.
const CACHE_IDLE_TIMEOUT: Duration = Duration::from_secs(10 * 60); // 10 minutes

/// How often the background sweep task wakes up to evict idle caches.
const CACHE_SWEEP_INTERVAL: Duration = Duration::from_secs(60); // 1 minute

// ── App state ────────────────────────────────────────────────────────────────

struct AppState {
    library_path: PathBuf,
    cache_dir: PathBuf,
    video_cache: Arc<RwLock<Vec<VideoItem>>>,
    /// Tracks the last time a segment was served for each video ID.
    /// Used by the background idle-eviction sweep.
    last_segment_access: RwLock<HashMap<String, Instant>>,
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

/// `GET /api/videos` — list all videos with metadata (served from cache).
async fn list_videos(state: web::Data<AppState>) -> impl Responder {
    let items = state.video_cache.read().expect("video cache lock poisoned").clone();
    HttpResponse::Ok().json(serde_json::json!({ "items": items }))
}

/// `POST /api/scan` — trigger an immediate re-scan of the media library.
async fn scan_videos(state: web::Data<AppState>) -> impl Responder {
    let items = scan_library(&state.library_path).await;
    *state.video_cache.write().expect("video cache lock poisoned") = items;
    HttpResponse::Ok().json(serde_json::json!({ "status": "ok" }))
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
/// Apple recommends 6 seconds; common range is 2–10 seconds.
/// Jellyfin/Plex default to 6 second segments.
const SEGMENT_DURATION: f64 = 6.0;

/// `GET /api/videos/{id}/playlist.m3u8`
///
/// Generates an HLS VOD playlist using MPEG-TS segments.
///
/// This follows the Jellyfin/Plex approach:
/// - MPEG-TS segment format (self-contained, no init segment required)
/// - HLS version 3 for maximum compatibility
/// - Segments are transcoded on-demand when first requested
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

    // Calculate number of segments based on duration
    let duration = duration_secs as f64;
    let num_segments = (duration / SEGMENT_DURATION).ceil() as usize;

    // Build the HLS VOD playlist with MPEG-TS segments.
    // No init segment is needed — each .ts segment is self-contained with
    // embedded codec info and PTS timestamps, unlike fMP4 which requires a
    // separate init segment with moov atom and sequential baseMediaDecodeTime.
    let mut playlist = String::new();
    playlist.push_str("#EXTM3U\n");
    playlist.push_str("#EXT-X-VERSION:3\n");
    playlist.push_str(&format!("#EXT-X-TARGETDURATION:{}\n", SEGMENT_DURATION.ceil() as u32));
    playlist.push_str("#EXT-X-MEDIA-SEQUENCE:0\n");
    playlist.push_str("#EXT-X-PLAYLIST-TYPE:VOD\n");

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
            "/api/videos/{}/segments/seg_{:05}.ts\n",
            id, i
        ));
    }

    playlist.push_str("#EXT-X-ENDLIST\n");

    HttpResponse::Ok()
        .content_type("application/vnd.apple.mpegurl")
        .insert_header((header::CACHE_CONTROL, "no-cache"))
        .body(playlist)
}

/// `GET /api/videos/{id}/segments/{filename}` — serve an MPEG-TS segment on-demand.
///
/// Segments are transcoded on-demand if they don't exist in the cache.
/// Uses MPEG-TS format (like Jellyfin) for self-contained segments with
/// embedded codec info and PTS timestamps, avoiding the fMP4
/// baseMediaDecodeTime issues that cause playback freezes.
async fn get_segment(
    params: web::Path<(String, String)>,
    state: web::Data<AppState>,
) -> impl Responder {
    let (id, filename) = params.into_inner();

    // Reject path traversal and unexpected extensions.
    if filename.contains('/') || filename.contains('\\') || filename.contains("..") {
        return HttpResponse::BadRequest().body("invalid filename");
    }
    if !filename.ends_with(".ts") {
        return HttpResponse::BadRequest().body("invalid segment type");
    }

    let hls_dir = state.cache_dir.join(&id);
    let seg_path = hls_dir.join(&filename);

    // Record that this video was actively streamed right now so the
    // idle-eviction sweep resets its 10-minute countdown.
    {
        let mut map = state
            .last_segment_access
            .write()
            .expect("last_segment_access lock poisoned");
        map.insert(id.clone(), Instant::now());
    }

    // If segment exists, serve it immediately from cache
    if let Ok(data) = tokio::fs::read(&seg_path).await {
        return HttpResponse::Ok()
            .content_type("video/mp2t")
            .insert_header((
                header::CACHE_CONTROL,
                "public, max-age=31536000, immutable",
            ))
            .body(data);
    }

    // Parse segment index from filename (e.g., "seg_00042.ts" -> 42)
    let seg_index: usize = match filename
        .strip_prefix("seg_")
        .and_then(|s| s.strip_suffix(".ts"))
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
    // start_time is always non-negative (seg_index * SEGMENT_DURATION, both >= 0)
    let start_time = seg_index as f64 * SEGMENT_DURATION;
    debug_assert!(start_time >= 0.0 && start_time.is_finite());

    // Create cache directory if needed
    if let Err(e) = tokio::fs::create_dir_all(&hls_dir).await {
        return HttpResponse::InternalServerError()
            .body(format!("cache dir error: {e}"));
    }

    // Transcode just this segment on-demand using MPEG-TS output format.
    //
    // This follows the Jellyfin approach (using ffmpeg's HLS/MPEG-TS muxer):
    // - `-ss` before `-i` for fast input seeking to the segment start
    // - `-t` to limit output to one segment duration
    // - `-output_ts_offset` to set correct absolute PTS timestamps
    //   (without this, each segment's PTS would start from 0 instead of
    //    the correct position in the stream timeline)
    // - `-f mpegts` for self-contained MPEG Transport Stream output
    // - `-force_key_frames expr:gte(t,0)` to ensure segment starts with a keyframe
    let ts_offset = format!("{:.3}", start_time);
    let output = Command::new("ffmpeg")
        .current_dir(&hls_dir)
        .stdin(std::process::Stdio::null())
        .args([
            "-y",
            "-nostdin",
            "-ss", &format!("{:.3}", start_time),
            "-i", &abs_str,
            "-t", &format!("{:.3}", SEGMENT_DURATION),
            "-c:v", "libx264",
            "-pix_fmt", "yuv420p",
            "-profile:v", "baseline",
            "-level", "3.1",
            "-preset", "veryfast",
            "-force_key_frames", "expr:gte(t,0)",
            "-c:a", "aac",
            "-b:a", "128k",
            "-output_ts_offset", &ts_offset,
            "-f", "mpegts",
            &filename,
        ])
        .output()
        .await;

    match output {
        Ok(out) if out.status.success() => {
            // Segment generated successfully, serve it
            match tokio::fs::read(&seg_path).await {
                Ok(data) => HttpResponse::Ok()
                    .content_type("video/mp2t")
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

// ── Cache management ─────────────────────────────────────────────────────────

/// `DELETE /api/videos/{id}/cache` — clear cached segments for a video.
///
/// Removes the directory `cache_dir/{id}/` which holds transcoded MPEG-TS
/// segments.  Called by the frontend when the user navigates away from the
/// player so that disk space is reclaimed immediately.
async fn clear_cache(
    id: web::Path<String>,
    state: web::Data<AppState>,
) -> impl Responder {
    let id = id.into_inner();

    // Validate that the ID is a well-formed UUID to prevent path-traversal.
    if Uuid::parse_str(&id).is_err() {
        return HttpResponse::BadRequest().body("invalid video id");
    }

    let cache_subdir = state.cache_dir.join(&id);

    match tokio::fs::remove_dir_all(&cache_subdir).await {
        Ok(_) => {
            // Also cancel idle-eviction tracking so a stale entry doesn't
            // trigger a redundant removal on the next sweep.
            state
                .last_segment_access
                .write()
                .expect("last_segment_access lock poisoned")
                .remove(&id);
            HttpResponse::NoContent().finish()
        }
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
            // Nothing cached – that's fine, treat as success.
            state
                .last_segment_access
                .write()
                .expect("last_segment_access lock poisoned")
                .remove(&id);
            HttpResponse::NoContent().finish()
        }
        Err(e) => HttpResponse::InternalServerError()
            .body(format!("failed to clear cache: {e}")),
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

    // Initial library scan at startup.
    println!("→ Scanning library…");
    let initial_items = scan_library(&library_path).await;
    println!("→ Found {} video(s)", initial_items.len());
    let video_cache: Arc<RwLock<Vec<VideoItem>>> = Arc::new(RwLock::new(initial_items));

    let state = web::Data::new(AppState {
        library_path: library_path.clone(),
        cache_dir: cache_dir.clone(),
        video_cache: Arc::clone(&video_cache),
    });

    // Background task: re-scan the library every 60 seconds.
    let bg_library_path = library_path.clone();
    let bg_cache = Arc::clone(&video_cache);
    tokio::spawn(async move {
        let mut interval = tokio::time::interval(std::time::Duration::from_secs(60));
        interval.tick().await; // skip the immediate tick
        loop {
            interval.tick().await;
            let items = scan_library(&bg_library_path).await;
            *bg_cache.write().expect("video cache lock poisoned") = items;
        }
        last_segment_access: RwLock::new(HashMap::new()),
    });

    // ── Idle-eviction background task ────────────────────────────────────────
    // Every CACHE_SWEEP_INTERVAL, remove the cached segments of any video that
    // has not had a segment request for at least CACHE_IDLE_TIMEOUT.
    {
        let sweep_state = state.clone();
        tokio::spawn(async move {
            let mut interval = tokio::time::interval(CACHE_SWEEP_INTERVAL);
            loop {
                interval.tick().await;

                // Collect IDs whose caches have gone idle.
                // The read lock is held only for the in-memory scan; it is
                // released (by dropping `map`) before any filesystem work.
                let idle_ids: Vec<String> = {
                    let map = sweep_state
                        .last_segment_access
                        .read()
                        .expect("last_segment_access lock poisoned");
                    map.iter()
                        .filter(|(_, t)| t.elapsed() >= CACHE_IDLE_TIMEOUT)
                        .map(|(id, _)| id.clone())
                        .collect()
                };

                for id in idle_ids {
                    let cache_subdir = sweep_state.cache_dir.join(&id);
                    match tokio::fs::remove_dir_all(&cache_subdir).await {
                        Ok(_) => {
                            println!("→ Cache evicted (idle): {id}");
                        }
                        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
                        Err(e) => {
                            eprintln!("cache eviction error for {id}: {e}");
                        }
                    }
                    sweep_state
                        .last_segment_access
                        .write()
                        .expect("last_segment_access lock poisoned")
                        .remove(&id);
                }
            }
        });
    }
    // ─────────────────────────────────────────────────────────────────────────

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
            .route("/api/scan", web::post().to(scan_videos))
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
            .route(
                "/api/videos/{id}/cache",
                web::delete().to(clear_cache),
            )
            .route("/{tail:.*}", web::get().to(frontend))
    })
    .bind((bind_addr.as_str(), port))?
    .run()
    .await
}
