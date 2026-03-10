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
use std::time::{Duration, Instant, UNIX_EPOCH};
use tokio::process::Command;
use uuid::Uuid;
use walkdir::WalkDir;

// ── Hardware acceleration ─────────────────────────────────────────────────────

/// The hardware acceleration backend detected at startup.
#[derive(Debug, Clone, PartialEq, Eq)]
enum HwAccel {
    /// NVIDIA GPU via NVENC/CUDA
    Nvidia,
    /// AMD or Intel GPU on Linux via VAAPI
    Vaapi,
    /// Intel GPU via Quick Sync Video
    Qsv,
    /// Apple GPU via VideoToolbox (macOS)
    VideoToolbox,
    /// AMD GPU on Windows via AMF
    Amf,
    /// Pure software fallback (libx264)
    Software,
}

impl HwAccel {
    /// Human-readable label shown in the dashboard.
    fn label(&self) -> &'static str {
        match self {
            HwAccel::Nvidia       => "NVIDIA (NVENC)",
            HwAccel::Vaapi        => "AMD/Intel (VAAPI)",
            HwAccel::Qsv          => "Intel (QSV)",
            HwAccel::VideoToolbox => "Apple (VideoToolbox)",
            HwAccel::Amf          => "AMD (AMF)",
            HwAccel::Software     => "CPU (software)",
        }
    }

    /// The `-c:v` encoder name to pass to ffmpeg for transcoding.
    fn encoder(&self) -> &'static str {
        match self {
            HwAccel::Nvidia       => "h264_nvenc",
            HwAccel::Vaapi        => "h264_vaapi",
            HwAccel::Qsv          => "h264_qsv",
            HwAccel::VideoToolbox => "h264_videotoolbox",
            HwAccel::Amf          => "h264_amf",
            HwAccel::Software     => "libx264",
        }
    }

    /// Extra args to insert BEFORE `-i` for hardware-accelerated decoding.
    fn hwaccel_decode_args(&self) -> &'static [&'static str] {
        match self {
            HwAccel::Nvidia => &["-hwaccel", "cuda", "-hwaccel_output_format", "cuda"],
            HwAccel::Vaapi  => &["-hwaccel", "vaapi", "-hwaccel_output_format", "vaapi",
                                  "-hwaccel_device", "/dev/dri/renderD128"],
            HwAccel::Qsv          => &["-hwaccel", "qsv"],
            HwAccel::VideoToolbox => &["-hwaccel", "videotoolbox"],
            HwAccel::Amf          => &["-hwaccel", "d3d11va"],
            HwAccel::Software     => &[],
        }
    }

    /// Extra quality/preset args appended after `-c:v <encoder>`.
    /// For the software encoder this also includes the compatibility args
    /// needed for broad HLS player support.
    fn encoder_quality_args(&self) -> &'static [&'static str] {
        match self {
            HwAccel::Nvidia        => &["-preset", "p7", "-tune", "hq", "-temporal-aq", "1", "-spatial-aq", "1", "-rc", "constqp", "-qp", "18"],
            HwAccel::Vaapi         => &["-profile:v", "high", "-qp", "18"],
            HwAccel::Qsv           => &["-preset", "veryslow", "-global_quality", "18"],
            HwAccel::VideoToolbox  => &["-qp", "18", "-profile:v", "high"],
            HwAccel::Amf           => &["-quality", "quality", "-rc", "cqp", "-qp", "18"],
            HwAccel::Software      => &["-preset", "veryslow",
                                        "-crf", "18",
                                        "-pix_fmt", "yuv420p",
                                        "-profile:v", "high",
                                        "-level", "4.2"],
        }
    }
}

async fn test_hwaccel(hw_name: &str) -> bool {
    match hw_name {
        "cuda" | "nvdec" => {
            let args = vec![
                "-v", "quiet", "-hide_banner",
                "-init_hw_device", "cuda=test",
                "-f", "lavfi", "-i", "nullsrc",
                "-frames:v", "1",
                "-f", "null", "-",
            ];
            run_ffmpeg_test(&args).await
        }
        "vaapi" => {
            // Hades Canyon priority: AMD Vega M first (stable PCI path), then Intel, then legacy nodes
            let candidates = [
                "/dev/dri/by-path/pci-0000:01:00.0-render", // ← AMD Radeon RX Vega M GH (your working one)
                "/dev/dri/by-path/pci-0000:00:02.0-render", // Intel UHD 630
                "/dev/dri/renderD129",
                "/dev/dri/renderD128",
                "/dev/dri/renderD130",
                "/dev/dri/renderD131",
            ];

            for dev in candidates {
                let device_string = format!("vaapi=va:{}", dev);
                let args = vec![
                    "-v", "quiet", "-hide_banner",
                    "-init_hw_device", device_string.as_str(),
                    "-f", "lavfi", "-i", "nullsrc",
                    "-frames:v", "1",
                    "-f", "null", "-",
                ];
                if run_ffmpeg_test(&args).await {
                    println!("→ VAAPI using {}", dev);
                    return true;
                }
            }
            false
        }
        "qsv" => {
            let args = vec![
                "-v", "quiet", "-hide_banner",
                "-init_hw_device", "qsv=test",
                "-f", "lavfi", "-i", "nullsrc",
                "-frames:v", "1",
                "-f", "null", "-",
            ];
            run_ffmpeg_test(&args).await
        }
        _ => false,
    }
}

