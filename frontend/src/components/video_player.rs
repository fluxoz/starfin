use futures::channel::mpsc;
use futures::StreamExt;
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

// ── Seek-anchor constants ────────────────────────────────────────────────────
// These must stay in sync with SEGMENT_DURATION, PRECACHE_SEGMENTS, and
// SPARSE_CACHE_STRIDE in `src/main.rs`.
const SEGMENT_DURATION_F: f64 = 6.0;
const PRECACHE_SEGMENTS_F: f64 = 20.0;
const SPARSE_CACHE_STRIDE_F: f64 = 3.0;

/// Snaps a seek time to the **start** of the cached segment at or before `time`.
///
/// Within the dense pre-cache window every segment is cached, so we just
/// snap to the segment boundary.  Beyond that window only every
/// `SPARSE_CACHE_STRIDE_F`-th segment is cached, so we round down to the
/// nearest anchor start.  The same logic applies whether the user is
/// clicking forward or backward.
fn snap_to_cached_segment(time: f64) -> f64 {
    if time <= 0.0 {
        return 0.0;
    }
    let dense_window = PRECACHE_SEGMENTS_F * SEGMENT_DURATION_F; // 120 seconds
    if time < dense_window {
        // Within the dense window — every segment is cached; snap to segment start.
        let seg_index = (time / SEGMENT_DURATION_F) as usize;
        return seg_index as f64 * SEGMENT_DURATION_F;
    }
    // Beyond the dense window — snap down to the nearest sparse anchor start.
    let stride = SPARSE_CACHE_STRIDE_F as usize;
    let seg_index = (time / SEGMENT_DURATION_F) as usize;
    let anchor_index = (seg_index / stride) * stride;
    anchor_index as f64 * SEGMENT_DURATION_F
}

// ── Stream quality options ────────────────────────────────────────────────────
/// (url-token, display-label) pairs for the quality selector.
/// "Original" uses direct remux (no re-encoding) when the source codecs are
/// browser-compatible (H.264 + AAC/MP3), giving VLC-like performance.
/// Falls back to high-quality transcode for incompatible sources.
const QUALITY_OPTIONS: [(&str, &str); 4] = [
    ("original", "Original (Direct)"),
    ("high",     "High (Transcode)"),
    ("medium",   "Medium (720p)"),
    ("low",      "Low (480p)"),
];
/// localStorage key used to persist the selected quality across sessions.
const QUALITY_STORAGE_KEY: &str = "starfin_quality";

// ── Controls auto-hide timeout (milliseconds of inactivity) ─────────────────
const CONTROL_HIDE_TIMEOUT_MS: f64 = 5000.0;
/// Pixel distance from the top or bottom edge of the player within which the
/// controls/header are considered "near" and should not be hidden.
const CONTROLS_VICINITY_PX: f64 = 80.0;

// ── MSE player constants ─────────────────────────────────────────────────────
/// Target seconds of video to keep buffered ahead of the playback position.
const MSE_TARGET_BUFFER_S: f64 = 30.0;
/// Seconds of back-buffer to retain behind the playback position when seeking.
const MSE_BACK_BUFFER_S: f64 = 5.0;

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

/// Calculate seek time and position from a mouse event on the progress bar.
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

// ── MSE player types ─────────────────────────────────────────────────────────

struct SegmentInfo {
    url: String,
    #[allow(dead_code)]
    duration: f64,
}

enum PumpMsg {
    /// SourceBuffer `updateend` event fired.
    AppendComplete,
    /// Top up buffer (from 150ms interval or initial kick).
    TopUp,
    /// Seek to unbuffered position — reset segment pointer.
    Seek(f64),
    /// Fetch completed successfully.
    FetchComplete(u32, Vec<u8>),
    /// Fetch failed after retries (carries generation for staleness check).
    FetchFailed(u32),
    /// Shutdown the pump loop.
    Shutdown,
}

#[derive(Clone, Copy, PartialEq)]
enum PumpState {
    Idle,
    Fetching,
    Appending,
}

struct MseState {
    media_source: web_sys::MediaSource,
    source_buffer: web_sys::SourceBuffer,
    /// Blob URL created for this MediaSource; revoked on cleanup.
    object_url: String,
    /// Parsed segment list from the M3U8 playlist.
    segments: Vec<SegmentInfo>,
    /// Index of the next segment to fetch.
    next_seg: usize,
    /// Monotonically increasing counter incremented on every seek to
    /// unbuffered territory.  Stale callbacks compare their captured
    /// generation against the current value (§11.6).
    generation: u32,
    /// Channel sender for the pump loop.
    pump_tx: mpsc::UnboundedSender<PumpMsg>,
    /// Persistent `updateend` closure — stored here so it is properly
    /// dropped on cleanup instead of being `.forget()`-ed (§11.2).
    _updateend_closure: Closure<dyn Fn()>,
    /// Persistent `error` closure on the SourceBuffer for diagnostics.
    _onerror_closure: Closure<dyn Fn()>,
}

// ── URL / playlist helpers ───────────────────────────────────────────────────

/// Resolve a relative segment path against the playlist URL.
///
/// Strips the filename from `playlist_url`, keeps the query string, and
/// prepends the base to `segment_path`.  Example:
///   playlist:  /api/videos/42/playlist.m3u8?quality=original
///   segment:   seg_00000.ts
///   →          /api/videos/42/seg_00000.ts?quality=original
fn resolve_segment_url(playlist_url: &str, segment_path: &str) -> String {
    if segment_path.starts_with("http://")
        || segment_path.starts_with("https://")
        || segment_path.starts_with('/')
    {
        return segment_path.to_string();
    }
    let (path_part, query_part) = if let Some(q) = playlist_url.find('?') {
        (&playlist_url[..q], &playlist_url[q..])
    } else {
        (playlist_url, "")
    };
    let base = if let Some(slash) = path_part.rfind('/') {
        &path_part[..=slash]
    } else {
        "/"
    };
    format!("{base}{segment_path}{query_part}")
}

