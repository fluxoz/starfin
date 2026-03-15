use gloo_net::http::Request;
use gloo_timers::callback::Interval;
use gloo_timers::future::TimeoutFuture;
use js_sys::Function;
use serde::Deserialize;
use std::cell::{Cell, RefCell};
use std::rc::Rc;
use wasm_bindgen::prelude::*;
use wasm_bindgen::JsCast;
use wasm_bindgen_futures::spawn_local;
use web_sys::{window, HtmlVideoElement, KeyboardEvent, MouseEvent};
use yew::prelude::*;

// ── Playback speed options ───────────────────────────────────────────────────
const PLAYBACK_SPEEDS: [f64; 9] = [0.25, 0.5, 0.75, 1.0, 1.25, 1.5, 1.75, 2.0, 3.0];

// ── Seek-anchor constants ────────────────────────────────────────────────────
// These must stay in sync with SEGMENT_DURATION, PRECACHE_SEGMENTS, and
// SPARSE_CACHE_STRIDE in `src/main.rs`.
const SEGMENT_DURATION_F: f64 = 6.0;
const PRECACHE_SEGMENTS_F: f64 = 20.0;
const SPARSE_CACHE_STRIDE_F: f64 = 3.0;

/// Snaps a seek time (in seconds) to the **nearest** pre-cached seek anchor.
///
/// Within the dense pre-cache window (`PRECACHE_SEGMENTS_F * SEGMENT_DURATION_F`
/// seconds) every segment is cached so no snapping is needed.  Beyond that
/// window, only every `SPARSE_CACHE_STRIDE_F`-th segment is cached, so we
/// round to whichever anchor boundary (down or up) is closest to `time`.
/// This applies equally to forward and backward seeks.
fn snap_to_nearest_seek_anchor(time: f64) -> f64 {
    if time <= 0.0 {
        return 0.0;
    }
    let dense_window = PRECACHE_SEGMENTS_F * SEGMENT_DURATION_F; // 120 seconds
    if time < dense_window {
        // Within the dense window — every segment is cached, no snapping needed.
        return time;
    }
    // Beyond the dense window — snap to the nearest sparse anchor.
    let stride = SPARSE_CACHE_STRIDE_F as usize;
    let seg_index = (time / SEGMENT_DURATION_F) as usize;
    let anchor_down_idx = (seg_index / stride) * stride;
    let anchor_up_idx = anchor_down_idx + stride;

    let time_down = anchor_down_idx as f64 * SEGMENT_DURATION_F;
    let time_up = anchor_up_idx as f64 * SEGMENT_DURATION_F;

    // Pick whichever anchor is closer; on an exact tie prefer the lower one.
    if (time - time_down) <= (time_up - time) {
        time_down
    } else {
        time_up
    }
}

// ── Stream quality options ────────────────────────────────────────────────────
/// (url-token, display-label) pairs for the quality selector.
const QUALITY_OPTIONS: [(&str, &str); 3] = [
    ("high",   "High"),
    ("medium", "Medium (720p)"),
    ("low",    "Low (480p)"),
];
/// localStorage key used to persist the selected quality across sessions.
const QUALITY_STORAGE_KEY: &str = "starfin_quality";

// ── Controls auto-hide timeout (milliseconds of inactivity) ─────────────────
const CONTROL_HIDE_TIMEOUT_MS: f64 = 5000.0;
/// Pixel distance from the top or bottom edge of the player within which the
/// controls/header are considered "near" and should not be hidden.
const CONTROLS_VICINITY_PX: f64 = 80.0;

// ── HLS.js configuration constants ───────────────────────────────────────────
// These settings are optimized for VOD content with on-demand transcoding.
// The timeouts and retry values are tuned for the latency introduced by
// transcoding segments on the fly (similar to Jellyfin/Plex approach).

/// Maximum buffer length in seconds (forward buffer)
const HLS_MAX_BUFFER_LENGTH: f64 = 30.0;
/// Maximum maximum buffer length in seconds (absolute cap)
const HLS_MAX_MAX_BUFFER_LENGTH: f64 = 60.0;
/// Maximum buffer size in bytes (60 MB)
const HLS_MAX_BUFFER_SIZE: f64 = 60.0 * 1000.0 * 1000.0;
/// Back buffer length in seconds (for backward seeking without refetch)
const HLS_BACK_BUFFER_LENGTH: f64 = 30.0;

/// Fragment loading timeout in milliseconds
/// Higher than default to accommodate on-demand transcoding latency
const HLS_FRAG_LOADING_TIMEOUT_MS: f64 = 20000.0;
/// Maximum retries for fragment loading
const HLS_FRAG_LOADING_MAX_RETRY: f64 = 4.0;
/// Delay between fragment loading retries in milliseconds
const HLS_FRAG_LOADING_RETRY_DELAY_MS: f64 = 1000.0;
/// Maximum total retry timeout for fragment loading in milliseconds
const HLS_FRAG_LOADING_MAX_RETRY_TIMEOUT_MS: f64 = 64000.0;

/// Level loading timeout in milliseconds
const HLS_LEVEL_LOADING_TIMEOUT_MS: f64 = 10000.0;
/// Maximum retries for level loading
const HLS_LEVEL_LOADING_MAX_RETRY: f64 = 4.0;

/// Manifest loading timeout in milliseconds
const HLS_MANIFEST_LOADING_TIMEOUT_MS: f64 = 10000.0;
/// Maximum retries for manifest loading
const HLS_MANIFEST_LOADING_MAX_RETRY: f64 = 2.0;

/// Nudge offset in seconds (helps recover from small stream gaps)
const HLS_NUDGE_OFFSET: f64 = 0.1;
/// Maximum nudge retry count
const HLS_NUDGE_MAX_RETRY: f64 = 5.0;

// ── Helpers ──────────────────────────────────────────────────────────────────

fn format_time(seconds: f64) -> String {
    if !seconds.is_finite() || seconds < 0.0 {
        return "0:00".to_string();
    }
    let total_secs = seconds as u64;
    let hours = total_secs / 3600;
    let mins = (total_secs % 3600) / 60;
    let secs = total_secs % 60;
    if hours > 0 {
        format!("{hours}:{mins:02}:{secs:02}")
    } else {
        format!("{mins}:{secs:02}")
    }
}

fn get_buffer_end(video: &HtmlVideoElement) -> f64 {
    let current_time = video.current_time();
    let buffered = video.buffered();
    for i in 0..buffered.length() {
        if let (Ok(start), Ok(end)) = (buffered.start(i), buffered.end(i)) {
            if current_time >= start && current_time <= end {
                return end;
            }
        }
    }
    0.0
}

// ── Thumbnail Preview State ──────────────────────────────────────────────────

#[derive(Clone, Debug, PartialEq, Deserialize)]
pub struct ThumbnailInfo {
    pub url: String,
    pub sprite_width: u32,
    pub sprite_height: u32,
    pub thumb_width: u32,
    pub thumb_height: u32,
    pub columns: u32,
    pub rows: u32,
    pub interval: f64,
}

// ── Subtitle Track Info ──────────────────────────────────────────────────────

#[derive(Clone, Debug, PartialEq, Deserialize)]
pub struct SubtitleTrack {
    pub index: u32,
    pub language: Option<String>,
    pub title: Option<String>,
    pub codec: String,
}

#[derive(Clone, Debug, PartialEq, Deserialize)]
pub struct SubtitleTracksResponse {
    pub tracks: Vec<SubtitleTrack>,
}

// ── HLS.js bindings ──────────────────────────────────────────────────────────

#[wasm_bindgen]
extern "C" {
    #[wasm_bindgen(js_namespace = Hls)]
    fn isSupported() -> bool;

    #[wasm_bindgen(js_name = Hls)]
    type HlsJs;

    #[wasm_bindgen(constructor, js_class = "Hls")]
    fn new() -> HlsJs;

    #[wasm_bindgen(constructor, js_class = "Hls")]
    fn new_with_config(config: &JsValue) -> HlsJs;

    #[wasm_bindgen(method, js_class = "Hls", js_name = loadSource)]
    fn load_source(this: &HlsJs, url: &str);

    #[wasm_bindgen(method, js_class = "Hls", js_name = attachMedia)]
    fn attach_media(this: &HlsJs, video: &HtmlVideoElement);

    #[wasm_bindgen(method, js_class = "Hls")]
    fn destroy(this: &HlsJs);

    #[wasm_bindgen(method, js_class = "Hls")]
    fn on(this: &HlsJs, event: &str, callback: &Function);

    #[wasm_bindgen(method, js_class = "Hls", js_name = recoverMediaError)]
    fn recover_media_error(this: &HlsJs);

    #[wasm_bindgen(method, js_class = "Hls", js_name = startLoad)]
    fn start_load(this: &HlsJs, start_position: f64);
}

// ── Component ────────────────────────────────────────────────────────────────

#[derive(Properties, PartialEq)]
pub struct VideoPlayerProps {
    pub video_id: String,
    pub title: String,
    pub on_close: Callback<()>,
}