/// Tiny helper so we don’t duplicate the spawn code
async fn run_ffmpeg_test(args: &[&str]) -> bool {
    tokio::process::Command::new("ffmpeg")
        .args(args)
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .output()
        .await
        .map(|o| o.status.success())
        .unwrap_or(false)
}

/// Probe which GPU encoder is available by attempting a tiny null-transcode
/// with each backend in priority order. Called once at startup.
async fn detect_hwaccel() -> HwAccel {
    // Quick filter: only test things that are actually compiled into this Nix build
    let output = match tokio::process::Command::new("ffmpeg")
        .args(["-hwaccels"])
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::null())
        .output()
        .await
    {
        Ok(output) if output.status.success() => output.stdout,
        _ => {
            println!("→ GPU acceleration: CPU (software fallback)");
            return HwAccel::Software;
        }
    };

    let output_str = String::from_utf8_lossy(&output);
    let mut available: std::collections::HashSet<String> = std::collections::HashSet::new();
    let mut in_list = false;

    for line in output_str.lines() {
        let trimmed = line.trim();
        if trimmed == "Hardware acceleration methods:" {
            in_list = true;
            continue;
        }
        if in_list && !trimmed.is_empty() && trimmed != "none" {
            available.insert(trimmed.to_lowercase());
        }
    }

    println!("available (compiled-in): {:?}", available);

    // Priority order + real runtime test (succeeds ONLY if hardware + driver + libs are usable)
    let candidates: &[(&str, HwAccel)] = &[
        ("cuda", HwAccel::Nvidia),
        ("nvdec", HwAccel::Nvidia),
        ("vaapi", HwAccel::Vaapi),
        ("qsv", HwAccel::Qsv),
    ];

    for (hw_name, variant) in candidates {
        if available.contains(*hw_name) && test_hwaccel(hw_name).await {
            println!("→ GPU acceleration: {} ({})", variant.label(), hw_name);
            return variant.clone();
        }
    }

    println!("→ GPU acceleration: CPU (software fallback)");
    HwAccel::Software
}

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
    /// Unix timestamp (seconds) of the file's last modification time.
    date_added: u64,
}

// ── Cache eviction constants ─────────────────────────────────────────────────

/// How long a video's segments may sit in cache without a new request before
/// they are automatically removed.
const CACHE_IDLE_TIMEOUT: Duration = Duration::from_secs(10 * 60); // 10 minutes

/// How often the background sweep task wakes up to evict idle caches.
const CACHE_SWEEP_INTERVAL: Duration = Duration::from_secs(60); // 1 minute

// ── App state ────────────────────────────────────────────────────────────────

/// Tracks the progress of the thumbnail generation background job.
struct ThumbProgress {
    current: u32,
    total: u32,
    active: bool,
    /// Which generation phase is running: `"quick"` or `"deep"`.
    phase: &'static str,
    /// The video ID currently being processed, or `None` when idle.
    current_id: Option<String>,
}

/// Tracks the progress of the sprite generation background job.
struct SpriteProgress {
    current: u32,
    total: u32,
    active: bool,
    /// The video ID currently being processed, or `None` when idle.
    current_id: Option<String>,
}

struct AppState {
    library_path: PathBuf,
    cache_dir: PathBuf,
    video_cache: Arc<RwLock<Vec<VideoItem>>>,
    /// Tracks the last time a segment was served for each video ID.
    /// Used by the background idle-eviction sweep.
    last_segment_access: RwLock<HashMap<String, Instant>>,
    /// Progress counters for the background deep-thumbnail generation worker.
    thumb_progress: Arc<RwLock<ThumbProgress>>,
    /// Notified to (re-)start the deep thumbnail generation batch.
    thumb_trigger: Arc<tokio::sync::Notify>,
    /// Progress counters for the background sprite generation worker.
    sprite_progress: Arc<RwLock<SpriteProgress>>,
    /// Notified to (re-)start the sprite generation batch.
    sprite_trigger: Arc<tokio::sync::Notify>,
    /// Detected hardware acceleration backend (detected once at startup).
    hwaccel: HwAccel,
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

/// Returns the file's modification time as a Unix timestamp (seconds).
/// Falls back to `0` if metadata is unavailable.
fn file_date_added(path: &Path) -> u64 {
    std::fs::metadata(path)
        .ok()
        .and_then(|m| m.modified().ok())
        .and_then(|t| t.duration_since(UNIX_EPOCH).ok())
        .map(|d| d.as_secs())
        .unwrap_or(0)
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
            date_added: file_date_added(&abs),
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

/// `GET /api/scan/ws` — WebSocket endpoint that starts an immediate library scan and
/// streams live progress as JSON text frames: `{"current":N,"total":M}`.
/// The connection closes once the scan completes and the cache has been updated.
async fn scan_ws(
    req: HttpRequest,
    body: web::Payload,
    state: web::Data<AppState>,
) -> Result<HttpResponse, actix_web::Error> {
    let (response, mut session, _msg_stream) = actix_ws::handle(&req, body)?;

    let library_path = state.library_path.clone();
    let video_cache = Arc::clone(&state.video_cache);
    let thumb_trigger = Arc::clone(&state.thumb_trigger);
    let sprite_trigger = Arc::clone(&state.sprite_trigger);

    actix_web::rt::spawn(async move {
        // Enumerate all video files up-front so we can report a total.
        let entries: Vec<_> = WalkDir::new(&library_path)
            .into_iter()
            .filter_map(|e| e.ok())
            .filter(|e| e.file_type().is_file() && is_video(e.path()))
            .collect();

        let total = entries.len() as u32;

        // Send the initial frame so the client knows the total immediately.
        let init_msg = serde_json::json!({"current": 0u32, "total": total}).to_string();
        if session.text(init_msg).await.is_err() {
            return; // Client already disconnected.
        }

        let mut items = Vec::new();
        for (idx, entry) in entries.into_iter().enumerate() {
            let abs = entry.path().to_path_buf();
            let rel = abs
                .strip_prefix(&library_path)
                .unwrap_or(&abs)
                .to_string_lossy()
                .to_string();

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
                date_added: file_date_added(&abs),
            });

            let current = (idx + 1) as u32;
            let msg = serde_json::json!({"current": current, "total": total}).to_string();
            if session.text(msg).await.is_err() {
                return; // Client disconnected mid-scan.
            }
        }

        // Commit the updated library to the shared cache.
        *video_cache.write().expect("video cache lock poisoned") = items;

        // Re-trigger deep thumbnail generation for any newly discovered videos.
        thumb_trigger.notify_one();

        // Re-trigger sprite generation for any newly discovered videos.
        sprite_trigger.notify_one();

        // Close the WebSocket — the client uses this signal to know the scan is done.
        let _ = session.close(None).await;
    });