/// Parse an HLS M3U8 playlist and return the list of segments.
///
/// Only handles `#EXTINF` + segment URL pairs (the format the server emits).
/// Relative segment URLs are resolved against `playlist_url`.
fn parse_m3u8(text: &str, playlist_url: &str) -> Vec<SegmentInfo> {
    let mut segments = Vec::new();
    let mut pending_duration: Option<f64> = None;
    for line in text.lines() {
        let line = line.trim();
        if let Some(rest) = line.strip_prefix("#EXTINF:") {
            let duration = rest
                .split(',')
                .next()
                .and_then(|d| d.parse::<f64>().ok())
                .unwrap_or(0.0);
            pending_duration = Some(duration);
        } else if !line.starts_with('#') && !line.is_empty() {
            if let Some(duration) = pending_duration.take() {
                segments.push(SegmentInfo {
                    url: resolve_segment_url(playlist_url, line),
                    duration,
                });
            }
        }
    }
    segments
}

// ── Channel-based MSE pump (§11.2) ──────────────────────────────────────────

/// Maximum number of retry attempts for a failed segment fetch (§11.7).
const SEGMENT_FETCH_MAX_RETRIES: u32 = 3;
/// Initial retry delay in milliseconds; doubled on each subsequent attempt.
const SEGMENT_FETCH_RETRY_BASE_MS: u32 = 500;

/// The main pump loop — a single async task driven by channel messages.
async fn pump_loop(
    mut rx: mpsc::UnboundedReceiver<PumpMsg>,
    state: Rc<RefCell<Option<MseState>>>,
    video: HtmlVideoElement,
) {
    let mut pump_state = PumpState::Idle;

    while let Some(msg) = rx.next().await {
        match msg {
            PumpMsg::Shutdown => break,

            PumpMsg::Seek(time) => {
                handle_pump_seek(&state, time);
                pump_state = PumpState::Idle;
            }

            PumpMsg::AppendComplete => {
                if pump_state == PumpState::Appending {
                    if let Some(s) = state.borrow_mut().as_mut() {
                        s.next_seg += 1;
                    }
                    pump_state = PumpState::Idle;
                }
                // Ignore spurious updateend (e.g. from remove())
            }

            PumpMsg::FetchComplete(pump_gen, bytes) => {
                if pump_state != PumpState::Fetching || !check_generation(&state, pump_gen) {
                    pump_state = PumpState::Idle;
                } else if try_append_bytes(&state, &bytes) {
                    pump_state = PumpState::Appending;
                    continue; // Wait for AppendComplete, don't try to start fetch
                } else {
                    pump_state = PumpState::Idle;
                }
            }

            PumpMsg::FetchFailed(pump_gen) => {
                // Only reset to idle if this failure is for the current generation;
                // a stale failure should not disturb an in-progress fetch.
                if pump_state == PumpState::Fetching && check_generation(&state, pump_gen) {
                    pump_state = PumpState::Idle;
                }
            }

            PumpMsg::TopUp => {}
        }

        // If idle, try to start a new fetch
        if pump_state == PumpState::Idle {
            if try_start_fetch(&state, &video) {
                pump_state = PumpState::Fetching;
            }
        }
    }
}

/// Handle a seek to unbuffered territory: increment generation, abort any
/// in-progress append, set `timestampOffset` for the new position, trim
/// back-buffer, and reset `next_seg`.
///
/// In "sequence" mode, calling `abort()` resets the SourceBuffer's append
/// state to WAITING_FOR_SEGMENT, allowing us to set `timestampOffset` to
/// redirect where the next append will be placed on the MSE timeline.
fn handle_pump_seek(
    state: &Rc<RefCell<Option<MseState>>>,
    time: f64,
) {
    let mut borrow = state.borrow_mut();
    if let Some(mse) = borrow.as_mut() {
        mse.generation = mse.generation.wrapping_add(1);

        // abort() resets the parser state so we can set timestampOffset.
        // Only valid when MediaSource readyState is "open".
        if mse.media_source.ready_state() == web_sys::MediaSourceReadyState::Open {
            let _ = mse.source_buffer.abort();
        }

        let snapped = snap_to_cached_segment(time);

        // Set timestampOffset so the next append is placed at the correct
        // position on the MSE timeline (critical for Firefox).
        if !mse.source_buffer.updating() {
            mse.source_buffer.set_timestamp_offset(snapped);
        }

        // Trim stale back-buffer.
        if !mse.source_buffer.updating() {
            let remove_end = (snapped - MSE_BACK_BUFFER_S).max(0.0);
            if remove_end > 0.0 {
                let _ = mse.source_buffer.remove(0.0, remove_end);
            }
        }

        mse.next_seg = (snapped / SEGMENT_DURATION_F) as usize;
    }
}

/// If the pump is idle and the buffer is below the target, spawn an async
/// fetch task for the next segment.  Returns `true` when a fetch was started.
fn try_start_fetch(
    state: &Rc<RefCell<Option<MseState>>>,
    video: &HtmlVideoElement,
) -> bool {
    let (seg_url, pump_gen, tx) = {
        let borrow = state.borrow();
        let mse = match borrow.as_ref() {
            Some(s) => s,
            None => return false,
        };
        if mse.source_buffer.updating() {
            return false;
        }
        let buffered_ahead = get_buffer_end(video) - video.current_time();
        if buffered_ahead >= MSE_TARGET_BUFFER_S && mse.next_seg != 0 {
            return false;
        }
        if mse.next_seg >= mse.segments.len() {
            // Only signal end-of-stream when the MediaSource is "open";
            // Firefox throws InvalidStateError otherwise.
            if mse.media_source.ready_state() == web_sys::MediaSourceReadyState::Open
                && !mse.source_buffer.updating()
            {
                let _ = mse.media_source.end_of_stream();
            }
            return false;
        }
        let url = mse.segments[mse.next_seg].url.clone();
        (url, mse.generation, mse.pump_tx.clone())
    };

    spawn_local(async move {
        fetch_segment_with_retry(seg_url, pump_gen, tx).await;
    });

    true
}