#[function_component(VideoPlayer)]
pub fn video_player(props: &VideoPlayerProps) -> Html {
    let video_ref = use_node_ref();
    let progress_ref = use_node_ref();
    let container_ref = use_node_ref();
    let thumbnail_canvas_ref = use_node_ref();

    // Player state
    let status = use_state(|| "Preparing stream…".to_string());
    let error = use_state(|| Option::<String>::None);
    let current_time = use_state(|| 0.0_f64);
    let duration = use_state(|| 0.0_f64);
    let buffered_end = use_state(|| 0.0_f64);
    let is_playing = use_state(|| false);
    let is_buffering = use_state(|| false);

    // Volume state
    let volume = use_state(|| 1.0_f64);
    let is_muted = use_state(|| false);
    let prev_volume = use_state(|| 1.0_f64);

    // Drag/Seek state
    let is_dragging = use_state(|| false);
    let drag_time = use_state(|| 0.0_f64);
    let just_dragged = use_state(|| false);

    // Hover preview state
    let is_hovering_progress = use_state(|| false);
    let hover_time = use_state(|| 0.0_f64);
    let hover_position = use_state(|| 0.0_f64);

    // UI visibility state
    let controls_visible = use_state(|| true);
    let last_mouse_move = use_mut_ref(|| js_sys::Date::now());
    let is_near_controls = use_mut_ref(|| false);
    let settings_open = use_state(|| false);
    let speed_menu_open = use_state(|| false);
    let quality_menu_open = use_state(|| false);
    let volume_slider_visible = use_state(|| false);

    // Fullscreen state
    let is_fullscreen = use_state(|| false);

    // Playback speed
    let playback_speed = use_state(|| 1.0_f64);

    // Stream quality — initialised from localStorage so the preference
    // persists across sessions.  Defaults to "high" if nothing is stored.
    let initial_quality = window()
        .and_then(|w| w.local_storage().ok())
        .flatten()
        .and_then(|s| s.get_item(QUALITY_STORAGE_KEY).ok())
        .flatten()
        .filter(|q| QUALITY_OPTIONS.iter().any(|(v, _)| v == q))
        .unwrap_or_else(|| "high".to_string());
    let selected_quality = use_state(|| initial_quality);

    // Stores the video position to resume at when quality is changed
    // mid-playback.  Updated by `on_quality_select` before triggering a
    // re-initialisation of HLS.
    let resume_position = use_mut_ref(|| 0.0_f64);

    // Thumbnail sprite info
    let thumbnail_info = use_state(|| Option::<ThumbnailInfo>::None);

    // Double-tap tracking for mobile
    let last_tap_time = use_state(|| 0.0_f64);
    let last_tap_x = use_state(|| 0.0_f64);

    // Skip indicator state
    let skip_indicator = use_state(|| Option::<(String, f64)>::None); // (direction, x_position)

    // Video ended state
    let video_ended = use_state(|| false);

    // Thumbnail sprite image (for canvas rendering)
    let thumbnail_image = use_state(|| Option::<web_sys::HtmlImageElement>::None);

    // Subtitle state
    let subtitle_tracks = use_state(|| Vec::<SubtitleTrack>::new());
    let active_subtitle = use_state(|| Option::<u32>::None); // index of active subtitle, None = off
    let captions_menu_open = use_state(|| false);

    // HLS.js instance storage.
    //
    // We use `use_mut_ref` (Rc<RefCell<…>>) rather than `use_state` here
    // because the HLS instance is set *asynchronously* inside a `spawn_local`
    // block (after a 50 ms delay).  In Yew 0.21, `use_state` clones a new
    // `Rc<R>` on every state update, so a handle captured in a cleanup closure
    // before the async write completes will always read the *initial* `None`
    // value — meaning `hls.destroy()` is never called and stale HLS instances
    // accumulate, each still loading segments at the old quality level.
    //
    // `use_mut_ref` wraps a single `Rc<RefCell<T>>` that is shared by all
    // clones, so any write made by the async task is immediately visible to
    // the cleanup closure captured earlier.
    let hls_instance = use_mut_ref(|| Option::<JsValue>::None);

    // Initialize HLS.js player (and re-initialise when video ID or quality changes).
    {
        let video_ref = video_ref.clone();
        let status = status.clone();
        let error = error.clone();
        let thumbnail_info = thumbnail_info.clone();
        let thumbnail_image = thumbnail_image.clone();
        let subtitle_tracks = subtitle_tracks.clone();
        let hls_instance = hls_instance.clone();
        let selected_quality = selected_quality.clone();
        let resume_position = resume_position.clone();

        use_effect_with(
            (props.video_id.clone(), (*selected_quality).clone()),
            move |(video_id, quality)| {
            let video_id = video_id.clone();
            let quality = quality.clone();

            // Fetch thumbnail info and load sprite (only on first load per video,
            // but harmless to re-fetch on quality change).
            let thumbnail_info_clone = thumbnail_info.clone();
            let thumbnail_image_clone = thumbnail_image.clone();
            let video_id_clone = video_id.clone();
            let video_id_for_subs = video_id.clone();
            let subtitle_tracks_clone = subtitle_tracks.clone();
            
            spawn_local(async move {
                if let Ok(info) = fetch_thumbnail_info(&video_id_clone).await {
                    // Load the sprite image
                    if let Ok(img) = web_sys::HtmlImageElement::new() {
                        let url = info.url.clone();
                        img.set_cross_origin(Some("anonymous"));
                        img.set_src(&url);
                        
                        // Store image after it loads
                        let thumbnail_image_onload = thumbnail_image_clone.clone();
                        let img_clone = img.clone();
                        let onload = Closure::once(Box::new(move || {
                            thumbnail_image_onload.set(Some(img_clone));
                        }) as Box<dyn FnOnce()>);
                        img.set_onload(Some(onload.as_ref().unchecked_ref()));
                        onload.forget();
                    }
                    thumbnail_info_clone.set(Some(info));
                }
            });

            // Fetch subtitle tracks
            spawn_local(async move {
                if let Ok(tracks) = fetch_subtitle_tracks(&video_id_for_subs).await {
                    subtitle_tracks_clone.set(tracks);
                }
            });

            // Read the resume position captured by `on_quality_select`, then
            // reset the ref to 0 so future initialisation starts from the
            // beginning by default.
            let start_pos = *resume_position.borrow();
            *resume_position.borrow_mut() = 0.0;

            // Initialize HLS.js
            let video_ref_clone = video_ref.clone();
            let status_clone = status.clone();
            let error_clone = error.clone();
            let hls_instance_clone = hls_instance.clone();

            spawn_local(async move {
                // Give time for video element to be created
                TimeoutFuture::new(50).await;

                let video = match video_ref_clone.cast::<HtmlVideoElement>() {
                    Some(v) => v,
                    None => {
                        error_clone.set(Some("Video element not found".to_string()));
                        return;
                    }
                };

                // Embed the selected quality in the playlist URL so the server
                // returns segment URLs for the correct quality level.
                let playlist_url = format!(
                    "/api/videos/{}/playlist.m3u8?quality={}",
                    video_id, quality
                );

                // Check if the browser has native HLS support (Safari)
                if !video
                    .can_play_type("application/vnd.apple.mpegurl")
                    .is_empty()
                {
                    // Safari: use native HLS
                    video.set_src(&playlist_url);
                    status_clone.set(String::new());
                    if start_pos > 0.0 {
                        video.set_current_time(start_pos);
                    }
                    let _ = video.play();
                    return;
                }

                // Check if HLS.js is supported
                if !isSupported() {
                    error_clone.set(Some(
                        "Your browser does not support HLS playback. Please use a modern browser.".to_string()
                    ));
                    return;
                }

                // Create HLS.js instance with configuration optimized for VOD seeking
                // Based on industry best practices (similar to Jellyfin/Plex approach)
                // See HLS_* constants at the top of this file for value documentation
                let config = js_sys::Object::new();
                
                // Enable debug logs only in development
                js_sys::Reflect::set(&config, &JsValue::from_str("debug"), &JsValue::from_bool(false)).ok();
                
                // Enable web worker for better UI responsiveness during seeking
                js_sys::Reflect::set(&config, &JsValue::from_str("enableWorker"), &JsValue::from_bool(true)).ok();
                
                // Buffer settings optimized for VOD with on-demand transcoding
                js_sys::Reflect::set(&config, &JsValue::from_str("maxBufferLength"), &JsValue::from_f64(HLS_MAX_BUFFER_LENGTH)).ok();
                js_sys::Reflect::set(&config, &JsValue::from_str("maxMaxBufferLength"), &JsValue::from_f64(HLS_MAX_MAX_BUFFER_LENGTH)).ok();
                js_sys::Reflect::set(&config, &JsValue::from_str("maxBufferSize"), &JsValue::from_f64(HLS_MAX_BUFFER_SIZE)).ok();
                
                // Back buffer settings - keep some played content for backward seeking
                js_sys::Reflect::set(&config, &JsValue::from_str("backBufferLength"), &JsValue::from_f64(HLS_BACK_BUFFER_LENGTH)).ok();
                
                // Start position: -1 means "from the beginning" for a fresh load;
                // a positive value resumes at the position captured before a
                // quality-switch reinitialisation.  Snap to the nearest seek anchor
                // so the resume position is guaranteed to be cached.
                let start_position = if start_pos > 0.0 { snap_to_nearest_seek_anchor(start_pos) } else { -1.0 };
                js_sys::Reflect::set(&config, &JsValue::from_str("startPosition"), &JsValue::from_f64(start_position)).ok();
                
                // Seek handling improvements - nudge settings help recover from small stream gaps
                js_sys::Reflect::set(&config, &JsValue::from_str("nudgeOffset"), &JsValue::from_f64(HLS_NUDGE_OFFSET)).ok();
                js_sys::Reflect::set(&config, &JsValue::from_str("nudgeMaxRetry"), &JsValue::from_f64(HLS_NUDGE_MAX_RETRY)).ok();
                
                // Fragment loading settings for on-demand transcoding
                js_sys::Reflect::set(&config, &JsValue::from_str("fragLoadingTimeOut"), &JsValue::from_f64(HLS_FRAG_LOADING_TIMEOUT_MS)).ok();
                js_sys::Reflect::set(&config, &JsValue::from_str("fragLoadingMaxRetry"), &JsValue::from_f64(HLS_FRAG_LOADING_MAX_RETRY)).ok();
                js_sys::Reflect::set(&config, &JsValue::from_str("fragLoadingRetryDelay"), &JsValue::from_f64(HLS_FRAG_LOADING_RETRY_DELAY_MS)).ok();
                js_sys::Reflect::set(&config, &JsValue::from_str("fragLoadingMaxRetryTimeout"), &JsValue::from_f64(HLS_FRAG_LOADING_MAX_RETRY_TIMEOUT_MS)).ok();
                
                // Level loading settings
                js_sys::Reflect::set(&config, &JsValue::from_str("levelLoadingTimeOut"), &JsValue::from_f64(HLS_LEVEL_LOADING_TIMEOUT_MS)).ok();
                js_sys::Reflect::set(&config, &JsValue::from_str("levelLoadingMaxRetry"), &JsValue::from_f64(HLS_LEVEL_LOADING_MAX_RETRY)).ok();
                
                // Manifest loading settings
                js_sys::Reflect::set(&config, &JsValue::from_str("manifestLoadingTimeOut"), &JsValue::from_f64(HLS_MANIFEST_LOADING_TIMEOUT_MS)).ok();
                js_sys::Reflect::set(&config, &JsValue::from_str("manifestLoadingMaxRetry"), &JsValue::from_f64(HLS_MANIFEST_LOADING_MAX_RETRY)).ok();

                let hls = HlsJs::new_with_config(&config);
                
                // Set up event handlers
                let status_for_manifest = status_clone.clone();
                let video_for_play = video.clone();
                let manifest_parsed_cb = Closure::once(Box::new(move || {
                    status_for_manifest.set(String::new());
                    let _ = video_for_play.play();
                }) as Box<dyn FnOnce()>);
                hls.on("hlsManifestParsed", manifest_parsed_cb.as_ref().unchecked_ref());
                manifest_parsed_cb.forget();

                let error_for_handler = error_clone.clone();
                // Store hls as JsValue for use in closure
                let hls_js_value: JsValue = hls.clone().into();
                let error_cb = Closure::wrap(Box::new(move |_event: JsValue, data: JsValue| {
                    // Get error details from data
                    let fatal = js_sys::Reflect::get(&data, &JsValue::from_str("fatal"))
                        .ok()
                        .and_then(|v| v.as_bool())
                        .unwrap_or(false);
                    
                    let error_type = js_sys::Reflect::get(&data, &JsValue::from_str("type"))
                        .ok()
                        .and_then(|v| v.as_string())
                        .unwrap_or_else(|| "Unknown".to_string());
                    
                    let error_details = js_sys::Reflect::get(&data, &JsValue::from_str("details"))
                        .ok()
                        .and_then(|v| v.as_string())
                        .unwrap_or_else(|| "Unknown".to_string());
                    
                    if fatal {
                        // Try to recover from errors that can happen during seeking
                        // especially with on-demand transcoding where segments may take time
                        if error_type == "mediaError" {
                            log::warn!("Fatal media error detected, attempting recovery: {}", error_details);
                            if let Ok(hls_for_recovery) = hls_js_value.clone().dyn_into::<HlsJs>() {
                                hls_for_recovery.recover_media_error();
                            }
                        } else if error_type == "networkError" {
                            // Network errors during seeking can occur when segments are still
                            // being transcoded. HLS.js will retry automatically based on config,
                            // but for fatal network errors, we try to recover by restarting load.
                            log::warn!("Fatal network error detected, attempting recovery: {}", error_details);
                            if let Ok(hls_for_recovery) = hls_js_value.clone().dyn_into::<HlsJs>() {
                                // Try to restart loading from current position
                                hls_for_recovery.start_load(-1.0);
                            }
                        } else {
                            error_for_handler.set(Some(format!(
                                "HLS playback error: {}. Please try refreshing the page.",
                                error_type
                            )));
                        }
                    } else {
                        // Log non-fatal errors for debugging (these are usually recoverable)
                        log::debug!("Non-fatal HLS error: {} - {}", error_type, error_details);
                    }
                }) as Box<dyn Fn(JsValue, JsValue)>);
                hls.on("hlsError", error_cb.as_ref().unchecked_ref());
                error_cb.forget();

                // Attach media and load source
                status_clone.set("Loading stream…".to_string());
                hls.attach_media(&video);
                hls.load_source(&playlist_url);
                
                // Store HLS instance so the cleanup closure can destroy it.
                // Using borrow_mut() on the shared Rc<RefCell<…>> ensures the
                // write is visible to any clone of the ref, even those captured
                // in closures before this async block ran.
                *hls_instance_clone.borrow_mut() = Some(hls.into());
            });

            // Cleanup function: called by Yew when the dep tuple changes (quality
            // or video ID changes) or when the component unmounts.  We .take() the
            // stored instance so it can never be double-destroyed.
            let hls_instance_for_cleanup = hls_instance.clone();
            move || {
                if let Some(hls_val) = hls_instance_for_cleanup.borrow_mut().take() {
                    if let Ok(hls) = hls_val.dyn_into::<HlsJs>() {
                        hls.destroy();
                    }
                }
            }
        });
    }

    // Effect to draw thumbnail on canvas when hovering
    {
        let thumbnail_canvas_ref = thumbnail_canvas_ref.clone();
        let thumbnail_info = thumbnail_info.clone();
        let thumbnail_image = thumbnail_image.clone();
        let hover_time = hover_time.clone();
        let is_hovering_progress = is_hovering_progress.clone();
        let is_dragging = is_dragging.clone();

        use_effect_with(
            ((*hover_time).clone(), (*is_hovering_progress).clone(), (*is_dragging).clone()),
            move |_| {
                if !*is_hovering_progress && !*is_dragging {
                    return;
                }

                if let (Some(info), Some(img)) = (&*thumbnail_info, &*thumbnail_image) {
                    if let Some(canvas) = thumbnail_canvas_ref.cast::<web_sys::HtmlCanvasElement>() {
                        if let Ok(Some(ctx)) = canvas.get_context("2d") {
                            if let Ok(ctx) = ctx.dyn_into::<web_sys::CanvasRenderingContext2d>() {
                                // Calculate which thumbnail to show based on hover time
                                let thumb_index = if info.interval > 0.0 {
                                    ((*hover_time) / info.interval).floor() as u32
                                } else {
                                    0
                                };
                                
                                let max_index = info.columns * info.rows - 1;
                                let thumb_index = thumb_index.min(max_index);
                                
                                let col = thumb_index % info.columns;
                                let row = thumb_index / info.columns;
                                
                                let sx = (col * info.thumb_width) as f64;
                                let sy = (row * info.thumb_height) as f64;
                                
                                // Clear canvas and draw the thumbnail portion
                                ctx.clear_rect(0.0, 0.0, canvas.width() as f64, canvas.height() as f64);
                                let _ = ctx.draw_image_with_html_image_element_and_sw_and_sh_and_dx_and_dy_and_dw_and_dh(
                                    img,
                                    sx, sy,
                                    info.thumb_width as f64, info.thumb_height as f64,
                                    0.0, 0.0,
                                    canvas.width() as f64, canvas.height() as f64,
                                );
                            }
                        }
                    }
                }
            },
        );
    }

    // Update time/duration periodically
    {
        let video_ref = video_ref.clone();
        let current_time = current_time.clone();
        let duration = duration.clone();
        let buffered_end = buffered_end.clone();
        let is_playing = is_playing.clone();
        let is_dragging = is_dragging.clone();
        let is_buffering = is_buffering.clone();
        let video_ended = video_ended.clone();

        use_effect_with(video_ref.clone(), move |video_ref| {
            let video_ref = video_ref.clone();
            let interval = Interval::new(150, move || {
                if let Some(video) = video_ref.cast::<HtmlVideoElement>() {
                    if !*is_dragging {
                        current_time.set(video.current_time());
                    }
                    let dur = video.duration();
                    if dur.is_finite() && dur > 0.0 {
                        duration.set(dur);
                    }
                    buffered_end.set(get_buffer_end(&video));
                    is_playing.set(!video.paused());

                    // Check buffering state
                    let ready_state = video.ready_state();
                    is_buffering.set(ready_state < 3 && !video.paused());

                    // Check if video ended
                    video_ended.set(video.ended());
                }
            });
            move || drop(interval)
        });
    }

    // Auto-hide controls after inactivity
    {
        let controls_visible = controls_visible.clone();
        let last_mouse_move = last_mouse_move.clone();
        let is_near_controls = is_near_controls.clone();
        let is_playing = is_playing.clone();
        let settings_open = settings_open.clone();
        let quality_menu_open = quality_menu_open.clone();

        use_effect_with(
            ((*is_playing).clone(), (*settings_open).clone(), (*quality_menu_open).clone()),
            move |_| {
                let controls_visible = controls_visible.clone();
                let last_mouse_move = last_mouse_move.clone();
                let is_near_controls = is_near_controls.clone();
                let is_playing = is_playing.clone();
                let settings_open = settings_open.clone();
                let quality_menu_open = quality_menu_open.clone();

                let interval = Interval::new(1000, move || {
                    if *is_playing && !*settings_open && !*quality_menu_open && !*is_near_controls.borrow() {
                        let now = js_sys::Date::now();
                        if now - *last_mouse_move.borrow() > CONTROL_HIDE_TIMEOUT_MS {
                            controls_visible.set(false);
                        }
                    }
                });
                move || drop(interval)
            },
        );
    }

    // Keyboard shortcuts
    {
        let video_ref = video_ref.clone();
        let container_ref = container_ref.clone();
        let is_fullscreen = is_fullscreen.clone();
        let is_muted = is_muted.clone();
        let volume = volume.clone();
        let prev_volume = prev_volume.clone();
        let playback_speed = playback_speed.clone();
        let skip_indicator = skip_indicator.clone();

        use_effect_with(video_ref.clone(), move |_| {
            let video_ref = video_ref.clone();
            let container_ref = container_ref.clone();
            let is_fullscreen = is_fullscreen.clone();
            let is_muted = is_muted.clone();
            let volume = volume.clone();
            let prev_volume = prev_volume.clone();
            let playback_speed = playback_speed.clone();
            let skip_indicator = skip_indicator.clone();

            let closure = Closure::<dyn Fn(KeyboardEvent)>::new(move |e: KeyboardEvent| {
                // Ignore if typing in an input field
                if let Some(target) = e.target() {
                    if let Ok(el) = target.dyn_into::<web_sys::HtmlElement>() {
                        let tag = el.tag_name().to_lowercase();
                        if tag == "input" || tag == "textarea" {
                            return;
                        }
                    }
                }

                let video = match video_ref.cast::<HtmlVideoElement>() {
                    Some(v) => v,
                    None => return,
                };

                let key = e.key();
                match key.as_str() {
                    // Space or K - Play/Pause
                    " " | "k" | "K" => {
                        e.prevent_default();
                        if video.paused() {
                            let _ = video.play();
                        } else {
                            let _ = video.pause();
                        }
                    }
                    // Left arrow or J - Seek backward 5/10 seconds
                    "ArrowLeft" => {
                        e.prevent_default();
                        let skip = if e.shift_key() { 10.0 } else { 5.0 };
                        let current = video.current_time();
                        video.set_current_time(snap_to_nearest_seek_anchor((current - skip).max(0.0)));
                        skip_indicator.set(Some(("backward".to_string(), 25.0)));
                        let skip_indicator_clone = skip_indicator.clone();
                        spawn_local(async move {
                            TimeoutFuture::new(500).await;
                            skip_indicator_clone.set(None);
                        });
                    }
                    "j" | "J" => {
                        e.prevent_default();
                        let current = video.current_time();
                        video.set_current_time(snap_to_nearest_seek_anchor((current - 10.0).max(0.0)));
                        skip_indicator.set(Some(("backward".to_string(), 25.0)));
                        let skip_indicator_clone = skip_indicator.clone();
                        spawn_local(async move {
                            TimeoutFuture::new(500).await;
                            skip_indicator_clone.set(None);
                        });
                    }
                    // Right arrow or L - Seek forward 5/10 seconds
                    "ArrowRight" => {
                        e.prevent_default();
                        let skip = if e.shift_key() { 10.0 } else { 5.0 };
                        let dur = video.duration();
                        if dur.is_finite() {
                            video.set_current_time(snap_to_nearest_seek_anchor((video.current_time() + skip).min(dur)));
                        }
                        skip_indicator.set(Some(("forward".to_string(), 75.0)));
                        let skip_indicator_clone = skip_indicator.clone();
                        spawn_local(async move {
                            TimeoutFuture::new(500).await;
                            skip_indicator_clone.set(None);
                        });
                    }
                    "l" | "L" => {
                        e.prevent_default();
                        let dur = video.duration();
                        if dur.is_finite() {
                            video.set_current_time(snap_to_nearest_seek_anchor((video.current_time() + 10.0).min(dur)));
                        }
                        skip_indicator.set(Some(("forward".to_string(), 75.0)));
                        let skip_indicator_clone = skip_indicator.clone();
                        spawn_local(async move {
                            TimeoutFuture::new(500).await;
                            skip_indicator_clone.set(None);
                        });
                    }
                    // Up arrow - Increase volume
                    "ArrowUp" => {
                        e.prevent_default();
                        let new_vol = (*volume + 0.1).min(1.0);
                        volume.set(new_vol);
                        video.set_volume(new_vol);
                        if new_vol > 0.0 {
                            is_muted.set(false);
                            video.set_muted(false);
                        }
                    }
                    // Down arrow - Decrease volume
                    "ArrowDown" => {
                        e.prevent_default();
                        let new_vol = (*volume - 0.1).max(0.0);
                        volume.set(new_vol);
                        video.set_volume(new_vol);
                    }
                    // M - Toggle mute
                    "m" | "M" => {
                        e.prevent_default();
                        if *is_muted {
                            is_muted.set(false);
                            video.set_muted(false);
                            volume.set(*prev_volume);
                            video.set_volume(*prev_volume);
                        } else {
                            prev_volume.set(*volume);
                            is_muted.set(true);
                            video.set_muted(true);
                        }
                    }
                    // F - Toggle fullscreen
                    "f" | "F" => {
                        e.prevent_default();
                        if let Some(container) = container_ref.cast::<web_sys::HtmlElement>() {
                            let doc = web_sys::window().unwrap().document().unwrap();
                            if doc.fullscreen_element().is_some() {
                                let _ = doc.exit_fullscreen();
                                is_fullscreen.set(false);
                            } else {
                                let _ = container.request_fullscreen();
                                is_fullscreen.set(true);
                            }
                        }
                    }
                    // 0-9 - Seek to percentage
                    "0" | "1" | "2" | "3" | "4" | "5" | "6" | "7" | "8" | "9" => {
                        e.prevent_default();
                        let num: f64 = key.parse().unwrap_or(0.0);
                        let dur = video.duration();
                        if dur.is_finite() {
                            video.set_current_time(snap_to_nearest_seek_anchor(dur * (num / 10.0)));
                        }
                    }
                    // < and > - Decrease/Increase playback speed
                    "<" | "," => {
                        e.prevent_default();
                        let current = *playback_speed;
                        if let Some(pos) =
                            PLAYBACK_SPEEDS.iter().position(|&s| (s - current).abs() < 0.01)
                        {
                            if pos > 0 {
                                let new_speed = PLAYBACK_SPEEDS[pos - 1];
                                playback_speed.set(new_speed);
                                video.set_playback_rate(new_speed);
                            }
                        }
                    }
                    ">" | "." => {
                        e.prevent_default();
                        let current = *playback_speed;
                        if let Some(pos) =
                            PLAYBACK_SPEEDS.iter().position(|&s| (s - current).abs() < 0.01)
                        {
                            if pos < PLAYBACK_SPEEDS.len() - 1 {
                                let new_speed = PLAYBACK_SPEEDS[pos + 1];
                                playback_speed.set(new_speed);
                                video.set_playback_rate(new_speed);
                            }
                        }
                    }
                    // Home - Go to beginning
                    "Home" => {
                        e.prevent_default();
                        video.set_current_time(0.0);
                    }
                    // End - Go to end
                    "End" => {
                        e.prevent_default();
                        let dur = video.duration();
                        if dur.is_finite() {
                            video.set_current_time(dur);
                        }
                    }
                    // P - Toggle Picture-in-Picture
                    "p" | "P" => {
                        e.prevent_default();
                        let doc = web_sys::window().unwrap().document().unwrap();
                        let pip_element = js_sys::Reflect::get(&doc, &JsValue::from_str("pictureInPictureElement"))
                            .ok()
                            .and_then(|v| if v.is_null() || v.is_undefined() { None } else { Some(v) });
                        
                        if pip_element.is_some() {
                            let _ = js_sys::Reflect::get(&doc, &JsValue::from_str("exitPictureInPicture"))
                                .ok()
                                .and_then(|f| f.dyn_ref::<Function>().cloned())
                                .map(|f| f.call0(&doc));
                        } else {
                            let _ = js_sys::Reflect::get(&video, &JsValue::from_str("requestPictureInPicture"))
                                .ok()
                                .and_then(|f| f.dyn_ref::<Function>().cloned())
                                .map(|f| f.call0(&video));
                        }
                    }
                    _ => {}
                }
            });

            if let Some(win) = window() {
                let _ = win.add_event_listener_with_callback(
                    "keydown",
                    closure.as_ref().unchecked_ref(),
                );
            }

            move || {
                if let Some(win) = window() {
                    let _ = win.remove_event_listener_with_callback(
                        "keydown",
                        closure.as_ref().unchecked_ref(),
                    );
                }
                drop(closure);
            }
        });
    }

    let on_close = props.on_close.clone();
    let video_id_for_close = props.video_id.clone();
    let title = props.title.clone();

    // Play/Pause toggle
    let on_play_pause = {
        let video_ref = video_ref.clone();
        let video_ended = video_ended.clone();
        Callback::from(move |_| {
            if let Some(video) = video_ref.cast::<HtmlVideoElement>() {
                if *video_ended {
                    video.set_current_time(0.0);
                }
                if video.paused() {
                    let _ = video.play();
                } else {
                    let _ = video.pause();
                }
            }
        })
    };

    // Mouse move handler — show controls and update vicinity flag
    let on_mouse_move = {
        let controls_visible = controls_visible.clone();
        let last_mouse_move = last_mouse_move.clone();
        let is_near_controls = is_near_controls.clone();
        let container_ref = container_ref.clone();
        Callback::from(move |e: MouseEvent| {
            controls_visible.set(true);
            *last_mouse_move.borrow_mut() = js_sys::Date::now();

            // Update vicinity: keep controls visible if mouse is within
            // CONTROLS_VICINITY_PX of the top (header) or bottom (controls bar).
            if let Some(el) = container_ref.cast::<web_sys::HtmlElement>() {
                let rect = el.get_bounding_client_rect();
                let mouse_y = e.client_y() as f64;
                let dist_from_bottom = (rect.bottom() - mouse_y).max(0.0);
                let dist_from_top = (mouse_y - rect.top()).max(0.0);
                let near = dist_from_bottom < CONTROLS_VICINITY_PX
                    || dist_from_top < CONTROLS_VICINITY_PX;
                *is_near_controls.borrow_mut() = near;
            }
        })
    };

    // Mouse leave handler — clear vicinity flag
    let on_mouse_leave = {
        let is_near_controls = is_near_controls.clone();
        Callback::from(move |_: MouseEvent| {
            *is_near_controls.borrow_mut() = false;
        })
    };

    // Volume toggle (mute/unmute)
    let on_volume_toggle = {
        let video_ref = video_ref.clone();
        let is_muted = is_muted.clone();
        let volume = volume.clone();
        let prev_volume = prev_volume.clone();
        Callback::from(move |_| {
            if let Some(video) = video_ref.cast::<HtmlVideoElement>() {
                if *is_muted {
                    is_muted.set(false);
                    video.set_muted(false);
                    volume.set(*prev_volume);
                    video.set_volume(*prev_volume);
                } else {
                    prev_volume.set(*volume);
                    is_muted.set(true);
                    video.set_muted(true);
                }
            }
        })
    };

    // Volume change
    let on_volume_change = {
        let video_ref = video_ref.clone();
        let volume = volume.clone();
        let is_muted = is_muted.clone();
        Callback::from(move |e: web_sys::InputEvent| {
            if let Some(target) = e.target() {
                if let Ok(input) = target.dyn_into::<web_sys::HtmlInputElement>() {
                    let new_vol: f64 = input.value().parse().unwrap_or(1.0);
                    volume.set(new_vol);
                    if let Some(video) = video_ref.cast::<HtmlVideoElement>() {
                        video.set_volume(new_vol);
                        if new_vol > 0.0 {
                            is_muted.set(false);
                            video.set_muted(false);
                        }
                    }
                }
            }
        })
    };

    // Fullscreen toggle
    let on_fullscreen_toggle = {
        let container_ref = container_ref.clone();
        let is_fullscreen = is_fullscreen.clone();
        Callback::from(move |_| {
            if let Some(container) = container_ref.cast::<web_sys::HtmlElement>() {
                let doc = web_sys::window().unwrap().document().unwrap();
                if doc.fullscreen_element().is_some() {
                    let _ = doc.exit_fullscreen();
                    is_fullscreen.set(false);
                } else {
                    let _ = container.request_fullscreen();
                    is_fullscreen.set(true);
                }
            }
        })
    };

    // Speed menu toggle
    let on_speed_toggle = {
        let speed_menu_open = speed_menu_open.clone();
        let settings_open = settings_open.clone();
        let quality_menu_open = quality_menu_open.clone();
        Callback::from(move |e: MouseEvent| {
            e.stop_propagation();
            speed_menu_open.set(!*speed_menu_open);
            settings_open.set(false);
            quality_menu_open.set(false);
        })
    };

    // Speed selection
    let on_speed_select = {
        let video_ref = video_ref.clone();
        let playback_speed = playback_speed.clone();
        let speed_menu_open = speed_menu_open.clone();
        Callback::from(move |speed: f64| {
            playback_speed.set(speed);
            speed_menu_open.set(false);
            if let Some(video) = video_ref.cast::<HtmlVideoElement>() {
                video.set_playback_rate(speed);
            }
        })
    };

    // Settings toggle
    let on_settings_toggle = {
        let settings_open = settings_open.clone();
        let speed_menu_open = speed_menu_open.clone();
        let quality_menu_open = quality_menu_open.clone();
        Callback::from(move |e: MouseEvent| {
            e.stop_propagation();
            settings_open.set(!*settings_open);
            speed_menu_open.set(false);
            quality_menu_open.set(false);
        })
    };

    // Quality menu toggle
    let on_quality_toggle = {
        let quality_menu_open = quality_menu_open.clone();
        let settings_open = settings_open.clone();
        let speed_menu_open = speed_menu_open.clone();
        Callback::from(move |e: MouseEvent| {
            e.stop_propagation();
            quality_menu_open.set(!*quality_menu_open);
            settings_open.set(false);
            speed_menu_open.set(false);
        })
    };

    // Quality selection — saves preference to localStorage and reinitialises
    // HLS at the new quality level, resuming from the current playback position.
    let on_quality_select = {
        let selected_quality = selected_quality.clone();
        let quality_menu_open = quality_menu_open.clone();
        let video_ref = video_ref.clone();
        let resume_position = resume_position.clone();
        Callback::from(move |quality: String| {
            // Capture the current position before tearing down the old stream.
            if let Some(video) = video_ref.cast::<HtmlVideoElement>() {
                *resume_position.borrow_mut() = video.current_time();
            }
            quality_menu_open.set(false);
            // Persist preference.
            if let Some(storage) = window()
                .and_then(|w| w.local_storage().ok())
                .flatten()
            {
                let _ = storage.set_item(QUALITY_STORAGE_KEY, &quality);
            }
            selected_quality.set(quality);
        })
    };

    // Close menus when clicking outside
    let on_container_click = {
        let settings_open = settings_open.clone();
        let speed_menu_open = speed_menu_open.clone();
        let quality_menu_open = quality_menu_open.clone();
        Callback::from(move |_: MouseEvent| {
            settings_open.set(false);
            speed_menu_open.set(false);
            quality_menu_open.set(false);
        })
    };

    // Helper function to calculate seek time from mouse position
    fn calculate_seek_time(
        e: &MouseEvent,
        progress_el: &web_sys::HtmlElement,
        video_duration: f64,
    ) -> Option<(f64, f64)> {
        let rect = progress_el.get_bounding_client_rect();
        let click_x = e.client_x() as f64 - rect.left();
        let width = rect.width();
        if width > 0.0 && video_duration.is_finite() && video_duration > 0.0 {
            let seek_ratio = (click_x / width).clamp(0.0, 1.0);
            Some((seek_ratio * video_duration, seek_ratio * 100.0))
        } else {
            None
        }
    }

    // Progress bar hover handler
    let on_progress_hover = {
        let progress_ref = progress_ref.clone();
        let is_hovering_progress = is_hovering_progress.clone();
        let hover_time = hover_time.clone();
        let hover_position = hover_position.clone();
        let duration_state = duration.clone();
        Callback::from(move |e: MouseEvent| {
            is_hovering_progress.set(true);
            if let Some(progress_el) = progress_ref.cast::<web_sys::HtmlElement>() {
                if let Some((time, pos)) =
                    calculate_seek_time(&e, &progress_el, *duration_state)
                {
                    hover_time.set(time);
                    hover_position.set(pos);
                }
            }
        })
    };

    // Progress bar leave handler
    let on_progress_leave = {
        let is_hovering_progress = is_hovering_progress.clone();
        Callback::from(move |_: MouseEvent| {
            is_hovering_progress.set(false);
        })
    };

    // Progress bar mousedown - start dragging
    let on_progress_mousedown = {
        let video_ref = video_ref.clone();
        let progress_ref = progress_ref.clone();
        let is_dragging = is_dragging.clone();
        let drag_time = drag_time.clone();
        let current_time = current_time.clone();
        let duration_state = duration.clone();
        let just_dragged = just_dragged.clone();
        let hover_time = hover_time.clone();
        let hover_position = hover_position.clone();

        Callback::from(move |e: MouseEvent| {
            e.prevent_default();

            let progress_el = match progress_ref.cast::<web_sys::HtmlElement>() {
                Some(el) => el,
                None => return,
            };

            let video = match video_ref.cast::<HtmlVideoElement>() {
                Some(v) => v,
                None => return,
            };

            let video_duration = video.duration();
            if !video_duration.is_finite() || video_duration <= 0.0 {
                return;
            }

            // Calculate initial seek position
            let rect = progress_el.get_bounding_client_rect();
            let click_x = e.client_x() as f64 - rect.left();
            let width = rect.width();

            if width <= 0.0 {
                return;
            }

            let seek_ratio = (click_x / width).clamp(0.0, 1.0);
            let initial_seek_time = seek_ratio * video_duration;

            is_dragging.set(true);
            drag_time.set(initial_seek_time);
            current_time.set(initial_seek_time);

            let shared_seek_time: Rc<Cell<f64>> = Rc::new(Cell::new(initial_seek_time));
            let shared_seek_time_move = shared_seek_time.clone();
            let shared_seek_time_up = shared_seek_time.clone();

            let progress_ref_move = progress_ref.clone();
            let duration_for_move = *duration_state;
            let drag_time_move = drag_time.clone();
            let current_time_move = current_time.clone();
            let is_dragging_up = is_dragging.clone();
            let video_ref_up = video_ref.clone();
            let just_dragged_up = just_dragged.clone();
            let hover_time_move = hover_time.clone();
            let hover_position_move = hover_position.clone();

            let closures: Rc<
                RefCell<Option<(Closure<dyn Fn(MouseEvent)>, Closure<dyn Fn(MouseEvent)>)>>,
            > = Rc::new(RefCell::new(None));
            let closures_for_mouseup = closures.clone();

            let on_mousemove = Closure::<dyn Fn(MouseEvent)>::new(move |e: MouseEvent| {
                if let Some(progress_el) = progress_ref_move.cast::<web_sys::HtmlElement>() {
                    let rect = progress_el.get_bounding_client_rect();
                    let click_x = e.client_x() as f64 - rect.left();
                    let width = rect.width();
                    if width > 0.0 && duration_for_move > 0.0 {
                        let seek_ratio = (click_x / width).clamp(0.0, 1.0);
                        let seek_time = seek_ratio * duration_for_move;
                        shared_seek_time_move.set(seek_time);
                        drag_time_move.set(seek_time);
                        current_time_move.set(seek_time);
                        hover_time_move.set(seek_time);
                        hover_position_move.set(seek_ratio * 100.0);
                    }
                }
            });

            let on_mouseup = Closure::<dyn Fn(MouseEvent)>::new(move |_: MouseEvent| {
                is_dragging_up.set(false);
                just_dragged_up.set(true);
                let seek_time = shared_seek_time_up.get();

                if let Some(video) = video_ref_up.cast::<HtmlVideoElement>() {
                    video.set_current_time(snap_to_nearest_seek_anchor(seek_time));
                }

                if let Some((mousemove_closure, mouseup_closure)) =
                    closures_for_mouseup.borrow_mut().take()
                {
                    if let Some(win) = window() {
                        let _ = win.remove_event_listener_with_callback(
                            "mousemove",
                            mousemove_closure.as_ref().unchecked_ref(),
                        );
                        let _ = win.remove_event_listener_with_callback(
                            "mouseup",
                            mouseup_closure.as_ref().unchecked_ref(),
                        );
                    }
                }
            });

            if let Some(win) = window() {
                let _ = win.add_event_listener_with_callback(
                    "mousemove",
                    on_mousemove.as_ref().unchecked_ref(),
                );
                let _ = win.add_event_listener_with_callback(
                    "mouseup",
                    on_mouseup.as_ref().unchecked_ref(),
                );

                *closures.borrow_mut() = Some((on_mousemove, on_mouseup));
            }
        })
    };

    // Click on progress bar
    let on_progress_click = {
        let video_ref = video_ref.clone();
        let progress_ref = progress_ref.clone();
        let just_dragged = just_dragged.clone();
        Callback::from(move |e: MouseEvent| {
            if *just_dragged {
                just_dragged.set(false);
                return;
            }
            if let Some(progress_el) = progress_ref.cast::<web_sys::HtmlElement>() {
                if let Some(video) = video_ref.cast::<HtmlVideoElement>() {
                    let video_duration = video.duration();
                    if let Some((seek_time, _)) =
                        calculate_seek_time(&e, &progress_el, video_duration)
                    {
                        video.set_current_time(snap_to_nearest_seek_anchor(seek_time));
                    }
                }
            }
        })
    };

    // Double-click on video to toggle fullscreen
    let on_video_dblclick = {
        let container_ref = container_ref.clone();
        let is_fullscreen = is_fullscreen.clone();
        Callback::from(move |_: MouseEvent| {
            if let Some(container) = container_ref.cast::<web_sys::HtmlElement>() {
                let doc = web_sys::window().unwrap().document().unwrap();
                if doc.fullscreen_element().is_some() {
                    let _ = doc.exit_fullscreen();
                    is_fullscreen.set(false);
                } else {
                    let _ = container.request_fullscreen();
                    is_fullscreen.set(true);
                }
            }
        })
    };

    // Single click on video to play/pause
    let on_video_click = {
        let video_ref = video_ref.clone();
        let last_tap_time = last_tap_time.clone();
        let last_tap_x = last_tap_x.clone();
        let skip_indicator = skip_indicator.clone();
        Callback::from(move |e: MouseEvent| {
            let now = js_sys::Date::now();
            let x = e.client_x() as f64;

            // Check for double-tap (within 300ms and same general area)
            if now - *last_tap_time < 300.0 {
                // Double tap detected - check which side
                if let Some(video) = video_ref.cast::<HtmlVideoElement>() {
                    let rect = video.get_bounding_client_rect();
                    let width = rect.width();
                    let relative_x = x - rect.left();

                    if relative_x < width / 3.0 {
                        // Left third - seek backward 10 seconds
                        let current = video.current_time();
                        video.set_current_time(snap_to_nearest_seek_anchor((current - 10.0).max(0.0)));
                        skip_indicator.set(Some(("backward".to_string(), 25.0)));
                        let skip_indicator_clone = skip_indicator.clone();
                        spawn_local(async move {
                            TimeoutFuture::new(500).await;
                            skip_indicator_clone.set(None);
                        });
                    } else if relative_x > width * 2.0 / 3.0 {
                        // Right third - seek forward 10 seconds
                        let dur = video.duration();
                        if dur.is_finite() {
                            video.set_current_time(snap_to_nearest_seek_anchor((video.current_time() + 10.0).min(dur)));
                        }
                        skip_indicator.set(Some(("forward".to_string(), 75.0)));
                        let skip_indicator_clone = skip_indicator.clone();
                        spawn_local(async move {
                            TimeoutFuture::new(500).await;
                            skip_indicator_clone.set(None);
                        });
                    }
                }
                last_tap_time.set(0.0);
            } else {
                // Single tap - store time and position for potential double tap
                last_tap_time.set(now);
                last_tap_x.set(x);

                // Delayed play/pause (will be cancelled if double tap occurs)
                let video_ref = video_ref.clone();
                let last_tap_time = last_tap_time.clone();
                spawn_local(async move {
                    TimeoutFuture::new(300).await;
                    // Only trigger if no second tap occurred
                    if *last_tap_time != 0.0 {
                        if let Some(video) = video_ref.cast::<HtmlVideoElement>() {
                            if video.paused() {
                                let _ = video.play();
                            } else {
                                let _ = video.pause();
                            }
                        }
                    }
                });
            }
        })
    };

    // Replay button for video end
    let on_replay = {
        let video_ref = video_ref.clone();
        Callback::from(move |_| {
            if let Some(video) = video_ref.cast::<HtmlVideoElement>() {
                video.set_current_time(0.0);
                let _ = video.play();
            }
        })
    };

    // Picture-in-Picture toggle
    let on_pip_toggle = {
        let video_ref = video_ref.clone();
        Callback::from(move |_| {
            if let Some(video) = video_ref.cast::<HtmlVideoElement>() {
                // Check if PiP is currently active
                let doc = web_sys::window().unwrap().document().unwrap();
                let pip_element = js_sys::Reflect::get(&doc, &JsValue::from_str("pictureInPictureElement"))
                    .ok()
                    .and_then(|v| if v.is_null() || v.is_undefined() { None } else { Some(v) });
                
                if pip_element.is_some() {
                    // Exit PiP
                    let _ = js_sys::Reflect::get(&doc, &JsValue::from_str("exitPictureInPicture"))
                        .ok()
                        .and_then(|f| f.dyn_ref::<Function>().cloned())
                        .map(|f| f.call0(&doc));
                } else {
                    // Enter PiP
                    let _ = js_sys::Reflect::get(&video, &JsValue::from_str("requestPictureInPicture"))
                        .ok()
                        .and_then(|f| f.dyn_ref::<Function>().cloned())
                        .map(|f| f.call0(&video));
                }
            }
        })
    };

    // Captions menu toggle
    let on_captions_toggle = {
        let captions_menu_open = captions_menu_open.clone();
        let settings_open = settings_open.clone();
        let speed_menu_open = speed_menu_open.clone();
        let quality_menu_open = quality_menu_open.clone();
        Callback::from(move |e: MouseEvent| {
            e.stop_propagation();
            captions_menu_open.set(!*captions_menu_open);
            settings_open.set(false);
            speed_menu_open.set(false);
            quality_menu_open.set(false);
        })
    };

    // Caption track selection
    let on_caption_select = {
        let video_ref = video_ref.clone();
        let active_subtitle = active_subtitle.clone();
        let captions_menu_open = captions_menu_open.clone();
        let video_id = props.video_id.clone();
        Callback::from(move |track_index: Option<u32>| {
            captions_menu_open.set(false);
            active_subtitle.set(track_index);
            
            // Add or remove text track from video element
            if let Some(video) = video_ref.cast::<HtmlVideoElement>() {
                // Remove all existing text tracks
                let text_tracks = video.text_tracks();
                if let Some(tracks) = text_tracks {
                    for i in 0..tracks.length() {
                        if let Some(track) = tracks.get(i) {
                            // Hide all tracks
                            track.set_mode(web_sys::TextTrackMode::Hidden);
                        }
                    }
                }
                
                if let Some(index) = track_index {
                    // Create a track element and add it
                    let doc = web_sys::window().unwrap().document().unwrap();
                    if let Ok(track_el) = doc.create_element("track") {
                        track_el.set_attribute("kind", "captions").ok();
                        track_el.set_attribute("src", &format!("/api/videos/{}/subtitles/{}.vtt", video_id, index)).ok();
                        track_el.set_attribute("default", "").ok();
                        
                        // Append to video element
                        video.append_child(&track_el).ok();
                        
                        // Enable the track after appending
                        let text_tracks = video.text_tracks();
                        if let Some(tracks) = text_tracks {
                            if let Some(track) = tracks.get(tracks.length() - 1) {
                                track.set_mode(web_sys::TextTrackMode::Showing);
                            }
                        }
                    }
                }
            }
        })
    };

    // Calculate progress percentages
    let progress_percent = if *duration > 0.0 {
        (*current_time / *duration * 100.0).min(100.0)
    } else {
        0.0
    };
    let buffered_percent = if *duration > 0.0 {
        (*buffered_end / *duration * 100.0).min(100.0)
    } else {
        0.0
    };

    let time_display = format!(
        "{} / {}",
        format_time(*current_time),
        format_time(*duration)
    );
    let play_pause_icon: Html = if *video_ended {
        icon_replay()
    } else if *is_playing {
        icon_pause()
    } else {
        icon_play()
    };

    let volume_icon: Html = if *is_muted || *volume == 0.0 {
        icon_volume_muted()
    } else if *volume < 0.5 {
        icon_volume_low()
    } else {
        icon_volume_high()
    };

    let fullscreen_icon: Html = if *is_fullscreen {
        icon_fullscreen_exit()
    } else {
        icon_fullscreen_enter()
    };

    let controls_class = if *controls_visible {
        "player-controls"
    } else {
        "player-controls player-controls--hidden"
    };

    let container_class = if *is_fullscreen {
        "player-overlay player-overlay--fullscreen"
    } else {
        "player-overlay"
    };

    // Calculate thumbnail preview position
    let preview_style = if *is_hovering_progress || *is_dragging {
        let left = (*hover_position).clamp(5.0, 95.0);
        format!("left: {}%; display: block;", left)
    } else {
        "display: none;".to_string()
    };

    let preview_time = if *is_dragging {
        *drag_time
    } else {
        *hover_time
    };

    html! {
        <div
            ref={container_ref}
            class={container_class}
            onclick={on_container_click}
            onmousemove={on_mouse_move}
            onmouseleave={on_mouse_leave}
        >
            // Header
            <div class={if *controls_visible { "player-header" } else { "player-header player-header--hidden" }}>
                <button
                    class="btn btn--back"
                    onclick={Callback::from(move |_| {
                        let vid = video_id_for_close.clone();
                        spawn_local(async move {
                            clear_video_cache(&vid).await;
                        });
                        on_close.emit(());
                    })}
                >
                    { icon_arrow_back() }
                    { " Back" }
                </button>
                <span class="player-title">{ title }</span>
            </div>

            // Error display
            if let Some(err) = &*error {
                <div class="notice notice--error">
                    <div class="notice__title">{ "Playback error" }</div>
                    <div class="notice__body">{ err }</div>
                </div>
            }

            // Loading status
            if !(*status).is_empty() && (*error).is_none() {
                <div class="player-status">{ &*status }</div>
            }

            // Buffering indicator
            if *is_buffering && (*error).is_none() && (*status).is_empty() {
                <div class="player-buffering">
                    <div class="player-buffering__spinner"></div>
                </div>
            }

            // Skip indicator (for double-tap/keyboard skip)
            if let Some((direction, x_pos)) = &*skip_indicator {
                <div
                    class={format!("skip-indicator skip-indicator--{}", direction)}
                    style={format!("left: {}%;", x_pos)}
                >
                    if direction == "forward" {
                        <span class="skip-indicator__icon">{ icon_skip_forward() }</span>
                        <span class="skip-indicator__text">{ "10s" }</span>
                    } else {
                        <span class="skip-indicator__icon">{ icon_skip_backward() }</span>
                        <span class="skip-indicator__text">{ "10s" }</span>
                    }
                </div>
            }

            // Video element
            <video
                ref={video_ref}
                class="video-el"
                onclick={on_video_click}
                ondblclick={on_video_dblclick}
            />

            // Video end overlay
            if *video_ended {
                <div class="video-end-overlay">
                    <button class="video-end-overlay__replay" onclick={on_replay}>
                        <span class="replay-icon">{ icon_replay() }</span>
                        <span>{ "Replay" }</span>
                    </button>
                </div>
            }

            // Controls bar
            <div class={controls_class}>
                // Progress bar container
                <div class="player-progress-container">
                    // Thumbnail preview
                    <div class="player-preview" style={preview_style}>
                        <canvas ref={thumbnail_canvas_ref} class="player-preview__canvas" width="160" height="90"></canvas>
                        <div class="player-preview__time">{ format_time(preview_time) }</div>
                    </div>

                    // Progress bar
                    <div
                        ref={progress_ref}
                        class="player-progress"
                        onclick={on_progress_click}
                        onmousedown={on_progress_mousedown}
                        onmousemove={on_progress_hover}
                        onmouseleave={on_progress_leave}
                    >
                        <div
                            class="player-progress__buffered"
                            style={format!("width: {}%", buffered_percent)}
                        />
                        <div
                            class="player-progress__played"
                            style={format!("width: {}%", progress_percent)}
                        />
                        // Hover indicator line
                        if *is_hovering_progress || *is_dragging {
                            <div
                                class="player-progress__hover-line"
                                style={format!("left: {}%", if *is_dragging { progress_percent } else { *hover_position })}
                            />
                        }
                        <div
                            class={if *is_dragging { "player-progress__thumb player-progress__thumb--dragging" } else { "player-progress__thumb" }}
                            style={format!("left: {}%", progress_percent)}
                        />
                    </div>
                </div>

                // Bottom controls
                <div class="player-controls__bottom">
                    // Left side controls
                    <div class="player-controls__left">
                        <button class="player-controls__btn" onclick={on_play_pause} title="Play/Pause (k)">
                            { play_pause_icon }
                        </button>

                        // Volume control
                        <div
                            class="player-volume"
                            onmouseenter={Callback::from({
                                let volume_slider_visible = volume_slider_visible.clone();
                                move |_| volume_slider_visible.set(true)
                            })}
                            onmouseleave={Callback::from({
                                let volume_slider_visible = volume_slider_visible.clone();
                                move |_| volume_slider_visible.set(false)
                            })}
                        >
                            <button class="player-controls__btn" onclick={on_volume_toggle} title="Mute (m)">
                                { volume_icon }
                            </button>
                            <div class={if *volume_slider_visible { "player-volume__slider player-volume__slider--visible" } else { "player-volume__slider" }}>
                                <input
                                    type="range"
                                    min="0"
                                    max="1"
                                    step="0.05"
                                    value={volume.to_string()}
                                    oninput={on_volume_change}
                                    class="player-volume__input"
                                />
                            </div>
                        </div>

                        <span class="player-controls__time">{ time_display }</span>
                    </div>

                    // Right side controls
                    <div class="player-controls__right">
                        // Playback speed
                        <div class="player-speed">
                            <button
                                class="player-controls__btn player-controls__btn--text"
                                onclick={on_speed_toggle}
                                title="Playback speed"
                            >
                                { format!("{}x", *playback_speed) }
                            </button>
                            if *speed_menu_open {
                                <div class="player-speed__menu">
                                    { for PLAYBACK_SPEEDS.iter().map(|&speed| {
                                        let on_select = on_speed_select.clone();
                                        let is_active = (*playback_speed - speed).abs() < 0.01;
                                        html! {
                                            <button
                                                class={if is_active { "player-speed__option player-speed__option--active" } else { "player-speed__option" }}
                                                onclick={Callback::from(move |e: MouseEvent| {
                                                    e.stop_propagation();
                                                    on_select.emit(speed);
                                                })}
                                            >
                                                { format!("{}x", speed) }
                                            </button>
                                        }
                                    })}
                                </div>
                            }
                        </div>

                        // Quality selector
                        <div class="player-quality">
                            <button
                                class="player-controls__btn player-controls__btn--text"
                                onclick={on_quality_toggle}
                                title="Stream quality"
                            >
                                { QUALITY_OPTIONS.iter()
                                    .find(|(v, _)| *v == selected_quality.as_str())
                                    .map(|(_, label)| *label)
                                    .unwrap_or("High") }
                            </button>
                            if *quality_menu_open {
                                <div class="player-quality__menu">
                                    { for QUALITY_OPTIONS.iter().map(|(value, label)| {
                                        let on_select = on_quality_select.clone();
                                        let is_active = selected_quality.as_str() == *value;
                                        let value_str = value.to_string();
                                        html! {
                                            <button
                                                class={if is_active { "player-quality__option player-quality__option--active" } else { "player-quality__option" }}
                                                onclick={Callback::from(move |e: MouseEvent| {
                                                    e.stop_propagation();
                                                    on_select.emit(value_str.clone());
                                                })}
                                            >
                                                { *label }
                                            </button>
                                        }
                                    })}
                                </div>
                            }
                        </div>

                        // Captions button (only show if subtitles available)
                        if !subtitle_tracks.is_empty() {
                            <div class="player-captions">
                                <button
                                    class={if active_subtitle.is_some() { "player-controls__btn player-controls__btn--active" } else { "player-controls__btn" }}
                                    onclick={on_captions_toggle}
                                    title="Captions (c)"
                                >
                                    { "CC" }
                                </button>
                                if *captions_menu_open {
                                    <div class="player-captions__menu">
                                        <button
                                            class={if active_subtitle.is_none() { "player-captions__option player-captions__option--active" } else { "player-captions__option" }}
                                            onclick={Callback::from({
                                                let on_select = on_caption_select.clone();
                                                move |e: MouseEvent| {
                                                    e.stop_propagation();
                                                    on_select.emit(None);
                                                }
                                            })}
                                        >
                                            { "Off" }
                                        </button>
                                        { for subtitle_tracks.iter().map(|track| {
                                            let on_select = on_caption_select.clone();
                                            let is_active = *active_subtitle == Some(track.index);
                                            let label = track.title.clone()
                                                .or_else(|| track.language.clone())
                                                .unwrap_or_else(|| format!("Track {}", track.index + 1));
                                            let track_index = track.index;
                                            html! {
                                                <button
                                                    class={if is_active { "player-captions__option player-captions__option--active" } else { "player-captions__option" }}
                                                    onclick={Callback::from(move |e: MouseEvent| {
                                                        e.stop_propagation();
                                                        on_select.emit(Some(track_index));
                                                    })}
                                                >
                                                    { label }
                                                </button>
                                            }
                                        })}
                                    </div>
                                }
                            </div>
                        }

                        // Settings button
                        <div class="player-settings">
                            <button
                                class="player-controls__btn"
                                onclick={on_settings_toggle}
                                title="Settings"
                            >
                                { icon_settings() }
                            </button>
                            if *settings_open {
                                <div class="player-settings__menu">
                                    <div class="player-settings__item">
                                        <span>{ "Quality" }</span>
                                        <span class="player-settings__value">
                                            { QUALITY_OPTIONS.iter()
                                                .find(|(v, _)| *v == selected_quality.as_str())
                                                .map(|(_, label)| *label)
                                                .unwrap_or("High") }
                                        </span>
                                    </div>
                                    <div class="player-settings__item">
                                        <span>{ "Speed" }</span>
                                        <span class="player-settings__value">{ format!("{}x", *playback_speed) }</span>
                                    </div>
                                </div>
                            }
                        </div>

                        // Picture-in-Picture button
                        <button class="player-controls__btn" onclick={on_pip_toggle} title="Picture-in-Picture (p)">
                            { icon_pip() }
                        </button>

                        // Fullscreen button
                        <button class="player-controls__btn" onclick={on_fullscreen_toggle} title="Fullscreen (f)">
                            { fullscreen_icon }
                        </button>
                    </div>
                </div>
            </div>
        </div>
    }
}