    Ok(response)
}

/// `GET /api/videos/{id}/thumbnail` — serve the cached JPEG thumbnail.
///
/// Thumbnails are generated entirely in the background by `run_thumb_worker`
/// (quick random-frame grab first, then upgraded to a signalstats-selected
/// frame).  If the thumbnail has not yet been generated this returns 404 so
/// callers can handle the not-ready state gracefully.
async fn get_thumbnail(
    id: web::Path<String>,
    state: web::Data<AppState>,
) -> impl Responder {
    let thumb_path = state.cache_dir.join(format!("{}.jpg", *id));
    match tokio::fs::read(&thumb_path).await {
        Ok(data) => HttpResponse::Ok()
            .content_type("image/jpeg")
            .insert_header((header::CACHE_CONTROL, "public, max-age=86400"))
            .body(data),
        Err(_) => HttpResponse::NotFound().body("thumbnail not ready"),
    }
}

// ── Thumbnail background job ──────────────────────────────────────────────────

/// Quick one-shot thumbnail: seeks to a **fresh random** position within
/// 20–80% of the video runtime and grabs a single frame.  The position is
/// different every time this function is called so repeated runs will pick
/// different frames.
///
/// ffmpeg stdout **and** stderr are suppressed so no ffmpeg output appears in
/// the main process.
async fn generate_quick_thumbnail(id: &str, video_path: &Path, cache_dir: &Path) -> bool {
    let thumb_path = cache_dir.join(format!("{}.jpg", id));
    if thumb_path.exists() {
        return true;
    }

    let (duration_secs, _) = probe_video(video_path).await;
    if duration_secs == 0 {
        return false;
    }
    let duration = duration_secs as f64;

    // Pick a fresh random position in [20 %, 80 %) of the runtime.
    // Uuid::new_v4() uses a CSPRNG, giving a different value every call.
    let random_byte = Uuid::new_v4().as_bytes()[0];
    let fraction = random_byte as f64 / 255.0; // maps to [0.0, 1.0]
    let seek_secs = format!("{:.3}", (duration * (0.20 + fraction * 0.60)).max(1.0));

    let video_str = match video_path.to_str() {
        Some(s) => s.to_owned(),
        None => return false,
    };
    let thumb_str = match thumb_path.to_str() {
        Some(s) => s.to_owned(),
        None => return false,
    };

    let status = Command::new("ffmpeg")
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .args([
            "-hwaccel", "auto",
            "-y", "-ss", &seek_secs, "-i", &video_str,
            "-frames:v", "1", "-q:v", "2",
            &thumb_str,
        ])
        .status()
        .await;

    status.map(|s| s.success()).unwrap_or(false)
}