/// Fetch a segment with exponential-backoff retry (§11.7) and report the
/// result back through the channel.
async fn fetch_segment_with_retry(
    url: String,
    pump_gen: u32,
    tx: mpsc::UnboundedSender<PumpMsg>,
) {
    let mut bytes_opt: Option<Vec<u8>> = None;
    for attempt in 0..=SEGMENT_FETCH_MAX_RETRIES {
        match Request::get(&url).send().await {
            Ok(r) => match r.binary().await {
                Ok(b) => {
                    bytes_opt = Some(b);
                    break;
                }
                Err(e) => {
                    log::warn!("Segment read error (attempt {}): {e:?}", attempt + 1);
                }
            },
            Err(e) => {
                log::warn!("Segment fetch error (attempt {}): {e:?}", attempt + 1);
            }
        }
        if attempt < SEGMENT_FETCH_MAX_RETRIES {
            let delay = SEGMENT_FETCH_RETRY_BASE_MS * (1 << attempt);
            TimeoutFuture::new(delay).await;
        }
    }

    match bytes_opt {
        Some(bytes) => {
            let _ = tx.unbounded_send(PumpMsg::FetchComplete(pump_gen, bytes));
        }
        None => {
            log::error!(
                "Failed to fetch segment after {} retries: {url}",
                SEGMENT_FETCH_MAX_RETRIES
            );
            let _ = tx.unbounded_send(PumpMsg::FetchFailed(pump_gen));
        }
    }
}

/// Try to append raw bytes to the SourceBuffer.  Returns `true` on success.
///
/// Defensively checks `updating()` — Firefox throws if an append is
/// attempted while the source buffer is still processing a previous
/// `appendBuffer` or `remove` call.
fn try_append_bytes(state: &Rc<RefCell<Option<MseState>>>, bytes: &[u8]) -> bool {
    let borrow = state.borrow();
    let mse = match borrow.as_ref() {
        Some(s) => s,
        None => return false,
    };
    if mse.source_buffer.updating() {
        return false;
    }
    let uint8_array = js_sys::Uint8Array::from(bytes);
    let array_buffer = uint8_array.buffer();
    mse.source_buffer
        .append_buffer_with_array_buffer(&array_buffer)
        .is_ok()
}

/// Compare a captured generation against the current MseState generation.
fn check_generation(state: &Rc<RefCell<Option<MseState>>>, pump_gen: u32) -> bool {
    state
        .borrow()
        .as_ref()
        .map_or(false, |s| s.generation == pump_gen)
}

// ── ProgressBar component (§11.1) ────────────────────────────────────────────

#[derive(Properties, PartialEq)]
struct ProgressBarProps {
    pub progress_ref: NodeRef,
    pub thumbnail_canvas_ref: NodeRef,
    pub duration: f64,
    pub current_time: f64,
    pub buffered_end: f64,
    pub is_dragging: bool,
    pub is_hovering_progress: bool,
    pub hover_time: f64,
    pub hover_position: f64,
    pub drag_time: f64,
    pub thumbnail_info: Option<ThumbnailInfo>,
    pub on_click: Callback<MouseEvent>,
    pub on_mousedown: Callback<MouseEvent>,
    pub on_mousemove: Callback<MouseEvent>,
    pub on_mouseleave: Callback<MouseEvent>,
}

#[function_component]
fn ProgressBar(props: &ProgressBarProps) -> Html {
    let progress_percent = if props.duration > 0.0 {
        (props.current_time / props.duration * 100.0).min(100.0)
    } else {
        0.0
    };
    let buffered_percent = if props.duration > 0.0 {
        (props.buffered_end / props.duration * 100.0).min(100.0)
    } else {
        0.0
    };

    let preview_style = if props.is_hovering_progress || props.is_dragging {
        let left = props.hover_position.clamp(5.0, 95.0);
        format!("left: {}%; display: block;", left)
    } else {
        "display: none;".to_string()
    };

    let preview_time = if props.is_dragging {
        props.drag_time
    } else {
        props.hover_time
    };

    html! {
        <div class="player-progress-container">
            // Thumbnail preview
            <div class="player-preview" style={preview_style}>
                <canvas
                    ref={props.thumbnail_canvas_ref.clone()}
                    class="player-preview__canvas"
                    width="160"
                    height="90"
                />
                <div class="player-preview__time">{ format_time(preview_time) }</div>
            </div>

            // Progress bar
            <div
                ref={props.progress_ref.clone()}
                class="player-progress"
                onclick={props.on_click.clone()}
                onmousedown={props.on_mousedown.clone()}
                onmousemove={props.on_mousemove.clone()}
                onmouseleave={props.on_mouseleave.clone()}
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
                if props.is_hovering_progress || props.is_dragging {
                    <div
                        class="player-progress__hover-line"
                        style={format!("left: {}%", if props.is_dragging { progress_percent } else { props.hover_position })}
                    />
                }
                <div
                    class={if props.is_dragging { "player-progress__thumb player-progress__thumb--dragging" } else { "player-progress__thumb" }}
                    style={format!("left: {}%", progress_percent)}
                />
            </div>
        </div>
    }
}

// ── ControlBar component (§11.1) ─────────────────────────────────────────────

#[derive(Properties, PartialEq)]
struct ControlBarProps {
    // Layout
    pub visible: bool,
    // Progress bar (forwarded)
    pub progress_ref: NodeRef,
    pub thumbnail_canvas_ref: NodeRef,
    pub duration: f64,
    pub current_time: f64,
    pub buffered_end: f64,
    pub is_dragging: bool,
    pub is_hovering_progress: bool,
    pub hover_time: f64,
    pub hover_position: f64,
    pub drag_time: f64,
    pub thumbnail_info: Option<ThumbnailInfo>,
    pub on_progress_click: Callback<MouseEvent>,
    pub on_progress_mousedown: Callback<MouseEvent>,
    pub on_progress_hover: Callback<MouseEvent>,
    pub on_progress_leave: Callback<MouseEvent>,
    // Playback
    pub is_playing: bool,
    pub video_ended: bool,
    pub playback_speed: f64,
    pub speed_menu_open: bool,
    pub on_play_pause: Callback<()>,
    pub on_speed_toggle: Callback<MouseEvent>,
    pub on_speed_select: Callback<f64>,
    // Volume
    pub volume: f64,
    pub is_muted: bool,
    pub volume_slider_visible: bool,
    pub on_volume_toggle: Callback<()>,
    pub on_volume_change: Callback<web_sys::InputEvent>,
    pub on_volume_enter: Callback<()>,
    pub on_volume_leave: Callback<()>,
    // Quality
    pub selected_quality: String,
    pub quality_menu_open: bool,
    pub on_quality_toggle: Callback<MouseEvent>,
    pub on_quality_select: Callback<String>,
    // Captions
    pub subtitle_tracks: Vec<SubtitleTrack>,
    pub active_subtitle: Option<u32>,
    pub captions_menu_open: bool,
    pub on_captions_toggle: Callback<MouseEvent>,
    pub on_caption_select: Callback<Option<u32>>,
    // Fullscreen
    pub is_fullscreen: bool,
    pub on_fullscreen_toggle: Callback<()>,
}