// ── Thumbnail Info Fetching ──────────────────────────────────────────────────

async fn fetch_thumbnail_info(video_id: &str) -> Result<ThumbnailInfo, String> {
    let url = format!("/api/videos/{video_id}/thumbnails/info");
    let resp = Request::get(&url)
        .send()
        .await
        .map_err(|e| format!("fetch error: {e:?}"))?;

    if !resp.ok() {
        return Err(format!("HTTP {} for {url}", resp.status()));
    }

    let info: ThumbnailInfo = resp
        .json()
        .await
        .map_err(|e| format!("JSON parse error: {e:?}"))?;
    Ok(info)
}

// ── Subtitle Track Fetching ──────────────────────────────────────────────────

async fn fetch_subtitle_tracks(video_id: &str) -> Result<Vec<SubtitleTrack>, String> {
    let url = format!("/api/videos/{video_id}/subtitles");
    let resp = Request::get(&url)
        .send()
        .await
        .map_err(|e| format!("fetch error: {e:?}"))?;

    if !resp.ok() {
        return Err(format!("HTTP {} for {url}", resp.status()));
    }

    let response: SubtitleTracksResponse = resp
        .json()
        .await
        .map_err(|e| format!("JSON parse error: {e:?}"))?;
    Ok(response.tracks)
}