/// Two-pass ffmpeg thumbnail that uses `signalstats` to pick the most visually
/// appealing frame from the 20–80% window of the video:
///
/// Pass 1 — sample frames at 1 fps/5 s and capture signal statistics.
/// Parse SATAVG (colour saturation) and BRNG (out-of-range pixel ratio) for
/// each frame.  Prefer frames with high saturation and low BRNG (i.e. neither
/// overexposed nor underexposed).
///
/// Pass 2 — seek directly to the chosen timestamp and write a single JPEG.
/// ffmpeg stdout **and** stderr are suppressed in pass 2.
///
/// A side-car marker file `{id}.deep` is created on success so the job is
/// skipped on subsequent runs.
async fn generate_deep_thumbnail(id: &str, video_path: &Path, cache_dir: &Path) -> bool {
    let deep_marker = cache_dir.join(format!("{}.deep", id));
    if deep_marker.exists() {
        return true;
    }

    let (duration_secs, _) = probe_video(video_path).await;
    if duration_secs == 0 {
        return false;
    }

    let duration = duration_secs as f64;
    let start = duration * 0.20;
    let length = duration * 0.60; // analyze 20 % – 80 %

    let video_str = match video_path.to_str() {
        Some(s) => s.to_owned(),
        None => return false,
    };

    // Pass 1: run signalstats on one frame every 5 seconds within the window.
    let analysis = Command::new("ffmpeg")
        .stdin(std::process::Stdio::null())
        .args([
            "-hwaccel",
            "auto",
            "-ss",
            &format!("{:.3}", start),
            "-t",
            &format!("{:.3}", length),
            "-i",
            &video_str,
            "-vf",
            "fps=1/5,signalstats",
            "-f",
            "null",
            "-",
        ])
        .output()
        .await;

    let default_time = start + length * 0.5;
    let best_time = match analysis {
        Ok(out) => {
            let stderr = String::from_utf8_lossy(&out.stderr);
            find_best_frame_time(&stderr, default_time)
        }
        Err(_) => default_time,
    };

    let thumb_path = cache_dir.join(format!("{}.jpg", id));
    let thumb_str = match thumb_path.to_str() {
        Some(s) => s.to_owned(),
        None => return false,
    };

    // Pass 2: extract the chosen frame.  Suppress stdout/stderr so no
    // ffmpeg output appears in the main process.
    let status = Command::new("ffmpeg")
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .args([
            "-hwaccel",
            "auto",
            "-y",
            "-ss",
            &format!("{:.3}", best_time),
            "-i",
            &video_str,
            "-frames:v",
            "1",
            "-q:v",
            "2",
            &thumb_str,
        ])
        .status()
        .await;

    if status.map(|s| s.success()).unwrap_or(false) {
        let _ = tokio::fs::write(&deep_marker, b"").await;
        true
    } else {
        false
    }
}

/// Maximum fraction of out-of-range pixels (BRNG) a frame may have to be
/// considered well-exposed.  Frames above this threshold are skipped.
const MAX_BRNG: f64 = 5.0;

/// Parse the signalstats stderr output and return the `pts_time` of the frame
/// with the highest `SATAVG` whose `BRNG` (out-of-range pixel fraction) is
/// below `MAX_BRNG`.  Falls back to `default_time` when no qualifying frame
/// is found.
fn find_best_frame_time(stderr: &str, default_time: f64) -> f64 {
    let mut best_time: Option<f64> = None;
    let mut best_satavg = -1.0_f64;

    for line in stderr.lines() {
        if !line.contains("signalstats") {
            continue;
        }
        let Some(pts_time) = parse_float_field(line, "pts_time:") else {
            continue;
        };
        let Some(satavg) = parse_float_field(line, "SATAVG:") else {
            continue;
        };
        // When BRNG is absent treat the frame as over/under-exposed.
        let brng = parse_float_field(line, "BRNG:").unwrap_or(f64::MAX);

        // Skip overexposed / underexposed frames.
        if brng > MAX_BRNG {
            continue;
        }
        if satavg > best_satavg {
            best_satavg = satavg;
            best_time = Some(pts_time);
        }
    }

    best_time.unwrap_or(default_time)
}

/// Extract a `f64` value from a signalstats output line immediately after the
/// given `field` label (e.g. `"pts_time:"`, `"SATAVG:"`).
fn parse_float_field(line: &str, field: &str) -> Option<f64> {
    let idx = line.find(field)?;
    let after = &line[idx + field.len()..];
    let end = after
        .find(|c: char| !c.is_ascii_digit() && c != '.' && c != '-')
        .unwrap_or(after.len());
    after[..end].parse().ok()
}

