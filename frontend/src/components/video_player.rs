// DASH video player — powered by dash.js (JavaScript library via wasm-bindgen).
//
// Architecture:
//   dash.js MediaPlayer      → handles MSE, MPD parsing, segment fetching, ABR,
//                               buffer management, gap detection, throughput estimation
//   Yew UI component         → custom controls, keyboard shortcuts, quality selection,
//                               subtitles, thumbnails, WebSocket reporting
//
// The UI/controls/styling are preserved from the original component.

use gloo_net::http::Request;
use gloo_timers::callback::Interval;
use gloo_timers::future::TimeoutFuture;
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

// ── Stream quality options ────────────────────────────────────────────────────
const QUALITY_OPTIONS: [(&str, &str); 4] = [
    ("original", "Original (Direct)"),
    ("high",     "High (Transcode)"),
    ("medium",   "Medium (720p)"),
    ("low",      "Low (480p)"),
];
const QUALITY_STORAGE_KEY: &str = "starfin_quality";

// ── Controls auto-hide ───────────────────────────────────────────────────────
const CONTROL_HIDE_TIMEOUT_MS: f64 = 5000.0;
const CONTROLS_VICINITY_PX: f64 = 80.0;

// ── Buffer target for dash.js configuration ──────────────────────────────────
const BUFFER_TARGET_S: f64 = 30.0;
const BACK_BUFFER_S: f64 = 20.0;
const BUFFER_PRUNING_INTERVAL_S: f64 = 30.0;

// ── Gap handling — dash.js will seek past gaps smaller than this ──────────────
const GAP_SMALL_LIMIT_S: f64 = 0.8;
const GAP_STALL_THRESHOLD_S: f64 = 0.5;

// ══════════════════════════════════════════════════════════════════════════════
// dash.js JavaScript interop via wasm-bindgen
// ══════════════════════════════════════════════════════════════════════════════

#[wasm_bindgen]
extern "C" {
    // Access the global `dashjs` object
    #[wasm_bindgen(js_namespace = dashjs)]
    type MediaPlayer;

    // dashjs.MediaPlayer().create() — factory
    #[wasm_bindgen(js_namespace = dashjs, js_name = "MediaPlayer")]
    fn media_player_factory() -> JsValue;
}

/// Wrapper around a dash.js MediaPlayer instance.
struct DashPlayer {
    /// The JS MediaPlayer object
    player: JsValue,
}

impl DashPlayer {
    /// Create a new dash.js MediaPlayer instance.
    fn create() -> Self {
        let factory = media_player_factory();
        let player = js_sys::Reflect::apply(
            &js_sys::Reflect::get(&factory, &"create".into()).unwrap().dyn_into::<js_sys::Function>().unwrap(),
            &factory,
            &js_sys::Array::new(),
        ).unwrap();
        Self { player }
    }

    /// Initialize the player with a video element and manifest URL.
    ///
    /// `start_time` is the optional resume position (seconds from start).
    /// Per the dash.js docs the 4th arg to `initialize()` is `startTime`
    /// which lets the player seek correctly from the very first segment
    /// request rather than loading from the beginning and seeking later.
    fn initialize(&self, video: &HtmlVideoElement, url: &str, auto_play: bool, start_time: f64) {
        let init_fn = js_sys::Reflect::get(&self.player, &"initialize".into())
            .unwrap()
            .dyn_into::<js_sys::Function>()
            .unwrap();
        let args = js_sys::Array::new();
        args.push(video);
        args.push(&JsValue::from_str(url));
        args.push(&JsValue::from_bool(auto_play));
        if start_time > 0.0 {
            args.push(&JsValue::from_f64(start_time));
        }
        let _ = js_sys::Reflect::apply(&init_fn, &self.player, &args);
    }

    /// Seek to a position in seconds.
    ///
    /// This MUST be used instead of `video.set_current_time()` because
    /// dash.js needs to recalculate segment scheduling, buffer ranges,
    /// and ABR state on seek.  Directly setting `video.currentTime`
    /// bypasses all of that, causing buffer underruns at the next
    /// segment boundary.
    fn seek(&self, time: f64) {
        if let Ok(func) = js_sys::Reflect::get(&self.player, &"seek".into()) {
            if let Ok(func) = func.dyn_into::<js_sys::Function>() {
                let _ = func.call1(&self.player, &JsValue::from_f64(time));
            }
        }
    }

    /// Start or resume playback via the dash.js API.
    ///
    /// Using `player.play()` instead of `video.play()` lets dash.js's
    /// internal PlaybackController/ScheduleController react immediately
    /// (e.g. resuming segment scheduling if `scheduleWhilePaused` is
    /// false).
    fn play(&self) {
        if let Ok(func) = js_sys::Reflect::get(&self.player, &"play".into()) {
            if let Ok(func) = func.dyn_into::<js_sys::Function>() {
                let _ = func.call0(&self.player);
            }
        }
    }

    /// Pause playback via the dash.js API.
    fn pause(&self) {
        if let Ok(func) = js_sys::Reflect::get(&self.player, &"pause".into()) {
            if let Ok(func) = func.dyn_into::<js_sys::Function>() {
                let _ = func.call0(&self.player);
            }
        }
    }

    /// Set the playback rate via the dash.js API.
    ///
    /// Using `player.setPlaybackRate()` rather than setting it directly
    /// on the video element lets dash.js fire `PLAYBACK_RATE_CHANGED`
    /// and adjust ABR / scheduling decisions for the new speed.
    fn set_playback_rate(&self, rate: f64) {
        if let Ok(func) = js_sys::Reflect::get(&self.player, &"setPlaybackRate".into()) {
            if let Ok(func) = func.dyn_into::<js_sys::Function>() {
                let _ = func.call1(&self.player, &JsValue::from_f64(rate));
            }
        }
    }

    /// Query whether the player is currently paused.
    fn is_paused(&self) -> bool {
        if let Ok(func) = js_sys::Reflect::get(&self.player, &"isPaused".into()) {
            if let Ok(func) = func.dyn_into::<js_sys::Function>() {
                if let Ok(val) = func.call0(&self.player) {
                    return val.as_bool().unwrap_or(true);
                }
            }
        }
        true
    }

    /// Update dash.js settings.
    fn update_settings(&self, settings: &JsValue) {
        let update_fn = js_sys::Reflect::get(&self.player, &"updateSettings".into())
            .unwrap()
            .dyn_into::<js_sys::Function>()
            .unwrap();
        let _ = update_fn.call1(&self.player, settings);
    }

    /// Register an event listener on the dash.js player.
    fn on(&self, event: &str, callback: &JsValue) {
        let on_fn = js_sys::Reflect::get(&self.player, &"on".into())
            .unwrap()
            .dyn_into::<js_sys::Function>()
            .unwrap();
        let _ = on_fn.call2(&self.player, &JsValue::from_str(event), callback);
    }

    /// Unregister an event listener.
    fn off(&self, event: &str, callback: &JsValue) {
        let off_fn = js_sys::Reflect::get(&self.player, &"off".into())
            .unwrap()
            .dyn_into::<js_sys::Function>()
            .unwrap();
        let _ = off_fn.call2(&self.player, &JsValue::from_str(event), callback);
    }

    /// Get buffer length for a media type ("video" or "audio").
    fn get_buffer_length(&self) -> f64 {
        let get_fn = js_sys::Reflect::get(&self.player, &"getBufferLength".into());
        if let Ok(func) = get_fn {
            if let Ok(func) = func.dyn_into::<js_sys::Function>() {
                if let Ok(val) = func.call0(&self.player) {
                    return val.as_f64().unwrap_or(0.0);
                }
            }
        }
        0.0
    }

    /// Destroy/reset the player.
    fn destroy(&self) {
        if let Ok(func) = js_sys::Reflect::get(&self.player, &"destroy".into()) {
            if let Ok(func) = func.dyn_into::<js_sys::Function>() {
                let _ = func.call0(&self.player);
            }
        }
    }

    /// Get the underlying JsValue.
    fn as_js(&self) -> &JsValue {
        &self.player
    }
}

// ── Server commands ──────────────────────────────────────────────────────────