// ── Cache management ─────────────────────────────────────────────────────────

/// Ask the server to delete the cached segments for `video_id`.
/// Errors are silently ignored – cache clearing is best-effort.
///
/// This is intentionally fire-and-forget: in the browser the underlying
/// `fetch()` request is owned by the browser networking stack and will
/// complete independently of the WASM component lifecycle, so starting
/// it with `spawn_local` before unmounting the player is safe.
/// In the unlikely event the request is lost (e.g. network error), the
/// server's idle-eviction sweep will clear the cache after 10 minutes.
async fn clear_video_cache(video_id: &str) {
    let url = format!("/api/videos/{video_id}/cache");
    if let Err(e) = Request::delete(&url).send().await {
        web_sys::console::warn_1(
            &format!("Failed to clear cache for {video_id}: {e:?}").into(),
        );
    }
}

// ── SVG Icons ────────────────────────────────────────────────────────────────

fn icon_play() -> Html {
    html! {
        <svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 24 24" fill="currentColor" width="1em" height="1em" aria-hidden="true">
            <path d="M8 5v14l11-7z"/>
        </svg>
    }
}

fn icon_pause() -> Html {
    html! {
        <svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 24 24" fill="currentColor" width="1em" height="1em" aria-hidden="true">
            <path d="M6 19h4V5H6v14zm8-14v14h4V5h-4z"/>
        </svg>
    }
}