/// Background worker that processes videos one at a time in two sequential
/// phases.
///
/// **Phase 1 — quick thumbnails**: for every video whose `.jpg` is absent,
/// grab a single deterministic random frame within 20–80% of the runtime.
/// This is fast (one short ffmpeg invocation per file) and gives the UI
/// something to show immediately.
///
/// **Phase 2 — deep thumbnails**: for every video whose `.deep` marker is
/// absent, run the two-pass signalstats analysis to select and extract the
/// most visually representative frame, then replace the quick thumbnail with
/// the better one.
///
/// Both phases are triggered by a notification on `trigger` (sent at startup
/// and after every library re-scan).  Progress counters are written to
/// `progress` so `GET /api/thumbnails/progress` can drive the frontend bar.
///
/// All ffmpeg invocations in this worker suppress their stdout **and** stderr
/// so no ffmpeg output appears in the main process.
async fn run_thumb_worker(
    library_path: PathBuf,
    cache_dir: PathBuf,
    progress: Arc<RwLock<ThumbProgress>>,
    trigger: Arc<tokio::sync::Notify>,
) {
    loop {
        trigger.notified().await;

        // ── Phase 1: quick thumbnails ─────────────────────────────────────

        let (quick_done, quick_entries): (Vec<_>, Vec<_>) = WalkDir::new(&library_path)
            .into_iter()
            .filter_map(|e| e.ok())
            .filter(|e| e.file_type().is_file() && is_video(e.path()))
            .partition(|e| {
                let abs = e.path();
                let rel = abs
                    .strip_prefix(&library_path)
                    .unwrap_or(abs)
                    .to_string_lossy();
                let id = video_id(&rel);
                cache_dir.join(format!("{}.jpg", id)).exists()
            });

        {
            let mut p = progress.write().expect("thumb_progress lock poisoned");
            p.current = quick_done.len() as u32;
            p.total = (quick_done.len() + quick_entries.len()) as u32;
            p.active = !quick_entries.is_empty();
            p.phase = "quick";
        }

        for entry in quick_entries {
            let abs = entry.path().to_path_buf();
            let rel = abs
                .strip_prefix(&library_path)
                .unwrap_or(&abs)
                .to_string_lossy()
                .to_string();
            let id = video_id(&rel);

            {
                let mut p = progress.write().expect("thumb_progress lock poisoned");
                p.current_id = Some(id.clone());
            }
            generate_quick_thumbnail(&id, &abs, &cache_dir).await;

            let mut p = progress.write().expect("thumb_progress lock poisoned");
            p.current_id = None;
            p.current += 1;
            if p.current >= p.total {
                p.active = false;
            }
        }

        // ── Phase 2: deep thumbnails ──────────────────────────────────────

        let (deep_done, deep_entries): (Vec<_>, Vec<_>) = WalkDir::new(&library_path)
            .into_iter()
            .filter_map(|e| e.ok())
            .filter(|e| e.file_type().is_file() && is_video(e.path()))
            .partition(|e| {
                let abs = e.path();
                let rel = abs
                    .strip_prefix(&library_path)
                    .unwrap_or(abs)
                    .to_string_lossy();
                let id = video_id(&rel);
                cache_dir.join(format!("{}.deep", id)).exists()
            });

        {
            let mut p = progress.write().expect("thumb_progress lock poisoned");
            p.current = deep_done.len() as u32;
            p.total = (deep_done.len() + deep_entries.len()) as u32;
            p.active = !deep_entries.is_empty();
            p.phase = "deep";
        }

        for entry in deep_entries {
            let abs = entry.path().to_path_buf();
            let rel = abs
                .strip_prefix(&library_path)
                .unwrap_or(&abs)
                .to_string_lossy()
                .to_string();
            let id = video_id(&rel);

            {
                let mut p = progress.write().expect("thumb_progress lock poisoned");
                p.current_id = Some(id.clone());
            }
            generate_deep_thumbnail(&id, &abs, &cache_dir).await;

            let mut p = progress.write().expect("thumb_progress lock poisoned");
            p.current_id = None;
            p.current += 1;
            if p.current >= p.total {
                p.active = false;
            }
        }
    }
}

// ─────────────────────────────────────────────────────────────────────────────

/// `GET /api/thumbnails/progress` — current thumbnail generation progress.
///
/// Returns `{"current":N,"total":M,"active":bool,"phase":"quick"|"deep"}`.
/// The frontend polls this every few seconds to drive the progress bar on the
/// homepage.
#[derive(Clone, Serialize)]
struct ThumbProgressResponse {
    current: u32,
    total: u32,
    active: bool,
    phase: String,
}

async fn get_thumb_progress(state: web::Data<AppState>) -> impl Responder {
    let p = state.thumb_progress.read().expect("thumb_progress lock poisoned");
    HttpResponse::Ok().json(ThumbProgressResponse {
        current: p.current,
        total: p.total,
        active: p.active,
        phase: p.phase.to_owned(),
    })
}