#[derive(Debug, Clone, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum ServerCommand {
    Play,
    Pause,
    Seek { time: f64 },
    SetQuality { quality: String },
    UpdateSource { video_id: String },
    SetVolume { volume: f64 },
}

fn apply_server_command(
    video: &HtmlVideoElement,
    cmd: &ServerCommand,
    player_ref: &RefCell<Option<Rc<DashPlayer>>>,
) -> bool {
    match cmd {
        ServerCommand::Play => { dash_play(player_ref, video); true }
        ServerCommand::Pause => { dash_pause(player_ref, video); true }
        ServerCommand::Seek { time } => {
            let dur = video.duration();
            if dur.is_finite() && *time >= 0.0 {
                dash_seek(player_ref, video, time.min(dur));
                true
            } else { false }
        }
        ServerCommand::SetVolume { volume } => {
            video.set_volume(volume.clamp(0.0, 1.0));
            true
        }
        ServerCommand::SetQuality { .. } | ServerCommand::UpdateSource { .. } => false,
    }
}

// ── Helpers ──────────────────────────────────────────────────────────────────

fn format_time(seconds: f64) -> String {
    if !seconds.is_finite() || seconds < 0.0 { return "0:00".to_string(); }
    let total_secs = seconds.round() as u64;
    let hours = total_secs / 3600;
    let mins = (total_secs % 3600) / 60;
    let secs = total_secs % 60;
    if hours > 0 { format!("{hours}:{mins:02}:{secs:02}") }
    else { format!("{mins}:{secs:02}") }
}

/// Seek using the dash.js player API when available, otherwise fall back to
/// setting `video.currentTime` directly.  Using `player.seek()` is critical
/// because it lets dash.js recalculate segment scheduling and buffer ranges;
/// setting `currentTime` directly bypasses all internal state management and
/// causes buffer underruns at the next segment boundary.
fn dash_seek(
    player_ref: &RefCell<Option<Rc<DashPlayer>>>,
    video: &HtmlVideoElement,
    time: f64,
) {
    let t = time.max(0.0);
    if let Some(player) = player_ref.borrow().as_ref() {
        player.seek(t);
    } else {
        video.set_current_time(t);
    }
}

/// Play via dash.js when available, otherwise fall back to the video element.
fn dash_play(
    player_ref: &RefCell<Option<Rc<DashPlayer>>>,
    video: &HtmlVideoElement,
) {
    if let Some(player) = player_ref.borrow().as_ref() {
        player.play();
    } else {
        let _ = video.play();
    }
}

/// Pause via dash.js when available, otherwise fall back to the video element.
fn dash_pause(
    player_ref: &RefCell<Option<Rc<DashPlayer>>>,
    video: &HtmlVideoElement,
) {
    if let Some(player) = player_ref.borrow().as_ref() {
        player.pause();
    } else {
        let _ = video.pause();
    }
}

/// Toggle play/pause via dash.js when available.
fn dash_play_pause(
    player_ref: &RefCell<Option<Rc<DashPlayer>>>,
    video: &HtmlVideoElement,
) {
    if let Some(player) = player_ref.borrow().as_ref() {
        if player.is_paused() { player.play(); } else { player.pause(); }
    } else if video.paused() {
        let _ = video.play();
    } else {
        let _ = video.pause();
    }
}

/// Set playback rate via dash.js when available, otherwise directly on the
/// video element.
fn dash_set_playback_rate(
    player_ref: &RefCell<Option<Rc<DashPlayer>>>,
    video: &HtmlVideoElement,
    rate: f64,
) {
    if let Some(player) = player_ref.borrow().as_ref() {
        player.set_playback_rate(rate);
    } else {
        video.set_playback_rate(rate);
    }
}

/// Get the end of the buffered range containing `time`.
fn buffered_end_at(video: &HtmlVideoElement, time: f64) -> f64 {
    let buffered = video.buffered();
    for i in 0..buffered.length() {
        if let (Ok(start), Ok(end)) = (buffered.start(i), buffered.end(i)) {
            if time >= start - 0.15 && time < end + 0.15 {
                return end;
            }
        }
    }
    0.0
}

// ── Thumbnail / Subtitle types ───────────────────────────────────────────────

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