fn icon_replay() -> Html {
    html! {
        <svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 24 24" fill="currentColor" width="1em" height="1em" aria-hidden="true">
            <path d="M12 5V1L7 6l5 5V7c3.31 0 6 2.69 6 6s-2.69 6-6 6-6-2.69-6-6H4c0 4.42 3.58 8 8 8s8-3.58 8-8-3.58-8-8-8z"/>
        </svg>
    }
}

fn icon_volume_muted() -> Html {
    html! {
        <svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 24 24" fill="currentColor" width="1em" height="1em" aria-hidden="true">
            <path d="M16.5 12c0-1.77-1.02-3.29-2.5-4.03v2.21l2.45 2.45c.03-.2.05-.41.05-.63zm2.5 0c0 .94-.2 1.82-.54 2.64l1.51 1.51C20.63 14.91 21 13.5 21 12c0-4.28-2.99-7.86-7-8.77v2.06c2.89.86 5 3.54 5 6.71zM4.27 3L3 4.27 7.73 9H3v6h4l5 5v-6.73l4.25 4.25c-.67.52-1.42.93-2.25 1.18v2.06c1.38-.31 2.63-.95 3.69-1.81L19.73 21 21 19.73l-9-9L4.27 3zM12 4L9.91 6.09 12 8.18V4z"/>
        </svg>
    }
}