#[function_component]
fn ControlBar(props: &ControlBarProps) -> Html {
    let controls_class = if props.visible {
        "player-controls"
    } else {
        "player-controls player-controls--hidden"
    };

    let play_pause_icon: Html = if props.video_ended {
        icon_replay()
    } else if props.is_playing {
        icon_pause()
    } else {
        icon_play()
    };

    let volume_icon: Html = if props.is_muted || props.volume == 0.0 {
        icon_volume_muted()
    } else if props.volume < 0.5 {
        icon_volume_low()
    } else {
        icon_volume_high()
    };

    let fullscreen_icon: Html = if props.is_fullscreen {
        icon_fullscreen_exit()
    } else {
        icon_fullscreen_enter()
    };

    let time_display = format!(
        "{} / {}",
        format_time(props.current_time),
        format_time(props.duration)
    );

    let on_play_pause = props.on_play_pause.clone();
    let on_volume_toggle = props.on_volume_toggle.clone();
    let on_fullscreen_toggle = props.on_fullscreen_toggle.clone();

    html! {
        <div class={controls_class}>
            <ProgressBar
                progress_ref={props.progress_ref.clone()}
                thumbnail_canvas_ref={props.thumbnail_canvas_ref.clone()}
                duration={props.duration}
                current_time={props.current_time}
                buffered_end={props.buffered_end}
                is_dragging={props.is_dragging}
                is_hovering_progress={props.is_hovering_progress}
                hover_time={props.hover_time}
                hover_position={props.hover_position}
                drag_time={props.drag_time}
                thumbnail_info={props.thumbnail_info.clone()}
                on_click={props.on_progress_click.clone()}
                on_mousedown={props.on_progress_mousedown.clone()}
                on_mousemove={props.on_progress_hover.clone()}
                on_mouseleave={props.on_progress_leave.clone()}
            />

            // Bottom controls
            <div class="player-controls__bottom">
                // Left side controls
                <div class="player-controls__left">
                    <button
                        class="player-controls__btn"
                        onclick={Callback::from(move |_| on_play_pause.emit(()))}
                        title="Play/Pause (k)"
                    >
                        { play_pause_icon }
                    </button>

                    // Volume control
                    <div
                        class="player-volume"
                        onmouseenter={Callback::from({
                            let on_enter = props.on_volume_enter.clone();
                            move |_: MouseEvent| on_enter.emit(())
                        })}
                        onmouseleave={Callback::from({
                            let on_leave = props.on_volume_leave.clone();
                            move |_: MouseEvent| on_leave.emit(())
                        })}
                    >
                        <button
                            class="player-controls__btn"
                            onclick={Callback::from(move |_| on_volume_toggle.emit(()))}
                            title="Mute (m)"
                        >
                            { volume_icon }
                        </button>
                        <div class={if props.volume_slider_visible { "player-volume__slider player-volume__slider--visible" } else { "player-volume__slider" }}>
                            <input
                                type="range"
                                min="0"
                                max="1"
                                step="0.05"
                                value={props.volume.to_string()}
                                oninput={props.on_volume_change.clone()}
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
                            onclick={props.on_speed_toggle.clone()}
                            title="Playback speed"
                        >
                            { format!("{}x", props.playback_speed) }
                        </button>
                        if props.speed_menu_open {
                            <div class="player-speed__menu">
                                { for PLAYBACK_SPEEDS.iter().map(|&speed| {
                                    let on_select = props.on_speed_select.clone();
                                    let is_active = (props.playback_speed - speed).abs() < 0.01;
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
                            onclick={props.on_quality_toggle.clone()}
                            title="Stream quality"
                        >
                            { QUALITY_OPTIONS.iter()
                                .find(|(v, _)| *v == props.selected_quality.as_str())
                                .map(|(_, label)| *label)
                                .unwrap_or("Original (Direct)") }
                        </button>
                        if props.quality_menu_open {
                            <div class="player-quality__menu">
                                { for QUALITY_OPTIONS.iter().map(|(value, label)| {
                                    let on_select = props.on_quality_select.clone();
                                    let is_active = props.selected_quality.as_str() == *value;
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
                    if !props.subtitle_tracks.is_empty() {
                        <div class="player-captions">
                            <button
                                class={if props.active_subtitle.is_some() { "player-controls__btn player-controls__btn--active" } else { "player-controls__btn" }}
                                onclick={props.on_captions_toggle.clone()}
                                title="Captions (c)"
                            >
                                { "CC" }
                            </button>
                            if props.captions_menu_open {
                                <div class="player-captions__menu">
                                    <button
                                        class={if props.active_subtitle.is_none() { "player-captions__option player-captions__option--active" } else { "player-captions__option" }}
                                        onclick={Callback::from({
                                            let on_select = props.on_caption_select.clone();
                                            move |e: MouseEvent| {
                                                e.stop_propagation();
                                                on_select.emit(None);
                                            }
                                        })}
                                    >
                                        { "Off" }
                                    </button>
                                    { for props.subtitle_tracks.iter().map(|track| {
                                        let on_select = props.on_caption_select.clone();
                                        let is_active = props.active_subtitle == Some(track.index);
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

                    // Fullscreen button
                    <button
                        class="player-controls__btn"
                        onclick={Callback::from(move |_| on_fullscreen_toggle.emit(()))}
                        title="Fullscreen (f)"
                    >
                        { fullscreen_icon }
                    </button>
                </div>
            </div>
        </div>
    }
}

// ── VideoPlayer component (§11.1 shell) ──────────────────────────────────────

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

    // Stream quality — initialised from localStorage so the preference
    // persists across sessions.  Defaults to "original" (direct remux)
    // which gives VLC-like performance for compatible sources.
    let initial_quality = window()
        .and_then(|w| w.local_storage().ok())
        .flatten()
        .and_then(|s| s.get_item(QUALITY_STORAGE_KEY).ok())
        .flatten()
        .filter(|q| QUALITY_OPTIONS.iter().any(|(v, _)| v == q))
        .unwrap_or_else(|| "original".to_string());
    let selected_quality = use_state(|| initial_quality);

    // Stores the video position to resume at when quality is changed
    // mid-playback.  Updated by `on_quality_select` before triggering a
    // re-initialisation of HLS.
    let resume_position = use_mut_ref(|| 0.0_f64);

    // Thumbnail sprite info
    let thumbnail_info = use_state(|| Option::<ThumbnailInfo>::None);

    // Double-tap tracking for mobile (§11.5 — Option<f64> instead of sentinel 0.0)
    let last_tap_time = use_state(|| Option::<f64>::None);
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

    // MSE player state storage.
    //
    // We use `use_mut_ref` (Rc<RefCell<…>>) rather than `use_state` here
    // because the MSE state is set *asynchronously* inside a `spawn_local`
    // block.  In Yew 0.21, `use_state` clones a new `Rc<R>` on every state
    // update, so a handle captured in a cleanup closure before the async
    // write completes will always read the *initial* `None` value — meaning
    // cleanup is never called correctly.
    //
    // `use_mut_ref` wraps a single `Rc<RefCell<T>>` that is shared by all
    // clones, so any write made by the async task is immediately visible to
    // the cleanup closure captured earlier.
    let mse_state = use_mut_ref(|| Option::<MseState>::None);

    // Initialize MSE player (and re-initialise when video ID or quality changes).
    {
        let video_ref = video_ref.clone();
        let status = status.clone();
        let error = error.clone();
        let thumbnail_info = thumbnail_info.clone();
        let thumbnail_image = thumbnail_image.clone();
        let subtitle_tracks = subtitle_tracks.clone();
        let mse_state = mse_state.clone();
        let selected_quality = selected_quality.clone();
        let resume_position = resume_position.clone();

        use_effect_with(
            (props.video_id.clone(), (*selected_quality).clone()),
            move |(video_id, quality)| {
            let video_id = video_id.clone();
            let quality = quality.clone();

            // ── Cancellation flag ────────────────────────────────────────
            //
            // Shared between the async setup task and the cleanup closure.
            // When the effect re-fires (or the component unmounts), the
            // cleanup sets this to `true` so that any in-flight async work
            // (e.g. the 50 ms wait, playlist fetch, sourceopen callback)
            // bails out instead of touching stale handles or creating a
            // second MediaSource.
            let cancelled = Rc::new(Cell::new(false));
            let cancelled_for_cleanup = cancelled.clone();

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

            // Initialize MSE player
            let video_ref_clone = video_ref.clone();
            let status_clone = status.clone();
            let error_clone = error.clone();
            let mse_state_clone = mse_state.clone();
            let cancelled_for_setup = cancelled.clone();

            spawn_local(async move {
                // Give time for video element to be created
                TimeoutFuture::new(50).await;

                // Bail out if the effect has already been cleaned up (e.g.
                // a very fast quality switch or unmount during the 50 ms wait).
                if cancelled_for_setup.get() {
                    return;
                }

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

                // Create a MediaSource (also verifies MSE is available in this browser).
                let media_source = match web_sys::MediaSource::new() {
                    Ok(ms) => ms,
                    Err(_) => {
                        error_clone.set(Some(
                            "Your browser does not support Media Source Extensions.".to_string(),
                        ));
                        return;
                    }
                };

                // Attach the MediaSource to the video element via a blob URL.
                let object_url =
                    match web_sys::Url::create_object_url_with_source(&media_source) {
                        Ok(u) => u,
                        Err(_) => {
                            error_clone.set(Some(
                                "Failed to create MediaSource URL.".to_string(),
                            ));
                            return;
                        }
                    };
                video.set_src(&object_url);
                status_clone.set("Loading stream…".to_string());

                // All SourceBuffer setup must happen inside the sourceopen callback.
                let playlist_url_for_open = playlist_url.clone();
                let video_for_open = video.clone();
                let status_for_open = status_clone.clone();
                let error_for_open = error_clone.clone();
                let mse_state_for_open = mse_state_clone.clone();
                let media_source_for_open = media_source.clone();
                let object_url_for_open = object_url.clone();
                let cancelled_for_open = cancelled_for_setup.clone();

                let sourceopen_cb = Closure::once(Box::new(move || {
                    let playlist_url = playlist_url_for_open;
                    let video = video_for_open;
                    let status = status_for_open;
                    let error = error_for_open;
                    let mse_state = mse_state_for_open;
                    let media_source = media_source_for_open;
                    let object_url = object_url_for_open;
                    let cancelled = cancelled_for_open;

                    spawn_local(async move {
                        // Guard: bail out if the effect has been cleaned up.
                        if cancelled.get() {
                            return;
                        }

                        // Fetch the M3U8 playlist.
                        let resp = match Request::get(&playlist_url).send().await {
                            Ok(r) => r,
                            Err(e) => {
                                error.set(Some(format!("Failed to fetch playlist: {e:?}")));
                                return;
                            }
                        };
                        let text = match resp.text().await {
                            Ok(t) => t,
                            Err(e) => {
                                error.set(Some(format!("Failed to read playlist: {e:?}")));
                                return;
                            }
                        };

                        // Parse segment list.
                        let segments = parse_m3u8(&text, &playlist_url);
                        if segments.is_empty() {
                            error.set(Some("Playlist contains no segments.".to_string()));
                            return;
                        }

                        // Guard: check cancellation again after the network round-trip.
                        if cancelled.get() {
                            return;
                        }

                        // The server produces fragmented MP4 (fMP4) segments which are
                        // supported by MSE in all major browsers (Chrome, Firefox, Safari
                        // uses native HLS above). The codec string covers all three server
                        // paths (remux, hybrid, transcode): H.264 (avc1.42E01E) + AAC-LC
                        // (mp4a.40.2).
                        let mime = "video/mp4; codecs=\"avc1.42E01E,mp4a.40.2\"".to_string();

                        // Create the SourceBuffer.
                        let source_buffer = match media_source.add_source_buffer(&mime) {
                            Ok(sb) => sb,
                            Err(e) => {
                                error.set(Some(format!(
                                    "Unsupported stream format. Try a different quality level. ({e:?})"
                                )));
                                return;
                            }
                        };

                        // Use "sequence" mode so the MSE pipeline places each appended
                        // segment after the previous one automatically.  This is critical
                        // for Firefox which strictly follows the MSE spec: without it,
                        // each fMP4 segment (whose PTS is rebased to 0 by the backend)
                        // would overwrite the 0-based timeline range and playback would
                        // stutter/reset every segment.
                        //
                        // In sequence mode, we control placement via timestampOffset:
                        //   • Initial load: timestampOffset = start_seg * SEGMENT_DURATION_F
                        //   • After seek:   abort() + timestampOffset = snapped_time
                        //   • Sequential:   MSE auto-advances from previous segment end
                        source_buffer.set_mode(web_sys::SourceBufferAppendMode::Sequence);

                        // Calculate which segment to start from when resuming.
                        let start_seg = if start_pos > 0.0 {
                            let snapped = snap_to_cached_segment(start_pos);
                            (snapped / SEGMENT_DURATION_F) as usize
                        } else {
                            0
                        };

                        // Set initial timestampOffset for resume position.
                        if start_seg > 0 {
                            source_buffer
                                .set_timestamp_offset(start_seg as f64 * SEGMENT_DURATION_F);
                        }

                        // §11.2: Create channel for the pump loop.
                        let (pump_tx, pump_rx) = mpsc::unbounded();

                        // Create a single persistent updateend closure that sends
                        // through the channel instead of being forgotten.
                        let updateend_tx = pump_tx.clone();
                        let updateend_closure = Closure::<dyn Fn()>::new(move || {
                            let _ = updateend_tx.unbounded_send(PumpMsg::AppendComplete);
                        });
                        source_buffer
                            .add_event_listener_with_callback(
                                "updateend",
                                updateend_closure.as_ref().unchecked_ref(),
                            )
                            .ok();

                        // Log SourceBuffer errors (helps diagnose Firefox-specific issues).
                        let onerror_closure = Closure::<dyn Fn()>::new(move || {
                            log::error!("SourceBuffer error event fired");
                        });
                        source_buffer
                            .add_event_listener_with_callback(
                                "error",
                                onerror_closure.as_ref().unchecked_ref(),
                            )
                            .ok();

                        // Store MSE state.
                        *mse_state.borrow_mut() = Some(MseState {
                            media_source,
                            source_buffer,
                            object_url,
                            segments,
                            next_seg: start_seg,
                            generation: 0,
                            pump_tx: pump_tx.clone(),
                            _updateend_closure: updateend_closure,
                            _onerror_closure: onerror_closure,
                        });

                        status.set(String::new());
                        if start_pos > 0.0 {
                            video.set_current_time(snap_to_cached_segment(start_pos));
                        }

                        // Start the pump loop as a spawned task.
                        let mse_state_for_pump = mse_state.clone();
                        let video_for_pump = video.clone();
                        spawn_local(pump_loop(pump_rx, mse_state_for_pump, video_for_pump));

                        // Kick off the initial segment fetch.
                        let _ = pump_tx.unbounded_send(PumpMsg::TopUp);
                    });
                }) as Box<dyn FnOnce()>);

                media_source
                    .add_event_listener_with_callback(
                        "sourceopen",
                        sourceopen_cb.as_ref().unchecked_ref(),
                    )
                    .ok();
                sourceopen_cb.forget();
            });

            // Cleanup function: called by Yew when the dep tuple changes (quality
            // or video ID changes) or when the component unmounts.
            let mse_state_for_cleanup = mse_state.clone();
            let video_ref_for_cleanup = video_ref.clone();
            move || {
                // Signal all in-flight async tasks to bail out.
                cancelled_for_cleanup.set(true);

                // §11.2: Send Shutdown before dropping MseState.
                {
                    let borrow = mse_state_for_cleanup.borrow();
                    if let Some(mse) = borrow.as_ref() {
                        let _ = mse.pump_tx.unbounded_send(PumpMsg::Shutdown);
                    }
                }
                if let Some(state) = mse_state_for_cleanup.borrow_mut().take() {
                    // Remove persistent event listeners.
                    let _ = state.source_buffer.remove_event_listener_with_callback(
                        "updateend",
                        state._updateend_closure.as_ref().unchecked_ref(),
                    );
                    let _ = state.source_buffer.remove_event_listener_with_callback(
                        "error",
                        state._onerror_closure.as_ref().unchecked_ref(),
                    );
                    // Firefox requires readyState == "open" before end_of_stream().
                    if state.media_source.ready_state() == web_sys::MediaSourceReadyState::Open
                        && !state.source_buffer.updating()
                    {
                        let _ = state.media_source.end_of_stream();
                    }
                    let _ = web_sys::Url::revoke_object_url(&state.object_url);
                    if let Some(video) = video_ref_for_cleanup.cast::<HtmlVideoElement>() {
                        video.set_src("");
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

    // Update time/duration periodically and send TopUp to pump loop
    {
        let video_ref = video_ref.clone();
        let current_time = current_time.clone();
        let duration = duration.clone();
        let buffered_end = buffered_end.clone();
        let is_playing = is_playing.clone();
        let is_dragging = is_dragging.clone();
        let is_buffering = is_buffering.clone();
        let video_ended = video_ended.clone();
        let mse_state = mse_state.clone();

        use_effect_with(video_ref.clone(), move |video_ref| {
            let video_ref = video_ref.clone();
            let mse_state_for_interval = mse_state.clone();
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

                    // Send TopUp to the pump loop
                    let tx = {
                        let borrow = mse_state_for_interval.borrow();
                        borrow.as_ref().map(|s| s.pump_tx.clone())
                    };
                    if let Some(tx) = tx {
                        let _ = tx.unbounded_send(PumpMsg::TopUp);
                    }
                }
            });
            move || drop(interval)
        });
    }

    // Handle seeks to unbuffered positions for the MSE player — sends
    // PumpMsg::Seek through the channel instead of manipulating state
    // directly (§11.2).
    {
        let video_ref = video_ref.clone();
        let mse_state = mse_state.clone();

        use_effect_with(video_ref.clone(), move |video_ref| {
            let video_opt = video_ref.cast::<HtmlVideoElement>();

            let seeked_cb = video_opt.as_ref().map(|video| {
                let mse_state_for_seeked = mse_state.clone();
                let video_for_seeked = video.clone();

                let cb = Closure::<dyn Fn()>::new(move || {
                    let current_time = video_for_seeked.current_time();
                    let buf_end = get_buffer_end(&video_for_seeked);

                    // If the seek target is already buffered, nothing to do.
                    if buf_end > current_time {
                        return;
                    }

                    // Send Seek through the channel (§11.2 / §11.6).
                    let tx = {
                        let borrow = mse_state_for_seeked.borrow();
                        borrow.as_ref().map(|s| s.pump_tx.clone())
                    };
                    if let Some(tx) = tx {
                        let _ = tx.unbounded_send(PumpMsg::Seek(current_time));
                    }
                });

                video
                    .add_event_listener_with_callback("seeked", cb.as_ref().unchecked_ref())
                    .ok();
                cb
            });

            move || {
                if let (Some(cb), Some(video)) = (seeked_cb, video_opt) {
                    video
                        .remove_event_listener_with_callback(
                            "seeked",
                            cb.as_ref().unchecked_ref(),
                        )
                        .ok();
                    drop(cb);
                }
            }
        });
    }

    // Auto-hide controls after inactivity
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

    // Track fullscreen changes via the document event so that pressing Esc
    // (which exits fullscreen without going through our toggle handler)
    // correctly updates `is_fullscreen` (§11.3).
    {
        let is_fullscreen = is_fullscreen.clone();

        use_effect_with((), move |_| {
            let is_fullscreen = is_fullscreen.clone();
            let closure = Closure::<dyn Fn()>::new(move || {
                let doc = web_sys::window().unwrap().document().unwrap();
                is_fullscreen.set(doc.fullscreen_element().is_some());
            });

            if let Some(doc) = web_sys::window().and_then(|w| w.document()) {
                let _ = doc.add_event_listener_with_callback(
                    "fullscreenchange",
                    closure.as_ref().unchecked_ref(),
                );
            }

            move || {
                if let Some(doc) = web_sys::window().and_then(|w| w.document()) {
                    let _ = doc.remove_event_listener_with_callback(
                        "fullscreenchange",
                        closure.as_ref().unchecked_ref(),
                    );
                }
                drop(closure);
            }
        });
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
                        video.set_current_time(snap_to_cached_segment((current - skip).max(0.0)));
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
                        video.set_current_time(snap_to_cached_segment((current - 10.0).max(0.0)));
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
                            video.set_current_time(snap_to_cached_segment((video.current_time() + skip).min(dur)));
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
                            video.set_current_time(snap_to_cached_segment((video.current_time() + 10.0).min(dur)));
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
                            video.set_current_time(snap_to_cached_segment(dur * (num / 10.0)));
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

    // Volume slider visibility
    let on_volume_enter = {
        let volume_slider_visible = volume_slider_visible.clone();
        Callback::from(move |_| {
            volume_slider_visible.set(true);
        })
    };
    let on_volume_leave = {
        let volume_slider_visible = volume_slider_visible.clone();
        Callback::from(move |_| {
            volume_slider_visible.set(false);
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
        let quality_menu_open = quality_menu_open.clone();
        Callback::from(move |e: MouseEvent| {
            e.stop_propagation();
            speed_menu_open.set(!*speed_menu_open);
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

    // Quality menu toggle
    let on_quality_toggle = {
        let quality_menu_open = quality_menu_open.clone();
        let speed_menu_open = speed_menu_open.clone();
        Callback::from(move |e: MouseEvent| {
            e.stop_propagation();
            quality_menu_open.set(!*quality_menu_open);
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
        let speed_menu_open = speed_menu_open.clone();
        let quality_menu_open = quality_menu_open.clone();
        Callback::from(move |_: MouseEvent| {
            speed_menu_open.set(false);
            quality_menu_open.set(false);
        })
    };

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
                    video.set_current_time(snap_to_cached_segment(seek_time));
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
                        video.set_current_time(snap_to_cached_segment(seek_time));
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

            // Check for double-tap (within 300ms) (§11.5 — Option-based sentinel)
            let is_double_tap = matches!(*last_tap_time, Some(prev) if now - prev < 300.0);

            if is_double_tap {
                // Double tap detected - check which side
                if let Some(video) = video_ref.cast::<HtmlVideoElement>() {
                    let rect = video.get_bounding_client_rect();
                    let width = rect.width();
                    let relative_x = x - rect.left();

                    if relative_x < width / 3.0 {
                        // Left third - seek backward 10 seconds
                        let current = video.current_time();
                        video.set_current_time(snap_to_cached_segment((current - 10.0).max(0.0)));
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
                            video.set_current_time(snap_to_cached_segment((video.current_time() + 10.0).min(dur)));
                        }
                        skip_indicator.set(Some(("forward".to_string(), 75.0)));
                        let skip_indicator_clone = skip_indicator.clone();
                        spawn_local(async move {
                            TimeoutFuture::new(500).await;
                            skip_indicator_clone.set(None);
                        });
                    }
                }
                last_tap_time.set(None);
            } else {
                // Single tap - store time and position for potential double tap
                last_tap_time.set(Some(now));
                last_tap_x.set(x);

                // Delayed play/pause (will be cancelled if double tap occurs)
                let video_ref = video_ref.clone();
                let last_tap_time = last_tap_time.clone();
                spawn_local(async move {
                    TimeoutFuture::new(300).await;
                    // Only trigger if no second tap occurred (§11.5)
                    if (*last_tap_time).is_some() {
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

    // Captions menu toggle
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

    // Caption track selection (§11.4 — remove old <track> elements instead of
    // accumulating hidden ones)
    let on_caption_select = {
        let video_ref = video_ref.clone();
        let active_subtitle = active_subtitle.clone();
        let captions_menu_open = captions_menu_open.clone();
        let video_id = props.video_id.clone();
        Callback::from(move |track_index: Option<u32>| {
            captions_menu_open.set(false);
            active_subtitle.set(track_index);

            if let Some(video) = video_ref.cast::<HtmlVideoElement>() {
                // Remove all existing <track> child elements to avoid
                // accumulating hidden tracks on repeated selections.
                let children = video.children();
                let mut to_remove = Vec::new();
                for i in 0..children.length() {
                    if let Some(child) = children.item(i) {
                        if child.tag_name().eq_ignore_ascii_case("TRACK") {
                            to_remove.push(child);
                        }
                    }
                }
                for el in to_remove {
                    video.remove_child(&el).ok();
                }

                if let Some(index) = track_index {
                    // Create a single new <track> element.
                    let doc = web_sys::window().unwrap().document().unwrap();
                    if let Ok(track_el) = doc.create_element("track") {
                        track_el.set_attribute("kind", "captions").ok();
                        track_el.set_attribute("src", &format!("/api/videos/{}/subtitles/{}.vtt", video_id, index)).ok();
                        track_el.set_attribute("default", "").ok();

                        video.append_child(&track_el).ok();

                        // Enable the newly added track.
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

    let container_class = if *is_fullscreen {
        "player-overlay player-overlay--fullscreen"
    } else {
        "player-overlay"
    };

    // ── Derived overlay visibility ───────────────────────────────────────
    //
    // All overlay elements are **always** present in the VDOM to keep the
    // child list stable.  Yew's VDOM diff compares children by position;
    // if conditional `if` blocks insert/remove elements before the
    // `<video>` tag, the video element shifts position and Yew recreates
    // the DOM node — destroying the attached MediaSource and causing an
    // infinite reload loop.
    //
    // Instead, visibility is controlled via CSS classes (`hidden` =
    // `display:none`) so the `<video>` element always occupies the same
    // slot in the child list.
    let error_shown = (*error).is_some();
    let status_shown = !(*status).is_empty() && !error_shown;
    let buffering_shown = *is_buffering && !error_shown && !status_shown;
    let (skip_class, skip_style, skip_fwd) = match &*skip_indicator {
        Some((direction, x_pos)) => (
            format!("skip-indicator skip-indicator--{}", direction),
            format!("left: {}%;", x_pos),
            direction == "forward",
        ),
        None => (
            "skip-indicator hidden".to_string(),
            String::new(),
            true,
        ),
    };

    html! {
        <div
            ref={container_ref}
            class={container_class}
            onclick={on_container_click}
            onmousemove={on_mouse_move}
            onmouseleave={on_mouse_leave}
        >
            // Header — always rendered, toggled via CSS
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

            // Error display — always in VDOM, hidden via CSS when inactive
            <div class={if error_shown { "notice notice--error" } else { "notice notice--error hidden" }}>
                <div class="notice__title">{ "Playback error" }</div>
                <div class="notice__body">{ (*error).as_deref().unwrap_or("") }</div>
            </div>

            // Loading status — always in VDOM
            <div class={if status_shown { "player-status" } else { "player-status hidden" }}>
                { &*status }
            </div>

            // Buffering indicator — always in VDOM
            <div class={if buffering_shown { "player-buffering" } else { "player-buffering hidden" }}>
                <div class="player-buffering__spinner"></div>
            </div>

            // Skip indicator — always in VDOM
            <div class={skip_class} style={skip_style}>
                if skip_fwd {
                    <span class="skip-indicator__icon">{ icon_skip_forward() }</span>
                    <span class="skip-indicator__text">{ "10s" }</span>
                } else {
                    <span class="skip-indicator__icon">{ icon_skip_backward() }</span>
                    <span class="skip-indicator__text">{ "10s" }</span>
                }
            </div>

            // Video element — position is now STABLE in the child list
            <video
                ref={video_ref}
                class="video-el"
                onclick={on_video_click}
                ondblclick={on_video_dblclick}
            />

            // Video end overlay — always in VDOM
            <div class={if *video_ended { "video-end-overlay" } else { "video-end-overlay hidden" }}>
                <button class="video-end-overlay__replay" onclick={on_replay}>
                    <span class="replay-icon">{ icon_replay() }</span>
                    <span>{ "Replay" }</span>
                </button>
            </div>

            // Controls bar (§11.1 — ControlBar sub-component)
            <ControlBar
                visible={*controls_visible}
                progress_ref={progress_ref}
                thumbnail_canvas_ref={thumbnail_canvas_ref}
                duration={*duration}
                current_time={*current_time}
                buffered_end={*buffered_end}
                is_dragging={*is_dragging}
                is_hovering_progress={*is_hovering_progress}
                hover_time={*hover_time}
                hover_position={*hover_position}
                drag_time={*drag_time}
                thumbnail_info={(*thumbnail_info).clone()}
                on_progress_click={on_progress_click}
                on_progress_mousedown={on_progress_mousedown}
                on_progress_hover={on_progress_hover}
                on_progress_leave={on_progress_leave}
                is_playing={*is_playing}
                video_ended={*video_ended}
                playback_speed={*playback_speed}
                speed_menu_open={*speed_menu_open}
                on_play_pause={on_play_pause}
                on_speed_toggle={on_speed_toggle}
                on_speed_select={on_speed_select}
                volume={*volume}
                is_muted={*is_muted}
                volume_slider_visible={*volume_slider_visible}
                on_volume_toggle={on_volume_toggle}
                on_volume_change={on_volume_change}
                on_volume_enter={on_volume_enter}
                on_volume_leave={on_volume_leave}
                selected_quality={(*selected_quality).clone()}
                quality_menu_open={*quality_menu_open}
                on_quality_toggle={on_quality_toggle}
                on_quality_select={on_quality_select}
                subtitle_tracks={(*subtitle_tracks).clone()}
                active_subtitle={*active_subtitle}
                captions_menu_open={*captions_menu_open}
                on_captions_toggle={on_captions_toggle}
                on_caption_select={on_caption_select}
                is_fullscreen={*is_fullscreen}
                on_fullscreen_toggle={on_fullscreen_toggle}
            />
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