/// `GET /api/progress/ws` — persistent WebSocket that streams live progress
/// updates from the thumbnail and sprite background workers at 500 ms intervals.
///
/// Each frame is a JSON text message:
/// ```json
/// {
///   "thumb":  { "current": N, "total": M, "active": bool, "phase": "quick" },
///   "sprite": { "current": N, "total": M, "active": bool }
/// }
/// ```
async fn progress_ws(
    req: HttpRequest,
    body: web::Payload,
    state: web::Data<AppState>,
) -> Result<HttpResponse, actix_web::Error> {
    let (response, mut session, _msg_stream) = actix_ws::handle(&req, body)?;

    let thumb_progress = Arc::clone(&state.thumb_progress);
    let sprite_progress = Arc::clone(&state.sprite_progress);

    actix_web::rt::spawn(async move {
        let mut ticker = tokio::time::interval(tokio::time::Duration::from_millis(500));
        loop {
            ticker.tick().await;

            let (tc, tt, ta, tph) = {
                let p = thumb_progress.read().expect("thumb_progress lock poisoned");
                (p.current, p.total, p.active, p.phase)
            };
            let (sc, st, sa) = {
                let p = sprite_progress.read().expect("sprite_progress lock poisoned");
                (p.current, p.total, p.active)
            };

            let msg = serde_json::json!({
                "thumb":  { "current": tc, "total": tt, "active": ta, "phase": tph },
                "sprite": { "current": sc, "total": st, "active": sa }
            })
            .to_string();

            if session.text(msg).await.is_err() {
                break; // Client disconnected.
            }
        }
    });

    Ok(response)
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
    // - `-force_key_frames 0` to ensure the very first frame is a keyframe
    // - `-bf 0` to disable B-frames (see below)
    let ts_offset = format!("{:.3}", start_time);
    let mut cmd = Command::new("ffmpeg");
    cmd.current_dir(&hls_dir)
       .stdin(std::process::Stdio::null());

    // Prepend GPU decode args before the input
    for arg in state.hwaccel.hwaccel_decode_args() {
        cmd.arg(arg);
    }

    cmd.args([
        "-y", "-nostdin",
        "-ss", &format!("{:.3}", start_time),
        "-i", &abs_str,
        "-t", &format!("{:.3}", SEGMENT_DURATION),
    ]);

    // GPU encoder
    cmd.args(["-c:v", state.hwaccel.encoder()]);
    cmd.args(state.hwaccel.encoder_quality_args());

    // Disable B-frames.  B-frames require DTS/PTS reordering which makes the
    // first decodable frame in an MPEG-TS segment *not* the first stored
    // packet.  When HLS.js seeks and the browser appends a segment to a
    // SourceBuffer for independent decoding, this mismatch causes
    // "avcodec_send_packet error: End of file" decode failures.  Disabling
    // B-frames ensures DTS == PTS order and each segment is independently
    // decodable from its very first packet.
    cmd.args(["-bf", "0"]);

    // For NVENC, promote forced keyframes to true IDR frames.  Without this
    // flag NVENC may emit a closed-GOP I-frame instead of an IDR, which
    // does not flush the decoder's reference picture buffer and can leave
    // the browser unable to decode the segment in isolation.
    if state.hwaccel == HwAccel::Nvidia {
        cmd.args(["-forced-idr", "1"]);
    }

    cmd.args([
        // Force a keyframe at encoding timestamp 0 (the first frame of this
        // segment).  Using "0" rather than "expr:gte(t,0)" is intentional:
        // the expression gte(t,0) is always true and would make *every* frame
        // a keyframe, which is extremely inefficient with hardware encoders.
        "-force_key_frames", "0",
        "-c:a", "aac",
        "-b:a", "128k",
        "-output_ts_offset", &ts_offset,
        "-f", "mpegts",
        &filename,
    ]);

    let output = cmd.output().await;

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

/// `GET /api/hwaccel` — returns the detected hardware acceleration backend.
async fn get_hwaccel(state: web::Data<AppState>) -> impl Responder {
    HttpResponse::Ok().json(serde_json::json!({
        "label":   state.hwaccel.label(),
        "encoder": state.hwaccel.encoder(),
    }))
}

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
const THUMBNAIL_WIDTH: u32 = 640;
const THUMBNAIL_HEIGHT: u32 = 360;
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

/// `GET /api/videos/{id}/thumbnails/sprite-status` — check if sprite is cached
///
/// Returns `{"ready": true}` when the sprite sheet has already been generated
/// and is available in the cache.  Returns `{"ready": false}` otherwise.
/// This endpoint never triggers ffmpeg — it is a cheap filesystem check so
/// the frontend can decide whether to show a hover preview.
async fn get_sprite_status(
    id: web::Path<String>,
    state: web::Data<AppState>,
) -> impl Responder {
    // Validate that the ID is a well-formed UUID to prevent path-traversal.
    if Uuid::parse_str(&id).is_err() {
        return HttpResponse::BadRequest().body("invalid video id");
    }

    let sprite_path = state
        .cache_dir
        .join(format!("{}_thumbs", *id))
        .join("sprite.jpg");

    let ready = sprite_path.exists();
    HttpResponse::Ok().json(serde_json::json!({ "ready": ready }))
}

/// `GET /api/videos/{id}/processing-status` — processing status for a video.
///
/// Returns one of three states:
/// - `{"status":"processed"}` — both deep thumbnail (`.deep` marker) and sprite
///   sheet (`_thumbs/sprite.jpg`) are present for the video
/// - `{"status":"processing"}` — a background worker is actively processing
///   this specific video right now (thumbnail or sprite)
/// - `{"status":"pending"}`   — not fully processed and no worker is currently
///   on this video
///
/// This is a cheap filesystem + lock-read check; it never triggers ffmpeg.
async fn get_processing_status(
    id: web::Path<String>,
    state: web::Data<AppState>,
) -> impl Responder {
    if Uuid::parse_str(&id).is_err() {
        return HttpResponse::BadRequest().body("invalid video id");
    }

    let deep_marker = state.cache_dir.join(format!("{}.deep", *id));
    let sprite_path = state
        .cache_dir
        .join(format!("{}_thumbs", *id))
        .join("sprite.jpg");

    let status = if deep_marker.exists() && sprite_path.exists() {
        "processed"
    } else {
        let thumb_on_this = state
            .thumb_progress
            .read()
            .map(|p| p.current_id.as_deref() == Some(id.as_str()))
            .unwrap_or(false);
        let sprite_on_this = state
            .sprite_progress
            .read()
            .map(|p| p.current_id.as_deref() == Some(id.as_str()))
            .unwrap_or(false);
        if thumb_on_this || sprite_on_this {
            "processing"
        } else {
            "pending"
        }
    };

    HttpResponse::Ok().json(serde_json::json!({ "status": status }))
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

    // Generate the sprite using the shared helper (creates dir, runs ffmpeg).
    if generate_sprite(&id, &abs_path, &state.cache_dir).await {
        match tokio::fs::read(&sprite_path).await {
            Ok(data) => HttpResponse::Ok()
                .content_type("image/jpeg")
                .insert_header((header::CACHE_CONTROL, "public, max-age=86400"))
                .body(data),
            Err(e) => HttpResponse::InternalServerError()
                .body(format!("failed to read sprite: {e}")),
        }
    } else {
        HttpResponse::ServiceUnavailable().body("sprite generation failed")
    }
}

/// Generates the thumbnail sprite sheet for a video.
///
/// Creates `{cache_dir}/{id}_thumbs/sprite.jpg` by running `ffmpeg` with the
/// tile filter.  Returns `true` on success, `false` on any error.  Skips
/// generation if the file already exists.
async fn generate_sprite(id: &str, abs_path: &Path, cache_dir: &Path) -> bool {
    let sprite_dir = cache_dir.join(format!("{}_thumbs", id));
    let sprite_path = sprite_dir.join("sprite.jpg");

    if sprite_path.exists() {
        return true;
    }

    if tokio::fs::create_dir_all(&sprite_dir).await.is_err() {
        return false;
    }

    let (duration_secs, _) = probe_video(abs_path).await;
    if duration_secs == 0 {
        return false;
    }

    let resolved_path = match abs_path.canonicalize() {
        Ok(p) => p,
        Err(_) => return false,
    };
    let abs_str = match resolved_path.to_str() {
        Some(s) => s.to_owned(),
        None => return false,
    };

    let duration = duration_secs as f64;
    let num_thumbnails = ((duration / THUMBNAIL_INTERVAL).ceil() as u32).max(1);
    let columns = THUMBNAILS_PER_ROW.min(num_thumbnails);
    let rows = (num_thumbnails as f64 / columns as f64).ceil() as u32;

    let fps = 1.0 / THUMBNAIL_INTERVAL;
    let tile_layout = format!("{}x{}", columns, rows);
    let scale = format!("scale={}:{}", THUMBNAIL_WIDTH, THUMBNAIL_HEIGHT);

    // Write to a temp file first so that `sprite.jpg` only exists once the
    // file is fully written.  An interrupted or failed ffmpeg run would
    // otherwise leave a partial `sprite.jpg` that the status check would
    // mistake for a completed sprite.
    let tmp_path = sprite_dir.join("sprite.tmp.jpg");
    let tmp_path_str = match tmp_path.to_str() {
        Some(s) => s.to_owned(),
        None => return false,
    };

    let output = Command::new("ffmpeg")
        .stdin(std::process::Stdio::null())
        .args([
            "-y",
            "-nostdin",
            "-hwaccel",
            "auto",
            "-i",
            &abs_str,
            "-vf",
            &format!(
                "fps={},{}:force_original_aspect_ratio=decrease,pad={}:{}:(ow-iw)/2:(oh-ih)/2,tile={}",
                fps, scale, THUMBNAIL_WIDTH, THUMBNAIL_HEIGHT, tile_layout
            ),
            "-frames:v",
            "1",
            "-q:v",
            "5",
            &tmp_path_str,
        ])
        .stdout(std::process::Stdio::null())
        .output()
        .await;

    match output {
        Ok(out) if out.status.success() => {
            // Atomically promote the temp file to the final path so that
            // `sprite.jpg` is never visible in a partially-written state.
            if tokio::fs::rename(&tmp_path, &sprite_path).await.is_ok() {
                true
            } else {
                let _ = tokio::fs::remove_file(&tmp_path).await;
                false
            }
        }
        Ok(out) => {
            let stderr = String::from_utf8_lossy(&out.stderr);
            eprintln!("ffmpeg sprite generation failed for {id}: {stderr}");
            let _ = tokio::fs::remove_file(&tmp_path).await;
            false
        }
        Err(e) => {
            eprintln!("failed to execute ffmpeg for sprite {id}: {e}");
            let _ = tokio::fs::remove_file(&tmp_path).await;
            false
        }
    }
}

/// Background worker that proactively generates sprite sheets for every video.
///
/// Mirrors `run_thumb_worker`: waits for a notification, walks the library,
/// skips videos whose `{id}_thumbs/sprite.jpg` already exists, generates the
/// rest, and updates progress counters as it goes.
async fn run_sprite_worker(
    library_path: PathBuf,
    cache_dir: PathBuf,
    progress: Arc<RwLock<SpriteProgress>>,
    trigger: Arc<tokio::sync::Notify>,
) {
    loop {
        trigger.notified().await;

        let (sprite_done, entries): (Vec<_>, Vec<_>) = WalkDir::new(&library_path)
            .into_iter()
            .filter_map(|e| e.ok())
            .filter(|e| e.file_type().is_file() && is_video(e.path()))
            .partition(|e| {
                let abs = e.path();
                let rel = abs
                    .strip_prefix(&library_path)
                    .unwrap_or(abs)
                    .to_string_lossy();
                let id = video_id(&rel);
                cache_dir
                    .join(format!("{}_thumbs", id))
                    .join("sprite.jpg")
                    .exists()
            });

        {
            let mut p = progress.write().expect("sprite_progress lock poisoned");
            p.current = sprite_done.len() as u32;
            p.total = (sprite_done.len() + entries.len()) as u32;
            p.active = !entries.is_empty();
        }

        for entry in entries {
            let abs = entry.path().to_path_buf();
            let rel = abs
                .strip_prefix(&library_path)
                .unwrap_or(&abs)
                .to_string_lossy()
                .to_string();
            let id = video_id(&rel);

            {
                let mut p = progress.write().expect("sprite_progress lock poisoned");
                p.current_id = Some(id.clone());
            }
            generate_sprite(&id, &abs, &cache_dir).await;

            let mut p = progress.write().expect("sprite_progress lock poisoned");
            p.current_id = None;
            p.current += 1;
            if p.current >= p.total {
                p.active = false;
            }
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

    let cache_dir = PathBuf::from(
        std::env::var("CACHE_DIR").unwrap_or_else(|_| "./starfin_cache".into()),
    );

    if !library_path.exists() {
        std::fs::create_dir_all(&library_path)?;
    }
    std::fs::create_dir_all(&cache_dir)?;

    // Initial library scan at startup.
    println!("→ Scanning library…");
    let initial_items = scan_library(&library_path).await;
    println!("→ Found {} video(s)", initial_items.len());
    let video_cache: Arc<RwLock<Vec<VideoItem>>> = Arc::new(RwLock::new(initial_items));

    println!("→ Detecting GPU hardware acceleration…");
    let hwaccel = detect_hwaccel().await;

    let thumb_progress = Arc::new(RwLock::new(ThumbProgress {
        current: 0,
        total: 0,
        active: false,
        phase: "quick",
        current_id: None,
    }));
    let thumb_trigger = Arc::new(tokio::sync::Notify::new());

    let sprite_progress = Arc::new(RwLock::new(SpriteProgress {
        current: 0,
        total: 0,
        active: false,
        current_id: None,
    }));
    let sprite_trigger = Arc::new(tokio::sync::Notify::new());

    let state = web::Data::new(AppState {
        library_path: library_path.clone(),
        cache_dir: cache_dir.clone(),
        video_cache: Arc::clone(&video_cache),
        last_segment_access: RwLock::new(HashMap::new()),
        thumb_progress: Arc::clone(&thumb_progress),
        thumb_trigger: Arc::clone(&thumb_trigger),
        sprite_progress: Arc::clone(&sprite_progress),
        sprite_trigger: Arc::clone(&sprite_trigger),
        hwaccel,
    });

    // Background task: re-scan the library every 60 seconds.
    let bg_library_path = library_path.clone();
    let bg_cache = Arc::clone(&video_cache);
    let bg_thumb_trigger = Arc::clone(&thumb_trigger);
    let bg_sprite_trigger = Arc::clone(&sprite_trigger);
    tokio::spawn(async move {
        let mut interval = tokio::time::interval(std::time::Duration::from_secs(60));
        interval.tick().await; // skip the immediate tick
        loop {
            interval.tick().await;
            let items = scan_library(&bg_library_path).await;
            *bg_cache.write().expect("video cache lock poisoned") = items;
            bg_thumb_trigger.notify_one();
            bg_sprite_trigger.notify_one();
        }
    });

    // ── Deep thumbnail background worker ─────────────────────────────────────
    {
        let worker_library = library_path.clone();
        let worker_cache = cache_dir.clone();
        let worker_progress = Arc::clone(&thumb_progress);
        let worker_trigger = Arc::clone(&thumb_trigger);
        tokio::spawn(async move {
            run_thumb_worker(worker_library, worker_cache, worker_progress, worker_trigger).await;
        });
        // Kick off the first batch immediately after startup.
        thumb_trigger.notify_one();
    }
    // ─────────────────────────────────────────────────────────────────────────

    // ── Sprite background worker ──────────────────────────────────────────────
    {
        let worker_library = library_path.clone();
        let worker_cache = cache_dir.clone();
        let worker_progress = Arc::clone(&sprite_progress);
        let worker_trigger = Arc::clone(&sprite_trigger);
        tokio::spawn(async move {
            run_sprite_worker(worker_library, worker_cache, worker_progress, worker_trigger).await;
        });
        // Kick off the first batch immediately after startup.
        sprite_trigger.notify_one();
    }
    // ─────────────────────────────────────────────────────────────────────────

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
            .route("/api/hwaccel", web::get().to(get_hwaccel))
            .route("/api/scan/ws", web::get().to(scan_ws))
            .route("/api/progress/ws", web::get().to(progress_ws))
            .route("/api/thumbnails/progress", web::get().to(get_thumb_progress))
            .route("/api/videos", web::get().to(list_videos))
            .route("/api/videos/{id}/thumbnail", web::get().to(get_thumbnail))
            .route(
                "/api/videos/{id}/thumbnails/info",
                web::get().to(get_thumbnail_info),
            )
            .route(
                "/api/videos/{id}/thumbnails/sprite-status",
                web::get().to(get_sprite_status),
            )
            .route(
                "/api/videos/{id}/processing-status",
                web::get().to(get_processing_status),
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