fn icon_volume_low() -> Html {
    html! {
        <svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 24 24" fill="currentColor" width="1em" height="1em" aria-hidden="true">
            <path d="M18.5 12c0-1.77-1.02-3.29-2.5-4.03v8.05c1.48-.73 2.5-2.25 2.5-4.02zM5 9v6h4l5 5V4L9 9H5z"/>
        </svg>
    }
}

fn icon_volume_high() -> Html {
    html! {
        <svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 24 24" fill="currentColor" width="1em" height="1em" aria-hidden="true">
            <path d="M3 9v6h4l5 5V4L7 9H3zm13.5 3c0-1.77-1.02-3.29-2.5-4.03v8.05c1.48-.73 2.5-2.25 2.5-4.02zM14 3.23v2.06c2.89.86 5 3.54 5 6.71s-2.11 5.85-5 6.71v2.06c4.01-.91 7-4.49 7-8.77s-2.99-7.86-7-8.77z"/>
        </svg>
    }
}

fn icon_fullscreen_enter() -> Html {
    html! {
        <svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 24 24" fill="currentColor" width="1em" height="1em" aria-hidden="true">
            <path d="M7 14H5v5h5v-2H7v-3zm-2-4h2V7h3V5H5v5zm12 7h-3v2h5v-5h-2v3zM14 5v2h3v3h2V5h-5z"/>
        </svg>
    }
}