// ══════════════════════════════════════════════════════════════════════════════
// UI COMPONENT
// ══════════════════════════════════════════════════════════════════════════════

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
    let speed_menu_open = use_state(|| false);
    let quality_menu_open = use_state(|| false);
    let volume_slider_visible = use_state(|| false);

    // Fullscreen state
    let is_fullscreen = use_state(|| false);

    // Playback speed
    let playback_speed = use_state(|| 1.0_f64);

    // Stream quality
    let initial_quality = window()
        .and_then(|w| w.local_storage().ok())
        .flatten()
        .and_then(|s| s.get_item(QUALITY_STORAGE_KEY).ok())
        .flatten()
        .filter(|q| QUALITY_OPTIONS.iter().any(|(v, _)| v == q))
        .unwrap_or_else(|| "original".to_string());
    let selected_quality = use_state(|| initial_quality);

    let resume_position = use_mut_ref(|| 0.0_f64);

    // Thumbnail state
    let thumbnail_info = use_state(|| Option::<ThumbnailInfo>::None);
    let thumbnail_image = use_state(|| Option::<web_sys::HtmlImageElement>::None);

    // Double-tap & skip
    let last_tap_time = use_state(|| 0.0_f64);
    let last_tap_x = use_state(|| 0.0_f64);
    let skip_indicator = use_state(|| Option::<(String, f64)>::None);

    // Video ended
    let video_ended = use_state(|| false);

    // Subtitles
    let subtitle_tracks = use_state(|| Vec::<SubtitleTrack>::new());
    let active_subtitle = use_state(|| Option::<u32>::None);
    let captions_menu_open = use_state(|| false);

    // dash.js player state (Rc for async access)
    let dash_player_ref = use_mut_ref(|| Option::<Rc<DashPlayer>>::None);

    // Dynamic quality labels fetched from the server (resolution + Mbps).
    // Falls back to static QUALITY_OPTIONS when not yet loaded.
    let quality_labels: UseStateHandle<Vec<(String, String)>> = use_state(|| Vec::new());

    // Fetch quality info from the server when the video changes.
    {
        let video_id = props.video_id.clone();
        let quality_labels = quality_labels.clone();
        use_effect_with(video_id.clone(), move |video_id| {
            let video_id = video_id.clone();
            let quality_labels = quality_labels.clone();
            spawn_local(async move {
                let url = format!("/api/videos/{}/quality-info", video_id);
                if let Ok(resp) = Request::get(&url).send().await {
                    if resp.ok() {
                        if let Ok(items) = resp.json::<Vec<serde_json::Value>>().await {
                            let labels: Vec<(String, String)> = items.iter().filter_map(|item| {
                                let value = item.get("value")?.as_str()?.to_string();
                                let label = item.get("label")?.as_str()?.to_string();
                                Some((value, label))
                            }).collect();
                            if !labels.is_empty() {
                                quality_labels.set(labels);
                            }
                        }
                    }
                }
            });
            || ()
        });
    }

    // ── Initialize dash.js player ────────────────────────────────────────────
    {
        let video_ref = video_ref.clone();
        let status = status.clone();
        let error = error.clone();
        let thumbnail_info = thumbnail_info.clone();
        let thumbnail_image = thumbnail_image.clone();
        let subtitle_tracks = subtitle_tracks.clone();
        let dash_player_ref = dash_player_ref.clone();
        let selected_quality = selected_quality.clone();
        let resume_position = resume_position.clone();
        let is_buffering = is_buffering.clone();

        use_effect_with(
            (props.video_id.clone(), (*selected_quality).clone()),
            move |(video_id, quality)| {
                let video_id = video_id.clone();
                let quality = quality.clone();

                // Fetch thumbnails
                let thumbnail_info_clone = thumbnail_info.clone();
                let thumbnail_image_clone = thumbnail_image.clone();
                let video_id_clone = video_id.clone();
                spawn_local(async move {
                    if let Ok(info) = fetch_thumbnail_info(&video_id_clone).await {
                        if let Ok(img) = web_sys::HtmlImageElement::new() {
                            let url = info.url.clone();
                            img.set_cross_origin(Some("anonymous"));
                            img.set_src(&url);
                            let img_store = thumbnail_image_clone.clone();
                            let img_clone = img.clone();
                            let onload = Closure::once(Box::new(move || {
                                img_store.set(Some(img_clone));
                            }) as Box<dyn FnOnce()>);
                            img.set_onload(Some(onload.as_ref().unchecked_ref()));
                            onload.forget();
                        }
                        thumbnail_info_clone.set(Some(info));
                    }
                });

                // Fetch subtitles
                let video_id_for_subs = video_id.clone();
                let subtitle_tracks_clone = subtitle_tracks.clone();
                spawn_local(async move {
                    if let Ok(tracks) = fetch_subtitle_tracks(&video_id_for_subs).await {
                        subtitle_tracks_clone.set(tracks);
                    }
                });

                let start_pos = *resume_position.borrow();
                *resume_position.borrow_mut() = 0.0;

                // Initialize dash.js player
                let video_ref_clone = video_ref.clone();
                let status_clone = status.clone();
                let error_clone = error.clone();
                let dash_player_ref_clone = dash_player_ref.clone();
                let is_buffering_clone = is_buffering.clone();

                spawn_local(async move {
                    TimeoutFuture::new(50).await;

                    let video = match video_ref_clone.cast::<HtmlVideoElement>() {
                        Some(v) => v,
                        None => { error_clone.set(Some("Video element not found".into())); return; }
                    };

                    let manifest_url = format!("/api/videos/{}/manifest.mpd?quality={}", video_id, quality);

                    // Create dash.js player
                    let player = DashPlayer::create();

                    // Configure dash.js v5 settings to match our server
                    let settings = js_sys::eval(&format!(
                        r#"({{
                            debug: {{
                                logLevel: 1
                            }},
                            streaming: {{
                                buffer: {{
                                    bufferTimeDefault: {buf_target},
                                    bufferTimeAtTopQuality: {buf_target},
                                    bufferTimeAtTopQualityLongForm: {buf_target},
                                    bufferToKeep: {back_buf},
                                    bufferPruningInterval: {prune_interval},
                                    avoidCurrentTimeRangePruning: true
                                }},
                                gaps: {{
                                    jumpGaps: true,
                                    jumpLargeGaps: true,
                                    smallGapLimit: {gap_small},
                                    threshold: {gap_threshold},
                                    enableSeekFix: true,
                                    enableStallFix: true,
                                    stallSeek: 0.1
                                }},
                                abr: {{
                                    autoSwitchBitrate: {{ video: false, audio: false }}
                                }},
                                errors: {{
                                    recoverAttempts: {{
                                        mediaErrorDecode: 5
                                    }}
                                }},
                                retryAttempts: {{
                                    MPD: 3,
                                    MediaSegment: 3,
                                    InitializationSegment: 3
                                }},
                                retryIntervals: {{
                                    MPD: 1000,
                                    MediaSegment: 1000,
                                    InitializationSegment: 1000
                                }},
                                cacheInitSegments: true
                            }}
                        }})"#,
                        buf_target = BUFFER_TARGET_S,
                        back_buf = BACK_BUFFER_S,
                        prune_interval = BUFFER_PRUNING_INTERVAL_S,
                        gap_small = GAP_SMALL_LIMIT_S,
                        gap_threshold = GAP_STALL_THRESHOLD_S,
                    )).unwrap();

                    player.update_settings(&settings);

                    // Listen for dash.js events
                    // BUFFER_EMPTY / BUFFER_LOADED for buffering indicator
                    let is_buffering_empty = is_buffering_clone.clone();
                    let on_buffer_empty = Closure::<dyn Fn()>::new(move || {
                        is_buffering_empty.set(true);
                    });
                    player.on("bufferStalled", on_buffer_empty.as_ref().unchecked_ref());
                    on_buffer_empty.forget();

                    let is_buffering_loaded = is_buffering_clone.clone();
                    let on_buffer_loaded = Closure::<dyn Fn()>::new(move || {
                        is_buffering_loaded.set(false);
                    });
                    player.on("bufferLoaded", on_buffer_loaded.as_ref().unchecked_ref());
                    on_buffer_loaded.forget();

                    // Error handling
                    let error_handler = error_clone.clone();
                    let on_error = Closure::<dyn Fn(JsValue)>::new(move |e: JsValue| {
                        let msg = if let Some(err_obj) = e.dyn_ref::<js_sys::Object>() {
                            let error_val = js_sys::Reflect::get(err_obj, &"error".into()).unwrap_or(JsValue::UNDEFINED);
                            if let Some(error_obj) = error_val.dyn_ref::<js_sys::Object>() {
                                let msg = js_sys::Reflect::get(error_obj, &"message".into())
                                    .unwrap_or(JsValue::UNDEFINED);
                                msg.as_string().unwrap_or_else(|| format!("{:?}", e))
                            } else {
                                format!("{:?}", e)
                            }
                        } else {
                            format!("{:?}", e)
                        };
                        log::error!("dash.js error: {msg}");
                        error_handler.set(Some(msg));
                    });
                    player.on("error", on_error.as_ref().unchecked_ref());
                    on_error.forget();

                    // Stream initialized event — clear status
                    let status_for_init = status_clone.clone();
                    let on_stream_init = Closure::<dyn Fn()>::new(move || {
                        status_for_init.set(String::new());
                    });
                    player.on("streamInitialized", on_stream_init.as_ref().unchecked_ref());
                    on_stream_init.forget();

                    // Initialize the player — dash.js handles MSE, MPD, segments.
                    // Pass start_pos as the 4th argument so dash.js requests the
                    // correct segments from the start instead of loading from 0
                    // and then seeking (which bypasses internal scheduling).
                    player.initialize(&video, &manifest_url, true, start_pos);

                    status_clone.set(String::new());

                    // Store the player reference
                    let player_rc = Rc::new(player);
                    *dash_player_ref_clone.borrow_mut() = Some(player_rc);
                });

                // Cleanup
                let dash_player_ref_cleanup = dash_player_ref.clone();
                move || {
                    if let Some(player) = dash_player_ref_cleanup.borrow_mut().take() {
                        player.destroy();
                    }
                    // Note: player.destroy() already tears down MSE and resets the
                    // video element.  Do NOT call video.set_src("") after destroy —
                    // it can race with the MediaSource teardown and log errors.
                }
            },
        );
    }

    // ── Thumbnail canvas effect ──────────────────────────────────────────────
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
                if !*is_hovering_progress && !*is_dragging { return; }
                if let (Some(info), Some(img)) = (&*thumbnail_info, &*thumbnail_image) {
                    if let Some(canvas) = thumbnail_canvas_ref.cast::<web_sys::HtmlCanvasElement>() {
                        if let Ok(Some(ctx)) = canvas.get_context("2d") {
                            if let Ok(ctx) = ctx.dyn_into::<web_sys::CanvasRenderingContext2d>() {
                                let thumb_index = if info.interval > 0.0 { ((*hover_time) / info.interval).floor() as u32 } else { 0 };
                                let max_index = info.columns * info.rows - 1;
                                let thumb_index = thumb_index.min(max_index);
                                let col = thumb_index % info.columns;
                                let row = thumb_index / info.columns;
                                let sx = (col * info.thumb_width) as f64;
                                let sy = (row * info.thumb_height) as f64;
                                ctx.clear_rect(0.0, 0.0, canvas.width() as f64, canvas.height() as f64);
                                let _ = ctx.draw_image_with_html_image_element_and_sw_and_sh_and_dx_and_dy_and_dw_and_dh(
                                    img, sx, sy, info.thumb_width as f64, info.thumb_height as f64,
                                    0.0, 0.0, canvas.width() as f64, canvas.height() as f64,
                                );
                            }
                        }
                    }
                }
            },
        );
    }

    // ── Periodic time update ─────────────────────────────────────────────────
    {
        let video_ref = video_ref.clone();
        let current_time = current_time.clone();
        let duration = duration.clone();
        let buffered_end = buffered_end.clone();
        let is_playing = is_playing.clone();
        let is_dragging = is_dragging.clone();
        let video_ended = video_ended.clone();

        use_effect_with(video_ref.clone(), move |video_ref| {
            let video_ref = video_ref.clone();
            let interval = Interval::new(150, move || {
                if let Some(video) = video_ref.cast::<HtmlVideoElement>() {
                    if !*is_dragging { current_time.set(video.current_time()); }
                    let dur = video.duration();
                    if dur.is_finite() && dur > 0.0 { duration.set(dur); }
                    buffered_end.set(buffered_end_at(&video, video.current_time()));
                    is_playing.set(!video.paused());
                    video_ended.set(video.ended());
                }
            });
            move || drop(interval)
        });
    }

    // ── Buffering detection via waiting/playing events ───────────────────────
    {
        let video_ref = video_ref.clone();
        let is_buffering = is_buffering.clone();

        use_effect_with(video_ref.clone(), move |video_ref| {
            let video_opt = video_ref.cast::<HtmlVideoElement>();

            let waiting_cb = video_opt.as_ref().map(|video| {
                let is_buffering = is_buffering.clone();
                let cb = Closure::<dyn Fn()>::new(move || {
                    is_buffering.set(true);
                });
                video.add_event_listener_with_callback("waiting", cb.as_ref().unchecked_ref()).ok();
                cb
            });

            let playing_cb = video_opt.as_ref().map(|video| {
                let is_buffering = is_buffering.clone();
                let cb = Closure::<dyn Fn()>::new(move || { is_buffering.set(false); });
                video.add_event_listener_with_callback("playing", cb.as_ref().unchecked_ref()).ok();
                cb
            });

            let video_opt_cleanup = video_opt.clone();
            move || {
                if let Some(video) = video_opt_cleanup {
                    if let Some(cb) = waiting_cb {
                        video.remove_event_listener_with_callback("waiting", cb.as_ref().unchecked_ref()).ok();
                    }
                    if let Some(cb) = playing_cb {
                        video.remove_event_listener_with_callback("playing", cb.as_ref().unchecked_ref()).ok();
                    }
                }
            }
        });
    }

    // ── Server integration: WebSocket for playback state reporting ────────────
    {
        let video_ref = video_ref.clone();
        let dash_player_ref = dash_player_ref.clone();

        use_effect_with(props.video_id.clone(), move |video_id| {
            let video_id = video_id.clone();
            let video_ref = video_ref.clone();
            let dash_player_ref = dash_player_ref.clone();

            // Fetch resume position from server on mount
            let video_id_resume = video_id.clone();
            let video_ref_resume = video_ref.clone();
            let dash_player_ref_resume = dash_player_ref.clone();
            spawn_local(async move {
                let url = format!("/api/player/position/{}", video_id_resume);
                if let Ok(resp) = Request::get(&url).send().await {
                    if resp.ok() {
                        if let Ok(json) = resp.json::<serde_json::Value>().await {
                            if let Some(time) = json.get("time").and_then(|t| t.as_f64()) {
                                if time > 1.0 {
                                    TimeoutFuture::new(500).await;
                                    if let Some(video) = video_ref_resume.cast::<HtmlVideoElement>() {
                                        let dur = video.duration();
                                        if dur.is_finite() && time < dur - 5.0 {
                                            dash_seek(&dash_player_ref_resume, &video, time);
                                            log::info!("Resumed from server position: {time:.1}s");
                                        }
                                    }
                                }
                            }
                        }
                    }
                }
            });

            // Connect WebSocket for live playback reporting
            let ws_url = {
                let loc = web_sys::window().unwrap().location();
                let proto = if loc.protocol().unwrap_or_default() == "https:" { "wss:" } else { "ws:" };
                let host = loc.host().unwrap_or_default();
                format!("{proto}//{host}/api/player/ws")
            };

            let ws = web_sys::WebSocket::new(&ws_url).ok();

            // Periodic playback state reporter (every 2 seconds)
            let ws_clone = ws.clone();
            let video_ref_report = video_ref.clone();
            let video_id_report = video_id.clone();
            let interval = Interval::new(2000, move || {
                if let Some(ref ws) = ws_clone {
                    if ws.ready_state() == 1 {
                        if let Some(video) = video_ref_report.cast::<HtmlVideoElement>() {
                            let msg = serde_json::json!({
                                "type": "playback_state",
                                "video_id": video_id_report,
                                "time": video.current_time(),
                                "paused": video.paused()
                            });
                            let _ = ws.send_with_str(&msg.to_string());
                        }
                    }
                }
            });

            // Handle incoming server commands
            if let Some(ref ws) = ws {
                let video_ref_cmd = video_ref.clone();
                let dash_player_ref_cmd = dash_player_ref.clone();
                let onmessage = Closure::<dyn Fn(web_sys::MessageEvent)>::new(move |e: web_sys::MessageEvent| {
                    if let Some(text) = e.data().as_string() {
                        if let Ok(cmd) = serde_json::from_str::<ServerCommand>(&text) {
                            if let Some(video) = video_ref_cmd.cast::<HtmlVideoElement>() {
                                apply_server_command(&video, &cmd, &dash_player_ref_cmd);
                            }
                        }
                    }
                });
                ws.set_onmessage(Some(onmessage.as_ref().unchecked_ref()));
                onmessage.forget();
            }

            move || {
                drop(interval);
                if let Some(ws) = ws {
                    let _ = ws.close();
                }
            }
        });
    }

    // ── Auto-hide controls ───────────────────────────────────────────────────
    {
        let controls_visible = controls_visible.clone();
        let last_mouse_move = last_mouse_move.clone();
        let is_near_controls = is_near_controls.clone();
        let is_playing = is_playing.clone();
        let quality_menu_open = quality_menu_open.clone();

        use_effect_with(
            ((*is_playing).clone(), (*quality_menu_open).clone()),
            move |_| {
                let controls_visible = controls_visible.clone();
                let last_mouse_move = last_mouse_move.clone();
                let is_near_controls = is_near_controls.clone();
                let is_playing = is_playing.clone();
                let quality_menu_open = quality_menu_open.clone();
                let interval = Interval::new(1000, move || {
                    if *is_playing && !*quality_menu_open && !*is_near_controls.borrow() {
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

    // ── Keyboard shortcuts ───────────────────────────────────────────────────
    {
        let video_ref = video_ref.clone();
        let container_ref = container_ref.clone();
        let is_fullscreen = is_fullscreen.clone();
        let is_muted = is_muted.clone();
        let volume = volume.clone();
        let prev_volume = prev_volume.clone();
        let playback_speed = playback_speed.clone();
        let skip_indicator = skip_indicator.clone();
        let dash_player_ref = dash_player_ref.clone();

        use_effect_with(video_ref.clone(), move |_| {
            let video_ref = video_ref.clone();
            let container_ref = container_ref.clone();
            let is_fullscreen = is_fullscreen.clone();
            let is_muted = is_muted.clone();
            let volume = volume.clone();
            let prev_volume = prev_volume.clone();
            let playback_speed = playback_speed.clone();
            let skip_indicator = skip_indicator.clone();
            let dash_player_ref = dash_player_ref.clone();

            let closure = Closure::<dyn Fn(KeyboardEvent)>::new(move |e: KeyboardEvent| {
                if let Some(target) = e.target() {
                    if let Ok(el) = target.dyn_into::<web_sys::HtmlElement>() {
                        let tag = el.tag_name().to_lowercase();
                        if tag == "input" || tag == "textarea" { return; }
                    }
                }

                let video = match video_ref.cast::<HtmlVideoElement>() {
                    Some(v) => v,
                    None => return,
                };

                let key = e.key();
                match key.as_str() {
                    " " | "k" | "K" => {
                        e.prevent_default();
                        dash_play_pause(&dash_player_ref, &video);
                    }
                    "ArrowLeft" => {
                        e.prevent_default();
                        let skip = if e.shift_key() { 10.0 } else { 5.0 };
                        dash_seek(&dash_player_ref, &video, (video.current_time() - skip).max(0.0));
                        skip_indicator.set(Some(("backward".to_string(), 25.0)));
                        let si = skip_indicator.clone();
                        spawn_local(async move { TimeoutFuture::new(500).await; si.set(None); });
                    }
                    "j" | "J" => {
                        e.prevent_default();
                        dash_seek(&dash_player_ref, &video, (video.current_time() - 10.0).max(0.0));
                        skip_indicator.set(Some(("backward".to_string(), 25.0)));
                        let si = skip_indicator.clone();
                        spawn_local(async move { TimeoutFuture::new(500).await; si.set(None); });
                    }
                    "ArrowRight" => {
                        e.prevent_default();
                        let skip = if e.shift_key() { 10.0 } else { 5.0 };
                        let dur = video.duration();
                        if dur.is_finite() { dash_seek(&dash_player_ref, &video, (video.current_time() + skip).min(dur)); }
                        skip_indicator.set(Some(("forward".to_string(), 75.0)));
                        let si = skip_indicator.clone();
                        spawn_local(async move { TimeoutFuture::new(500).await; si.set(None); });
                    }
                    "l" | "L" => {
                        e.prevent_default();
                        let dur = video.duration();
                        if dur.is_finite() { dash_seek(&dash_player_ref, &video, (video.current_time() + 10.0).min(dur)); }
                        skip_indicator.set(Some(("forward".to_string(), 75.0)));
                        let si = skip_indicator.clone();
                        spawn_local(async move { TimeoutFuture::new(500).await; si.set(None); });
                    }
                    "ArrowUp" => {
                        e.prevent_default();
                        let new_vol = (*volume + 0.1).min(1.0);
                        volume.set(new_vol);
                        video.set_volume(new_vol);
                        if new_vol > 0.0 { is_muted.set(false); video.set_muted(false); }
                    }
                    "ArrowDown" => {
                        e.prevent_default();
                        let new_vol = (*volume - 0.1).max(0.0);
                        volume.set(new_vol);
                        video.set_volume(new_vol);
                    }
                    "m" | "M" => {
                        e.prevent_default();
                        if *is_muted {
                            is_muted.set(false); video.set_muted(false);
                            volume.set(*prev_volume); video.set_volume(*prev_volume);
                        } else {
                            prev_volume.set(*volume);
                            is_muted.set(true); video.set_muted(true);
                        }
                    }
                    "f" | "F" => {
                        e.prevent_default();
                        if let Some(container) = container_ref.cast::<web_sys::HtmlElement>() {
                            let doc = web_sys::window().unwrap().document().unwrap();
                            if doc.fullscreen_element().is_some() {
                                let _ = doc.exit_fullscreen(); is_fullscreen.set(false);
                            } else {
                                let _ = container.request_fullscreen(); is_fullscreen.set(true);
                            }
                        }
                    }
                    "0"|"1"|"2"|"3"|"4"|"5"|"6"|"7"|"8"|"9" => {
                        e.prevent_default();
                        let num: f64 = key.parse().unwrap_or(0.0);
                        let dur = video.duration();
                        if dur.is_finite() { dash_seek(&dash_player_ref, &video, dur * (num / 10.0)); }
                    }
                    "<" | "," => {
                        e.prevent_default();
                        let current = *playback_speed;
                        if let Some(pos) = PLAYBACK_SPEEDS.iter().position(|&s| (s - current).abs() < 0.01) {
                            if pos > 0 { let ns = PLAYBACK_SPEEDS[pos - 1]; playback_speed.set(ns); dash_set_playback_rate(&dash_player_ref, &video, ns); }
                        }
                    }
                    ">" | "." => {
                        e.prevent_default();
                        let current = *playback_speed;
                        if let Some(pos) = PLAYBACK_SPEEDS.iter().position(|&s| (s - current).abs() < 0.01) {
                            if pos < PLAYBACK_SPEEDS.len() - 1 { let ns = PLAYBACK_SPEEDS[pos + 1]; playback_speed.set(ns); dash_set_playback_rate(&dash_player_ref, &video, ns); }
                        }
                    }
                    "Home" => { e.prevent_default(); dash_seek(&dash_player_ref, &video, 0.0); }
                    "End" => { e.prevent_default(); let dur = video.duration(); if dur.is_finite() { dash_seek(&dash_player_ref, &video, dur); } }
                    _ => {}
                }
            });

            if let Some(win) = window() {
                let _ = win.add_event_listener_with_callback("keydown", closure.as_ref().unchecked_ref());
            }
            move || {
                if let Some(win) = window() {
                    let _ = win.remove_event_listener_with_callback("keydown", closure.as_ref().unchecked_ref());
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
        let dash_player_ref = dash_player_ref.clone();
        Callback::from(move |_| {
            if let Some(video) = video_ref.cast::<HtmlVideoElement>() {
                if *video_ended { dash_seek(&dash_player_ref, &video, 0.0); }
                dash_play_pause(&dash_player_ref, &video);
            }
        })
    };

    let on_mouse_move = {
        let controls_visible = controls_visible.clone();
        let last_mouse_move = last_mouse_move.clone();
        let is_near_controls = is_near_controls.clone();
        let container_ref = container_ref.clone();
        Callback::from(move |e: MouseEvent| {
            controls_visible.set(true);
            *last_mouse_move.borrow_mut() = js_sys::Date::now();
            if let Some(el) = container_ref.cast::<web_sys::HtmlElement>() {
                let rect = el.get_bounding_client_rect();
                let mouse_y = e.client_y() as f64;
                let near = (rect.bottom() - mouse_y).max(0.0) < CONTROLS_VICINITY_PX
                    || (mouse_y - rect.top()).max(0.0) < CONTROLS_VICINITY_PX;
                *is_near_controls.borrow_mut() = near;
            }
        })
    };

    let on_mouse_leave = {
        let is_near_controls = is_near_controls.clone();
        Callback::from(move |_: MouseEvent| { *is_near_controls.borrow_mut() = false; })
    };

    let on_volume_toggle = {
        let video_ref = video_ref.clone();
        let is_muted = is_muted.clone();
        let volume = volume.clone();
        let prev_volume = prev_volume.clone();
        Callback::from(move |_| {
            if let Some(video) = video_ref.cast::<HtmlVideoElement>() {
                if *is_muted {
                    is_muted.set(false); video.set_muted(false);
                    volume.set(*prev_volume); video.set_volume(*prev_volume);
                } else {
                    prev_volume.set(*volume);
                    is_muted.set(true); video.set_muted(true);
                }
            }
        })
    };

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
                        if new_vol > 0.0 { is_muted.set(false); video.set_muted(false); }
                    }
                }
            }
        })
    };

    let on_fullscreen_toggle = {
        let container_ref = container_ref.clone();
        let is_fullscreen = is_fullscreen.clone();
        Callback::from(move |_| {
            if let Some(container) = container_ref.cast::<web_sys::HtmlElement>() {
                let doc = web_sys::window().unwrap().document().unwrap();
                if doc.fullscreen_element().is_some() {
                    let _ = doc.exit_fullscreen(); is_fullscreen.set(false);
                } else {
                    let _ = container.request_fullscreen(); is_fullscreen.set(true);
                }
            }
        })
    };

    let on_speed_toggle = {
        let speed_menu_open = speed_menu_open.clone();
        let quality_menu_open = quality_menu_open.clone();
        Callback::from(move |e: MouseEvent| {
            e.stop_propagation();
            speed_menu_open.set(!*speed_menu_open);
            quality_menu_open.set(false);
        })
    };

    let on_speed_select = {
        let video_ref = video_ref.clone();
        let playback_speed = playback_speed.clone();
        let speed_menu_open = speed_menu_open.clone();
        let dash_player_ref = dash_player_ref.clone();
        Callback::from(move |speed: f64| {
            playback_speed.set(speed);
            speed_menu_open.set(false);
            if let Some(video) = video_ref.cast::<HtmlVideoElement>() {
                dash_set_playback_rate(&dash_player_ref, &video, speed);
            }
        })
    };

    let on_quality_toggle = {
        let quality_menu_open = quality_menu_open.clone();
        let speed_menu_open = speed_menu_open.clone();
        Callback::from(move |e: MouseEvent| {
            e.stop_propagation();
            quality_menu_open.set(!*quality_menu_open);
            speed_menu_open.set(false);
        })
    };

    let on_quality_select = {
        let selected_quality = selected_quality.clone();
        let quality_menu_open = quality_menu_open.clone();
        let video_ref = video_ref.clone();
        let resume_position = resume_position.clone();
        Callback::from(move |quality: String| {
            if let Some(video) = video_ref.cast::<HtmlVideoElement>() {
                *resume_position.borrow_mut() = video.current_time();
            }
            quality_menu_open.set(false);
            if let Some(storage) = window().and_then(|w| w.local_storage().ok()).flatten() {
                let _ = storage.set_item(QUALITY_STORAGE_KEY, &quality);
            }
            selected_quality.set(quality);
        })
    };

    let on_container_click = {
        let speed_menu_open = speed_menu_open.clone();
        let quality_menu_open = quality_menu_open.clone();
        Callback::from(move |_: MouseEvent| {
            speed_menu_open.set(false);
            quality_menu_open.set(false);
        })
    };

    fn calculate_seek_time(e: &MouseEvent, progress_el: &web_sys::HtmlElement, video_duration: f64) -> Option<(f64, f64)> {
        let rect = progress_el.get_bounding_client_rect();
        let click_x = e.client_x() as f64 - rect.left();
        let width = rect.width();
        if width > 0.0 && video_duration.is_finite() && video_duration > 0.0 {
            let ratio = (click_x / width).clamp(0.0, 1.0);
            Some((ratio * video_duration, ratio * 100.0))
        } else { None }
    }

    let on_progress_hover = {
        let progress_ref = progress_ref.clone();
        let is_hovering_progress = is_hovering_progress.clone();
        let hover_time = hover_time.clone();
        let hover_position = hover_position.clone();
        let duration_state = duration.clone();
        Callback::from(move |e: MouseEvent| {
            is_hovering_progress.set(true);
            if let Some(el) = progress_ref.cast::<web_sys::HtmlElement>() {
                if let Some((time, pos)) = calculate_seek_time(&e, &el, *duration_state) {
                    hover_time.set(time);
                    hover_position.set(pos);
                }
            }
        })
    };

    let on_progress_leave = {
        let is_hovering_progress = is_hovering_progress.clone();
        Callback::from(move |_: MouseEvent| { is_hovering_progress.set(false); })
    };

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
        let dash_player_ref = dash_player_ref.clone();

        Callback::from(move |e: MouseEvent| {
            e.prevent_default();
            let progress_el = match progress_ref.cast::<web_sys::HtmlElement>() { Some(el) => el, None => return };
            let video = match video_ref.cast::<HtmlVideoElement>() { Some(v) => v, None => return };
            let video_duration = video.duration();
            if !video_duration.is_finite() || video_duration <= 0.0 { return; }

            let rect = progress_el.get_bounding_client_rect();
            let click_x = e.client_x() as f64 - rect.left();
            let width = rect.width();
            if width <= 0.0 { return; }

            let seek_ratio = (click_x / width).clamp(0.0, 1.0);
            let initial_seek_time = seek_ratio * video_duration;

            is_dragging.set(true);
            drag_time.set(initial_seek_time);
            current_time.set(initial_seek_time);

            let shared_seek_time: Rc<Cell<f64>> = Rc::new(Cell::new(initial_seek_time));
            let shared_move = shared_seek_time.clone();
            let shared_up = shared_seek_time.clone();

            let progress_ref_move = progress_ref.clone();
            let duration_for_move = *duration_state;
            let drag_time_move = drag_time.clone();
            let current_time_move = current_time.clone();
            let is_dragging_up = is_dragging.clone();
            let video_ref_up = video_ref.clone();
            let just_dragged_up = just_dragged.clone();
            let hover_time_move = hover_time.clone();
            let hover_position_move = hover_position.clone();

            let closures: Rc<RefCell<Option<(Closure<dyn Fn(MouseEvent)>, Closure<dyn Fn(MouseEvent)>)>>> = Rc::new(RefCell::new(None));
            let closures_for_mouseup = closures.clone();
            let dash_player_ref_up = dash_player_ref.clone();

            let on_mousemove = Closure::<dyn Fn(MouseEvent)>::new(move |e: MouseEvent| {
                if let Some(el) = progress_ref_move.cast::<web_sys::HtmlElement>() {
                    let rect = el.get_bounding_client_rect();
                    let cx = e.client_x() as f64 - rect.left();
                    let w = rect.width();
                    if w > 0.0 && duration_for_move > 0.0 {
                        let ratio = (cx / w).clamp(0.0, 1.0);
                        let t = ratio * duration_for_move;
                        shared_move.set(t);
                        drag_time_move.set(t);
                        current_time_move.set(t);
                        hover_time_move.set(t);
                        hover_position_move.set(ratio * 100.0);
                    }
                }
            });

            let on_mouseup = Closure::<dyn Fn(MouseEvent)>::new(move |_: MouseEvent| {
                is_dragging_up.set(false);
                just_dragged_up.set(true);
                let t = shared_up.get();
                if let Some(video) = video_ref_up.cast::<HtmlVideoElement>() { dash_seek(&dash_player_ref_up, &video, t); }
                if let Some((mc, uc)) = closures_for_mouseup.borrow_mut().take() {
                    if let Some(win) = window() {
                        let _ = win.remove_event_listener_with_callback("mousemove", mc.as_ref().unchecked_ref());
                        let _ = win.remove_event_listener_with_callback("mouseup", uc.as_ref().unchecked_ref());
                    }
                }
            });

            if let Some(win) = window() {
                let _ = win.add_event_listener_with_callback("mousemove", on_mousemove.as_ref().unchecked_ref());
                let _ = win.add_event_listener_with_callback("mouseup", on_mouseup.as_ref().unchecked_ref());
                *closures.borrow_mut() = Some((on_mousemove, on_mouseup));
            }
        })
    };

    let on_progress_click = {
        let video_ref = video_ref.clone();
        let progress_ref = progress_ref.clone();
        let just_dragged = just_dragged.clone();
        let dash_player_ref = dash_player_ref.clone();
        Callback::from(move |e: MouseEvent| {
            if *just_dragged { just_dragged.set(false); return; }
            if let Some(el) = progress_ref.cast::<web_sys::HtmlElement>() {
                if let Some(video) = video_ref.cast::<HtmlVideoElement>() {
                    if let Some((t, _)) = calculate_seek_time(&e, &el, video.duration()) {
                        dash_seek(&dash_player_ref, &video, t);
                    }
                }
            }
        })
    };

    let on_video_dblclick = {
        let container_ref = container_ref.clone();
        let is_fullscreen = is_fullscreen.clone();
        Callback::from(move |_: MouseEvent| {
            if let Some(container) = container_ref.cast::<web_sys::HtmlElement>() {
                let doc = web_sys::window().unwrap().document().unwrap();
                if doc.fullscreen_element().is_some() {
                    let _ = doc.exit_fullscreen(); is_fullscreen.set(false);
                } else {
                    let _ = container.request_fullscreen(); is_fullscreen.set(true);
                }
            }
        })
    };

    let on_video_click = {
        let video_ref = video_ref.clone();
        let last_tap_time = last_tap_time.clone();
        let last_tap_x = last_tap_x.clone();
        let skip_indicator = skip_indicator.clone();
        let dash_player_ref = dash_player_ref.clone();
        Callback::from(move |e: MouseEvent| {
            let now = js_sys::Date::now();
            let x = e.client_x() as f64;
            if now - *last_tap_time < 300.0 {
                if let Some(video) = video_ref.cast::<HtmlVideoElement>() {
                    let rect = video.get_bounding_client_rect();
                    let w = rect.width();
                    let rx = x - rect.left();
                    if rx < w / 3.0 {
                        dash_seek(&dash_player_ref, &video, (video.current_time() - 10.0).max(0.0));
                        skip_indicator.set(Some(("backward".to_string(), 25.0)));
                        let si = skip_indicator.clone();
                        spawn_local(async move { TimeoutFuture::new(500).await; si.set(None); });
                    } else if rx > w * 2.0 / 3.0 {
                        let dur = video.duration();
                        if dur.is_finite() { dash_seek(&dash_player_ref, &video, (video.current_time() + 10.0).min(dur)); }
                        skip_indicator.set(Some(("forward".to_string(), 75.0)));
                        let si = skip_indicator.clone();
                        spawn_local(async move { TimeoutFuture::new(500).await; si.set(None); });
                    }
                }
                last_tap_time.set(0.0);
            } else {
                last_tap_time.set(now);
                last_tap_x.set(x);
                let video_ref = video_ref.clone();
                let last_tap_time = last_tap_time.clone();
                let dash_player_ref = dash_player_ref.clone();
                spawn_local(async move {
                    TimeoutFuture::new(300).await;
                    if *last_tap_time != 0.0 {
                        if let Some(video) = video_ref.cast::<HtmlVideoElement>() {
                            dash_play_pause(&dash_player_ref, &video);
                        }
                    }
                });
            }
        })
    };

    let on_replay = {
        let video_ref = video_ref.clone();
        let dash_player_ref = dash_player_ref.clone();
        Callback::from(move |_| {
            if let Some(video) = video_ref.cast::<HtmlVideoElement>() {
                dash_seek(&dash_player_ref, &video, 0.0);
                dash_play(&dash_player_ref, &video);
            }
        })
    };

    let on_captions_toggle = {
        let captions_menu_open = captions_menu_open.clone();
        let speed_menu_open = speed_menu_open.clone();
        let quality_menu_open = quality_menu_open.clone();
        Callback::from(move |e: MouseEvent| {
            e.stop_propagation();
            captions_menu_open.set(!*captions_menu_open);
            speed_menu_open.set(false);
            quality_menu_open.set(false);
        })
    };

    let on_caption_select = {
        let video_ref = video_ref.clone();
        let active_subtitle = active_subtitle.clone();
        let captions_menu_open = captions_menu_open.clone();
        let video_id = props.video_id.clone();
        Callback::from(move |track_index: Option<u32>| {
            captions_menu_open.set(false);
            active_subtitle.set(track_index);
            if let Some(video) = video_ref.cast::<HtmlVideoElement>() {
                let text_tracks = video.text_tracks();
                if let Some(tracks) = text_tracks {
                    for i in 0..tracks.length() {
                        if let Some(track) = tracks.get(i) {
                            track.set_mode(web_sys::TextTrackMode::Hidden);
                        }
                    }
                }
                if let Some(index) = track_index {
                    let doc = web_sys::window().unwrap().document().unwrap();
                    if let Ok(track_el) = doc.create_element("track") {
                        track_el.set_attribute("kind", "captions").ok();
                        track_el.set_attribute("src", &format!("/api/videos/{}/subtitles/{}.vtt", video_id, index)).ok();
                        track_el.set_attribute("default", "").ok();
                        video.append_child(&track_el).ok();
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
    let progress_percent = if *duration > 0.0 { (*current_time / *duration * 100.0).min(100.0) } else { 0.0 };
    let buffered_percent = if *duration > 0.0 { (*buffered_end / *duration * 100.0).min(100.0) } else { 0.0 };

    let time_display = format!("{} / {}", format_time(*current_time), format_time(*duration));
    let play_pause_icon: Html = if *video_ended { icon_replay() } else if *is_playing { icon_pause() } else { icon_play() };
    let volume_icon: Html = if *is_muted || *volume == 0.0 { icon_volume_muted() } else if *volume < 0.5 { icon_volume_low() } else { icon_volume_high() };
    let fullscreen_icon: Html = if *is_fullscreen { icon_fullscreen_exit() } else { icon_fullscreen_enter() };

    let controls_class = if *controls_visible { "player-controls" } else { "player-controls player-controls--hidden" };
    let container_class = if *is_fullscreen { "player-overlay player-overlay--fullscreen" } else { "player-overlay" };

    let preview_style = if *is_hovering_progress || *is_dragging {
        let left = (*hover_position).clamp(5.0, 95.0);
        format!("left: {}%; display: block;", left)
    } else { "display: none;".to_string() };

    let preview_time = if *is_dragging { *drag_time } else { *hover_time };

    html! {
        <div ref={container_ref} class={container_class} onclick={on_container_click} onmousemove={on_mouse_move} onmouseleave={on_mouse_leave}>
            // Header
            <div class={if *controls_visible { "player-header" } else { "player-header player-header--hidden" }}>
                <button class="btn btn--back" onclick={Callback::from(move |_| {
                    let vid = video_id_for_close.clone();
                    spawn_local(async move { clear_video_cache(&vid).await; });
                    on_close.emit(());
                })}>
                    { icon_arrow_back() }{ " Back" }
                </button>
                <span class="player-title">{ title }</span>
            </div>

            if let Some(err) = &*error {
                <div class="notice notice--error">
                    <div class="notice__title">{ "Playback error" }</div>
                    <div class="notice__body">{ err }</div>
                </div>
            }

            if !(*status).is_empty() && (*error).is_none() {
                <div class="player-status">{ &*status }</div>
            }

            if *is_buffering && (*error).is_none() && (*status).is_empty() {
                <div class="player-buffering"><div class="player-buffering__spinner"></div></div>
            }

            if let Some((direction, x_pos)) = &*skip_indicator {
                <div class={format!("skip-indicator skip-indicator--{}", direction)} style={format!("left: {}%;", x_pos)}>
                    if direction == "forward" {
                        <span class="skip-indicator__icon">{ icon_skip_forward() }</span>
                        <span class="skip-indicator__text">{ "10s" }</span>
                    } else {
                        <span class="skip-indicator__icon">{ icon_skip_backward() }</span>
                        <span class="skip-indicator__text">{ "10s" }</span>
                    }
                </div>
            }

            <video ref={video_ref} class="video-el" onclick={on_video_click} ondblclick={on_video_dblclick} />

            if *video_ended {
                <div class="video-end-overlay">
                    <button class="video-end-overlay__replay" onclick={on_replay}>
                        <span class="replay-icon">{ icon_replay() }</span>
                        <span>{ "Replay" }</span>
                    </button>
                </div>
            }

            <div class={controls_class}>
                <div class="player-progress-container">
                    <div class="player-preview" style={preview_style}>
                        <canvas ref={thumbnail_canvas_ref} class="player-preview__canvas" width="160" height="90"></canvas>
                        <div class="player-preview__time">{ format_time(preview_time) }</div>
                    </div>
                    <div ref={progress_ref} class="player-progress" onclick={on_progress_click} onmousedown={on_progress_mousedown} onmousemove={on_progress_hover} onmouseleave={on_progress_leave}>
                        <div class="player-progress__buffered" style={format!("width: {}%", buffered_percent)} />
                        <div class="player-progress__played" style={format!("width: {}%", progress_percent)} />
                        if *is_hovering_progress || *is_dragging {
                            <div class="player-progress__hover-line" style={format!("left: {}%", if *is_dragging { progress_percent } else { *hover_position })} />
                        }
                        <div class={if *is_dragging { "player-progress__thumb player-progress__thumb--dragging" } else { "player-progress__thumb" }} style={format!("left: {}%", progress_percent)} />
                    </div>
                </div>

                <div class="player-controls__bottom">
                    <div class="player-controls__left">
                        <button class="player-controls__btn" onclick={on_play_pause} title="Play/Pause (k)">{ play_pause_icon }</button>
                        <div class="player-volume"
                            onmouseenter={Callback::from({ let v = volume_slider_visible.clone(); move |_| v.set(true) })}
                            onmouseleave={Callback::from({ let v = volume_slider_visible.clone(); move |_| v.set(false) })}
                        >
                            <button class="player-controls__btn" onclick={on_volume_toggle} title="Mute (m)">{ volume_icon }</button>
                            <div class={if *volume_slider_visible { "player-volume__slider player-volume__slider--visible" } else { "player-volume__slider" }}>
                                <input type="range" min="0" max="1" step="0.05" value={volume.to_string()} oninput={on_volume_change} class="player-volume__input" />
                            </div>
                        </div>
                        <span class="player-controls__time">{ time_display }</span>
                    </div>
                    <div class="player-controls__right">
                        <div class="player-speed">
                            <button class="player-controls__btn player-controls__btn--text" onclick={on_speed_toggle} title="Playback speed">{ format!("{}x", *playback_speed) }</button>
                            if *speed_menu_open {
                                <div class="player-speed__menu">
                                    { for PLAYBACK_SPEEDS.iter().map(|&speed| {
                                        let on_select = on_speed_select.clone();
                                        let is_active = (*playback_speed - speed).abs() < 0.01;
                                        html! {
                                            <button class={if is_active { "player-speed__option player-speed__option--active" } else { "player-speed__option" }}
                                                onclick={Callback::from(move |e: MouseEvent| { e.stop_propagation(); on_select.emit(speed); })}>
                                                { format!("{}x", speed) }
                                            </button>
                                        }
                                    })}
                                </div>
                            }
                        </div>
                        <div class="player-quality">
                            <button class="player-controls__btn player-controls__btn--text" onclick={on_quality_toggle} title="Stream quality">
                                { QUALITY_OPTIONS.iter().find(|(v, _)| *v == selected_quality.as_str()).map(|(_, l)| *l).unwrap_or("Original (Direct)") }
                            </button>
                            if *quality_menu_open {
                                <div class="player-quality__menu">
                                    { for QUALITY_OPTIONS.iter().map(|(value, label)| {
                                        let on_select = on_quality_select.clone();
                                        let is_active = selected_quality.as_str() == *value;
                                        let vs = value.to_string();
                                        html! {
                                            <button class={if is_active { "player-quality__option player-quality__option--active" } else { "player-quality__option" }}
                                                onclick={Callback::from(move |e: MouseEvent| { e.stop_propagation(); on_select.emit(vs.clone()); })}>
                                                { *label }
                                            </button>
                                        }
                                    })}
                                </div>
                            }
                        </div>
                        if !subtitle_tracks.is_empty() {
                            <div class="player-captions">
                                <button class={if active_subtitle.is_some() { "player-controls__btn player-controls__btn--active" } else { "player-controls__btn" }}
                                    onclick={on_captions_toggle} title="Captions (c)">{ "CC" }</button>
                                if *captions_menu_open {
                                    <div class="player-captions__menu">
                                        <button class={if active_subtitle.is_none() { "player-captions__option player-captions__option--active" } else { "player-captions__option" }}
                                            onclick={Callback::from({ let s = on_caption_select.clone(); move |e: MouseEvent| { e.stop_propagation(); s.emit(None); } })}>{ "Off" }</button>
                                        { for subtitle_tracks.iter().map(|track| {
                                            let on_select = on_caption_select.clone();
                                            let is_active = *active_subtitle == Some(track.index);
                                            let label = track.title.clone().or_else(|| track.language.clone()).unwrap_or_else(|| format!("Track {}", track.index + 1));
                                            let ti = track.index;
                                            html! {
                                                <button class={if is_active { "player-captions__option player-captions__option--active" } else { "player-captions__option" }}
                                                    onclick={Callback::from(move |e: MouseEvent| { e.stop_propagation(); on_select.emit(Some(ti)); })}>{ label }</button>
                                            }
                                        })}
                                    </div>
                                }
                            </div>
                        }
                        <button class="player-controls__btn" onclick={on_fullscreen_toggle} title="Fullscreen (f)">{ fullscreen_icon }</button>
                    </div>
                </div>
            </div>
        </div>
    }
}

// ── API Fetchers ─────────────────────────────────────────────────────────────

async fn fetch_thumbnail_info(video_id: &str) -> Result<ThumbnailInfo, String> {
    let url = format!("/api/videos/{video_id}/thumbnails/info");
    let resp = Request::get(&url).send().await.map_err(|e| format!("{e:?}"))?;
    if !resp.ok() { return Err(format!("HTTP {}", resp.status())); }
    resp.json().await.map_err(|e| format!("{e:?}"))
}

async fn fetch_subtitle_tracks(video_id: &str) -> Result<Vec<SubtitleTrack>, String> {
    let url = format!("/api/videos/{video_id}/subtitles");
    let resp = Request::get(&url).send().await.map_err(|e| format!("{e:?}"))?;
    if !resp.ok() { return Err(format!("HTTP {}", resp.status())); }
    let response: SubtitleTracksResponse = resp.json().await.map_err(|e| format!("{e:?}"))?;
    Ok(response.tracks)
}

async fn clear_video_cache(video_id: &str) {
    let url = format!("/api/videos/{video_id}/cache");
    if let Err(e) = Request::delete(&url).send().await {
        web_sys::console::warn_1(&format!("Failed to clear cache: {e:?}").into());
    }
}

// ── SVG Icons ────────────────────────────────────────────────────────────────

fn icon_play() -> Html {
    html! { <svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 24 24" fill="currentColor" width="1em" height="1em" aria-hidden="true"><path d="M8 5v14l11-7z"/></svg> }
}
fn icon_pause() -> Html {
    html! { <svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 24 24" fill="currentColor" width="1em" height="1em" aria-hidden="true"><path d="M6 19h4V5H6v14zm8-14v14h4V5h-4z"/></svg> }
}
fn icon_replay() -> Html {
    html! { <svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 24 24" fill="currentColor" width="1em" height="1em" aria-hidden="true"><path d="M12 5V1L7 6l5 5V7c3.31 0 6 2.69 6 6s-2.69 6-6 6-6-2.69-6-6H4c0 4.42 3.58 8 8 8s8-3.58 8-8-3.58-8-8-8z"/></svg> }
}
fn icon_volume_muted() -> Html {
    html! { <svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 24 24" fill="currentColor" width="1em" height="1em" aria-hidden="true"><path d="M16.5 12c0-1.77-1.02-3.29-2.5-4.03v2.21l2.45 2.45c.03-.2.05-.41.05-.63zm2.5 0c0 .94-.2 1.82-.54 2.64l1.51 1.51C20.63 14.91 21 13.5 21 12c0-4.28-2.99-7.86-7-8.77v2.06c2.89.86 5 3.54 5 6.71zM4.27 3L3 4.27 7.73 9H3v6h4l5 5v-6.73l4.25 4.25c-.67.52-1.42.93-2.25 1.18v2.06c1.38-.31 2.63-.95 3.69-1.81L19.73 21 21 19.73l-9-9L4.27 3zM12 4L9.91 6.09 12 8.18V4z"/></svg> }
}
fn icon_volume_low() -> Html {
    html! { <svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 24 24" fill="currentColor" width="1em" height="1em" aria-hidden="true"><path d="M18.5 12c0-1.77-1.02-3.29-2.5-4.03v8.05c1.48-.73 2.5-2.25 2.5-4.02zM5 9v6h4l5 5V4L9 9H5z"/></svg> }
}
fn icon_volume_high() -> Html {
    html! { <svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 24 24" fill="currentColor" width="1em" height="1em" aria-hidden="true"><path d="M3 9v6h4l5 5V4L7 9H3zm13.5 3c0-1.77-1.02-3.29-2.5-4.03v8.05c1.48-.73 2.5-2.25 2.5-4.02zM14 3.23v2.06c2.89.86 5 3.54 5 6.71s-2.11 5.85-5 6.71v2.06c4.01-.91 7-4.49 7-8.77s-2.99-7.86-7-8.77z"/></svg> }
}
fn icon_fullscreen_enter() -> Html {
    html! { <svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 24 24" fill="currentColor" width="1em" height="1em" aria-hidden="true"><path d="M7 14H5v5h5v-2H7v-3zm-2-4h2V7h3V5H5v5zm12 7h-3v2h5v-5h-2v3zM14 5v2h3v3h2V5h-5z"/></svg> }
}
fn icon_fullscreen_exit() -> Html {
    html! { <svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 24 24" fill="currentColor" width="1em" height="1em" aria-hidden="true"><path d="M5 16h3v3h2v-5H5v2zm3-8H5v2h5V5H8v3zm6 11h2v-3h3v-2h-5v5zm2-11V5h-2v5h5V8h-3z"/></svg> }
}
fn icon_arrow_back() -> Html {
    html! { <svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 24 24" fill="currentColor" width="1em" height="1em" aria-hidden="true"><path d="M20 11H7.83l5.59-5.59L12 4l-8 8 8 8 1.41-1.41L7.83 13H20v-2z"/></svg> }
}
fn icon_skip_forward() -> Html {
    html! { <svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 24 24" fill="currentColor" width="1em" height="1em" aria-hidden="true"><path d="M4 18l8.5-6L4 6v12zm9-12v12l8.5-6L13 6z"/></svg> }
}
fn icon_skip_backward() -> Html {
    html! { <svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 24 24" fill="currentColor" width="1em" height="1em" aria-hidden="true"><path d="M11 18V6l-8.5 6 8.5 6zm.5-6l8.5 6V6l-8.5 6z"/></svg> }
}