fn icon_fullscreen_exit() -> Html {
    html! {
        <svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 24 24" fill="currentColor" width="1em" height="1em" aria-hidden="true">
            <path d="M5 16h3v3h2v-5H5v2zm3-8H5v2h5V5H8v3zm6 11h2v-3h3v-2h-5v5zm2-11V5h-2v5h5V8h-3z"/>
        </svg>
    }
}

fn icon_settings() -> Html {
    html! {
        <svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 24 24" fill="currentColor" width="1em" height="1em" aria-hidden="true">
            <path d="M19.14 12.94c.04-.3.06-.61.06-.94 0-.32-.02-.64-.07-.94l2.03-1.58c.18-.14.23-.41.12-.61l-1.92-3.32c-.12-.22-.37-.29-.59-.22l-2.39.96c-.5-.38-1.03-.7-1.62-.94l-.36-2.54c-.04-.24-.24-.41-.48-.41h-3.84c-.24 0-.43.17-.47.41l-.36 2.54c-.59.24-1.13.56-1.62.94l-2.39-.96c-.22-.08-.47 0-.59.22L2.74 8.87c-.12.21-.08.47.12.61l2.03 1.58c-.05.3-.09.63-.09.94s.02.64.07.94l-2.03 1.58c-.18.14-.23.41-.12.61l1.92 3.32c.12.22.37.29.59.22l2.39-.96c.5.38 1.03.7 1.62.94l.36 2.54c.05.24.24.41.48.41h3.84c.24 0 .44-.17.47-.41l.36-2.54c.59-.24 1.13-.56 1.62-.94l2.39.96c.22.08.47 0 .59-.22l1.92-3.32c.12-.22.07-.47-.12-.61l-2.01-1.58zM12 15.6c-1.98 0-3.6-1.62-3.6-3.6s1.62-3.6 3.6-3.6 3.6 1.62 3.6 3.6-1.62 3.6-3.6 3.6z"/>
        </svg>
    }
}

fn icon_pip() -> Html {
    html! {
        <svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 24 24" fill="currentColor" width="1em" height="1em" aria-hidden="true">
            <path d="M19 11h-8v6h8v-6zm4 8V4.98C23 3.88 22.1 3 21 3H3C1.9 3 1 3.88 1 4.98V19c0 1.1.9 2 2 2h18c1.1 0 2-.9 2-2zm-2 .02H3V4.97h18v14.05z"/>
        </svg>
    }
}

fn icon_arrow_back() -> Html {
    html! {
        <svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 24 24" fill="currentColor" width="1em" height="1em" aria-hidden="true">
            <path d="M20 11H7.83l5.59-5.59L12 4l-8 8 8 8 1.41-1.41L7.83 13H20v-2z"/>
        </svg>
    }
}

fn icon_skip_forward() -> Html {
    html! {
        <svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 24 24" fill="currentColor" width="1em" height="1em" aria-hidden="true">
            <path d="M4 18l8.5-6L4 6v12zm9-12v12l8.5-6L13 6z"/>
        </svg>
    }
}

fn icon_skip_backward() -> Html {
    html! {
        <svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 24 24" fill="currentColor" width="1em" height="1em" aria-hidden="true">
            <path d="M11 18V6l-8.5 6 8.5 6zm.5-6l8.5 6V6l-8.5 6z"/>
        </svg>
    }
}
