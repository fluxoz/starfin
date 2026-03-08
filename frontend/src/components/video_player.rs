use gloo_net::http::Request;
use gloo_timers::callback::Interval;
use gloo_timers::future::TimeoutFuture;
use js_sys::{Array, Function, Promise, Uint8Array};
use std::cell::RefCell;
use std::rc::Rc;
use wasm_bindgen::prelude::*;
use wasm_bindgen::JsCast;
use wasm_bindgen_futures::{spawn_local, JsFuture};
use web_sys::{window, HtmlVideoElement, MediaSource, MouseEvent, SourceBuffer};
use yew::prelude::*;

// ── Buffer management constants ──────────────────────────────────────────────
// These values control how much video data we keep buffered.
// By limiting the buffer size, we avoid QuotaExceededError on large videos.

/// How many seconds of video to buffer ahead of the current playback position.
const BUFFER_AHEAD_SECONDS: f64 = 30.0;

/// How many seconds of video to keep buffered behind the current playback position.
const BUFFER_BEHIND_SECONDS: f64 = 10.0;

/// Minimum buffer level (in seconds) before we start loading more segments.
const MIN_BUFFER_AHEAD: f64 = 10.0;

/// Duration of each segment in seconds (matches backend ffmpeg -hls_time setting).
const SEGMENT_DURATION: f64 = 6.0;

/// How often (in milliseconds) to check buffer status and load new segments.
const BUFFER_CHECK_INTERVAL_MS: u32 = 500;

// ── Low-level helpers ────────────────────────────────────────────────────────

/// Returns a [`JsFuture`] that resolves the next time `event` fires on a
/// [`MediaSource`].  Uses `set_onsourceopen` so no extra web-sys features are
/// needed beyond `MediaSource` itself.
fn sourceopen_future(ms: &MediaSource) -> JsFuture {
    let p = Promise::new(&mut |resolve: Function, _: Function| {
        let cb = Closure::once_into_js(move || {
            resolve.call0(&JsValue::NULL).ok();
        });
        ms.set_onsourceopen(Some(cb.unchecked_ref()));
    });
    JsFuture::from(p)
}

/// Returns a [`JsFuture`] that resolves the next time `updateend` fires on a
/// [`SourceBuffer`].  Must be registered *before* calling `append_buffer` so
/// the event is never missed.
fn updateend_future(sb: &SourceBuffer) -> JsFuture {
    let p = Promise::new(&mut |resolve: Function, _: Function| {
        let cb = Closure::once_into_js(move || {
            resolve.call0(&JsValue::NULL).ok();
        });
        sb.set_onupdateend(Some(cb.unchecked_ref()));
    });
    JsFuture::from(p)
}

/// Fetch raw bytes from a URL via the browser's native fetch.
/// Returns an error if the request fails **or** the server responds with a
/// non-2xx status code (so that 404 / 5xx error bodies are never mistaken
/// for valid segment data).
async fn fetch_bytes(url: &str) -> Result<Vec<u8>, String> {
    let resp = Request::get(url)
        .send()
        .await
        .map_err(|e| format!("fetch error: {e:?}"))?;
    if !resp.ok() {
        return Err(format!("HTTP {} for {url}", resp.status()));
    }
    resp.binary().await.map_err(|e| format!("binary error: {e:?}"))
}

/// Parse an HLS playlist and return `(init_uri, segment_uris)`.
///
/// The backend already rewrites all URIs to absolute API paths, so no
/// base-URL resolution is needed here.
fn parse_m3u8(text: &str) -> (Option<String>, Vec<String>) {
    let mut init_uri: Option<String> = None;
    let mut segs: Vec<String> = Vec::new();

    for line in text.lines() {
        let line = line.trim();
        if let Some(rest) = line.strip_prefix("#EXT-X-MAP:URI=\"") {
            if let Some(uri) = rest.strip_suffix('"') {
                init_uri = Some(uri.to_owned());
            }
        } else if !line.starts_with('#') && !line.is_empty() {
            segs.push(line.to_owned());
        }
    }
    (init_uri, segs)
}

/// Format a time in seconds as MM:SS or HH:MM:SS.
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

/// Get the end time of the buffered data at the current playback position.
/// Returns 0.0 if nothing is buffered at the current position.
fn get_buffer_end(video: &HtmlVideoElement) -> f64 {
    let current_time = video.current_time();
    let buffered = video.buffered();
    for i in 0..buffered.length() {
        if let (Ok(start), Ok(end)) = (buffered.start(i), buffered.end(i)) {
            // Check if current position falls within this buffered range
            if current_time >= start && current_time <= end {
                return end;
            }
        }
    }
    0.0
}

/// Calculate how many seconds are buffered ahead of the current playback position.
fn get_buffer_ahead(video: &HtmlVideoElement) -> f64 {
    let buffer_end = get_buffer_end(video);
    let current_time = video.current_time();
    (buffer_end - current_time).max(0.0)
}

/// Determine which segment index corresponds to a given time position.
fn time_to_segment_index(time: f64, total_segments: usize) -> usize {
    let index = (time / SEGMENT_DURATION).floor() as usize;
    index.min(total_segments.saturating_sub(1))
}

/// Check if a specific segment is actually buffered in the video element.
/// A segment is considered sufficiently buffered if enough of it falls within
/// a buffered range to allow smooth playback. We check that at least the start
/// of the segment is buffered (the streaming loop will continue loading
/// subsequent segments as needed).
fn is_segment_buffered(video: &HtmlVideoElement, segment_index: usize) -> bool {
    let segment_start = segment_index as f64 * SEGMENT_DURATION;
    let segment_end = segment_start + SEGMENT_DURATION;
    let buffered = video.buffered();
    for i in 0..buffered.length() {
        // TimeRanges.start() and .end() can fail if index is out of bounds,
        // but we're iterating within length() so this should not happen.
        // If it does fail, we skip this range and continue checking others.
        if let (Ok(start), Ok(end)) = (buffered.start(i), buffered.end(i)) {
            // Consider segment buffered if the buffered range covers at least
            // the start of the segment and extends into it meaningfully.
            // We use a small threshold to avoid re-fetching segments that are
            // nearly fully buffered.
            if segment_start >= start && segment_start < end {
                // If the buffer extends to cover most of the segment, consider it buffered
                let buffered_portion = (end - segment_start).min(segment_end - segment_start);
                if buffered_portion >= SEGMENT_DURATION * 0.5 {
                    return true;
                }
            }
        }
    }
    false
}

/// Remove buffered data that is more than `BUFFER_BEHIND_SECONDS` behind the
/// current playback position. This frees up buffer space for new segments.
async fn remove_old_buffer(sb: &SourceBuffer, current_time: f64) -> Result<(), String> {
    // Wait for any pending update to complete
    while sb.updating() {
        updateend_future(sb)
            .await
            .map_err(|e| format!("waiting for update before remove: {e:?}"))?;
    }

    // Calculate the cutoff point - we want to keep BUFFER_BEHIND_SECONDS of data
    let remove_end = (current_time - BUFFER_BEHIND_SECONDS).max(0.0);
    
    if remove_end <= 0.0 {
        return Ok(());
    }

    // Try to remove the old data
    if let Err(e) = sb.remove(0.0, remove_end) {
        // Ignore errors from remove (it's best-effort cleanup)
        log::warn!("remove buffer failed: {e:?}");
        return Ok(());
    }

    // Wait for the remove operation to complete
    while sb.updating() {
        updateend_future(sb)
            .await
            .map_err(|e| format!("waiting for remove to complete: {e:?}"))?;
    }

    Ok(())
}

/// Check if the error is a QuotaExceededError.
fn is_quota_exceeded_error(error: &JsValue) -> bool {
    if let Some(err_str) = error.as_string() {
        return err_str.contains("QuotaExceededError") || err_str.contains("quota");
    }
    // Try to get the error name if it's an Error object
    if let Ok(name) = js_sys::Reflect::get(error, &JsValue::from_str("name")) {
        if let Some(name_str) = name.as_string() {
            return name_str == "QuotaExceededError";
        }
    }
    // Check if the debug representation contains QuotaExceededError
    let debug_str = format!("{error:?}");
    debug_str.contains("QuotaExceededError") || debug_str.contains("Quota")
}

/// Arm both the `updateend` (success) and `error` (failure) futures, then
/// call `appendBuffer`.  Awaiting the returned future blocks until the
/// SourceBuffer finishes processing.
///
/// Using `Promise.race` between the two event handlers means that a
/// SourceBuffer decode error is surfaced immediately as an `Err` rather than
/// being silently swallowed.  Without this, a decode error fires `error` then
/// `updateend`; the old code would see `updateend` and return `Ok(())`, never
/// detecting the failure.  On Chromium-based browsers a decode error also
/// triggers an internal `endOfStream("decode")` call which transitions the
/// `MediaSource` to `"ended"`, causing the *next* `appendBuffer` to throw
/// `InvalidStateError`.
async fn append_segment(sb: &SourceBuffer, data: &[u8]) -> Result<(), String> {
    // If the SourceBuffer is currently updating, wait for it to finish.
    while sb.updating() {
        updateend_future(sb)
            .await
            .map_err(|e| format!("waiting for update: {e:?}"))?;
    }

    // Register *both* listeners before calling appendBuffer so neither event
    // can be missed.
    //   • updateend_p resolves → append succeeded
    //   • error_p   rejects  → SourceBuffer decode error
    let updateend_p = Promise::new(&mut |resolve: Function, _: Function| {
        let cb = Closure::once_into_js(move || {
            resolve.call0(&JsValue::NULL).ok();
        });
        sb.set_onupdateend(Some(cb.unchecked_ref()));
    });
    let error_p = Promise::new(&mut |_: Function, reject: Function| {
        let cb = Closure::once_into_js(move || {
            reject.call0(&JsValue::NULL).ok();
        });
        sb.set_onerror(Some(cb.unchecked_ref()));
    });
    let race = Promise::race(&Array::of2(updateend_p.as_ref(), error_p.as_ref()));

    let arr = Uint8Array::from(data);
    if let Err(e) = sb.append_buffer_with_array_buffer_view(arr.unchecked_ref()) {
        // appendBuffer threw synchronously (e.g. InvalidStateError because the
        // MediaSource is no longer open).  Clear both handlers before returning.
        sb.set_onupdateend(None);
        sb.set_onerror(None);
        return Err(format!("appendBuffer: {e:?}"));
    }

    let result = JsFuture::from(race).await;
    // Clean up whichever handler did not fire.
    sb.set_onupdateend(None);
    sb.set_onerror(None);
    result.map_err(|e| format!("SourceBuffer decode error: {e:?}"))?;
    Ok(())
}

/// Append a segment with QuotaExceededError handling.
/// If quota is exceeded, removes old buffer data and retries.
async fn append_segment_with_quota_handling(
    sb: &SourceBuffer,
    data: &[u8],
    video: &HtmlVideoElement,
) -> Result<(), String> {
    // First attempt to append
    match try_append_segment(sb, data).await {
        Ok(()) => Ok(()),
        Err((err_str, err_val)) => {
            if is_quota_exceeded_error(&err_val) {
                // QuotaExceededError - try to free up buffer space
                log::info!("QuotaExceededError detected, removing old buffer data...");
                
                // Remove data behind the current playback position
                remove_old_buffer(sb, video.current_time()).await?;
                
                // Retry the append
                append_segment(sb, data).await
            } else {
                Err(err_str)
            }
        }
    }
}

/// Try to append a segment, returning both error string and JsValue on failure.
async fn try_append_segment(sb: &SourceBuffer, data: &[u8]) -> Result<(), (String, JsValue)> {
    // If the SourceBuffer is currently updating, wait for it to finish.
    while sb.updating() {
        updateend_future(sb)
            .await
            .map_err(|e| (format!("waiting for update: {e:?}"), e))?;
    }

    let updateend_p = Promise::new(&mut |resolve: Function, _: Function| {
        let cb = Closure::once_into_js(move || {
            resolve.call0(&JsValue::NULL).ok();
        });
        sb.set_onupdateend(Some(cb.unchecked_ref()));
    });
    let error_p = Promise::new(&mut |_: Function, reject: Function| {
        let cb = Closure::once_into_js(move || {
            // Reject with an error indicator - the actual error details are
            // available on the SourceBuffer/MediaSource error properties
            reject.call1(&JsValue::NULL, &JsValue::from_str("SourceBuffer error event")).ok();
        });
        sb.set_onerror(Some(cb.unchecked_ref()));
    });
    let race = Promise::race(&Array::of2(updateend_p.as_ref(), error_p.as_ref()));

    let arr = Uint8Array::from(data);
    if let Err(e) = sb.append_buffer_with_array_buffer_view(arr.unchecked_ref()) {
        sb.set_onupdateend(None);
        sb.set_onerror(None);
        return Err((format!("appendBuffer: {e:?}"), e));
    }

    let result = JsFuture::from(race).await;
    sb.set_onupdateend(None);
    sb.set_onerror(None);
    result.map_err(|e| (format!("SourceBuffer decode error: {e:?}"), e))?;
    Ok(())
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
    // Human-readable status shown while buffering.
    let status = use_state(|| "Preparing stream…".to_string());
    let error = use_state(|| Option::<String>::None);
    
    // Playback state for custom controls
    let current_time = use_state(|| 0.0_f64);
    let duration = use_state(|| 0.0_f64);
    let buffered_end = use_state(|| 0.0_f64);
    let is_playing = use_state(|| false);
    
    // Drag state for progress bar scrubbing
    let is_dragging = use_state(|| false);
    let drag_time = use_state(|| 0.0_f64);

    // Effect to run the player logic
    {
        let video_ref = video_ref.clone();
        let video_id = props.video_id.clone();
        let status = status.clone();
        let error = error.clone();

        use_effect_with(props.video_id.clone(), move |_| {
            spawn_local(async move {
                if let Err(msg) = run_player(video_ref, &video_id, status).await {
                    error.set(Some(msg));
                }
            });
            || ()
        });
    }

    // Effect to update time/duration state periodically (skip during drag)
    {
        let video_ref = video_ref.clone();
        let current_time = current_time.clone();
        let duration = duration.clone();
        let buffered_end = buffered_end.clone();
        let is_playing = is_playing.clone();
        let is_dragging = is_dragging.clone();

        use_effect_with(video_ref.clone(), move |video_ref| {
            let video_ref = video_ref.clone();
            let interval = Interval::new(250, move || {
                if let Some(video) = video_ref.cast::<HtmlVideoElement>() {
                    // Only update current_time if not dragging
                    if !*is_dragging {
                        current_time.set(video.current_time());
                    }
                    let dur = video.duration();
                    if dur.is_finite() && dur > 0.0 {
                        duration.set(dur);
                    }
                    buffered_end.set(get_buffer_end(&video));
                    is_playing.set(!video.paused());
                }
            });
            move || drop(interval)
        });
    }

    let on_close = props.on_close.clone();
    let title = props.title.clone();

    // Play/pause toggle
    let on_play_pause = {
        let video_ref = video_ref.clone();
        Callback::from(move |_| {
            if let Some(video) = video_ref.cast::<HtmlVideoElement>() {
                if video.paused() {
                    let _ = video.play();
                } else {
                    let _ = video.pause();
                }
            }
        })
    };

    // Helper function to calculate seek time from mouse position
    fn calculate_seek_time(
        e: &MouseEvent,
        progress_el: &web_sys::HtmlElement,
        video_duration: f64,
    ) -> Option<f64> {
        let rect = progress_el.get_bounding_client_rect();
        let click_x = e.client_x() as f64 - rect.left();
        let width = rect.width();
        if width > 0.0 && video_duration.is_finite() && video_duration > 0.0 {
            let seek_ratio = (click_x / width).clamp(0.0, 1.0);
            Some(seek_ratio * video_duration)
        } else {
            None
        }
    }

    // Mouse down on progress bar - start dragging
    let on_progress_mousedown = {
        let video_ref = video_ref.clone();
        let progress_ref = progress_ref.clone();
        let is_dragging = is_dragging.clone();
        let drag_time = drag_time.clone();
        let current_time = current_time.clone();
        Callback::from(move |e: MouseEvent| {
            e.prevent_default();
            if let Some(progress_el) = progress_ref.cast::<web_sys::HtmlElement>() {
                if let Some(video) = video_ref.cast::<HtmlVideoElement>() {
                    let video_duration = video.duration();
                    if let Some(seek_time) = calculate_seek_time(&e, &progress_el, video_duration) {
                        is_dragging.set(true);
                        drag_time.set(seek_time);
                        current_time.set(seek_time);
                    }
                }
            }
        })
    };

    // Effect to handle global mousemove and mouseup for dragging
    {
        let video_ref = video_ref.clone();
        let progress_ref = progress_ref.clone();
        let is_dragging = is_dragging.clone();
        let drag_time = drag_time.clone();
        let current_time = current_time.clone();
        let duration_state = duration.clone();

        use_effect_with(is_dragging.clone(), move |is_dragging| {
            // Store event listeners in RefCell so we can clean them up
            let closures: Rc<RefCell<Option<(Closure<dyn Fn(MouseEvent)>, Closure<dyn Fn(MouseEvent)>)>>> = 
                Rc::new(RefCell::new(None));
            
            if **is_dragging {
                let is_dragging_move = is_dragging.clone();
                let is_dragging_up = is_dragging.clone();
                let drag_time_move = drag_time.clone();
                let drag_time_up = drag_time.clone();
                let current_time_move = current_time.clone();
                let video_ref_up = video_ref.clone();
                let progress_ref_move = progress_ref.clone();
                let duration_state_move = duration_state.clone();

                // Mousemove handler - update drag position
                let on_mousemove = Closure::<dyn Fn(MouseEvent)>::new(move |e: MouseEvent| {
                    if !*is_dragging_move {
                        return;
                    }
                    if let Some(progress_el) = progress_ref_move.cast::<web_sys::HtmlElement>() {
                        let video_duration = *duration_state_move;
                        let rect = progress_el.get_bounding_client_rect();
                        let click_x = e.client_x() as f64 - rect.left();
                        let width = rect.width();
                        if width > 0.0 && video_duration > 0.0 {
                            let seek_ratio = (click_x / width).clamp(0.0, 1.0);
                            let seek_time = seek_ratio * video_duration;
                            drag_time_move.set(seek_time);
                            current_time_move.set(seek_time);
                        }
                    }
                });

                // Mouseup handler - finish dragging and seek
                let on_mouseup = Closure::<dyn Fn(MouseEvent)>::new(move |_: MouseEvent| {
                    if !*is_dragging_up {
                        return;
                    }
                    is_dragging_up.set(false);
                    let seek_time = *drag_time_up;
                    if let Some(video) = video_ref_up.cast::<HtmlVideoElement>() {
                        video.set_current_time(seek_time);
                    }
                });

                // Add event listeners to window
                if let Some(win) = window() {
                    let _ = win.add_event_listener_with_callback(
                        "mousemove",
                        on_mousemove.as_ref().unchecked_ref(),
                    );
                    let _ = win.add_event_listener_with_callback(
                        "mouseup",
                        on_mouseup.as_ref().unchecked_ref(),
                    );
                    
                    // Store closures to prevent them from being dropped
                    *closures.borrow_mut() = Some((on_mousemove, on_mouseup));
                }
            }
            
            // Cleanup function
            let closures_cleanup = closures;
            move || {
                if let Some((mousemove_closure, mouseup_closure)) = closures_cleanup.borrow_mut().take() {
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
            }
        });
    }

    // Click on progress bar - immediate seek (for clicks without drag)
    let on_progress_click = {
        let video_ref = video_ref.clone();
        let progress_ref = progress_ref.clone();
        let is_dragging = is_dragging.clone();
        Callback::from(move |e: MouseEvent| {
            // Don't handle click if we just finished dragging
            if *is_dragging {
                return;
            }
            if let Some(progress_el) = progress_ref.cast::<web_sys::HtmlElement>() {
                if let Some(video) = video_ref.cast::<HtmlVideoElement>() {
                    let video_duration = video.duration();
                    if let Some(seek_time) = calculate_seek_time(&e, &progress_el, video_duration) {
                        video.set_current_time(seek_time);
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

    let time_display = format!("{} / {}", format_time(*current_time), format_time(*duration));
    let play_pause_label = if *is_playing { "⏸" } else { "▶" };

    html! {
        <div class="player-overlay">
            <div class="player-header">
                <button
                    class="btn btn--back"
                    onclick={Callback::from(move |_| on_close.emit(()))}
                >
                    { "← Back to library" }
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

            <video
                ref={video_ref}
                class="video-el"
            />

            // Custom controls bar
            <div class="player-controls">
                <button class="player-controls__btn" onclick={on_play_pause}>
                    { play_pause_label }
                </button>
                <div 
                    ref={progress_ref}
                    class="player-progress"
                    onclick={on_progress_click}
                    onmousedown={on_progress_mousedown}
                >
                    <div 
                        class="player-progress__buffered"
                        style={format!("width: {}%", buffered_percent)}
                    />
                    <div 
                        class="player-progress__played"
                        style={format!("width: {}%", progress_percent)}
                    />
                    <div 
                        class="player-progress__thumb"
                        style={format!("left: {}%", progress_percent)}
                    />
                </div>
                <span class="player-controls__time">{ time_display }</span>
            </div>
        </div>
    }
}

// ── Player logic (async) ─────────────────────────────────────────────────────

/// State for tracking which segments have been loaded.
struct BufferState {
    /// Set of segment indices that have been loaded.
    loaded_segments: std::collections::HashSet<usize>,
    /// Total number of segments in the playlist.
    total_segments: usize,
}

impl BufferState {
    fn new(total_segments: usize) -> Self {
        Self {
            loaded_segments: std::collections::HashSet::new(),
            total_segments,
        }
    }

    fn mark_loaded(&mut self, index: usize) {
        self.loaded_segments.insert(index);
    }

    fn all_loaded(&self) -> bool {
        self.loaded_segments.len() >= self.total_segments
    }
}

/// All async work for setting up and feeding the MSE player.
/// Separated from the component to keep error handling clean.
async fn run_player(
    video_ref: NodeRef,
    video_id: &str,
    status: UseStateHandle<String>,
) -> Result<(), String> {
    let playlist_url = format!("/api/videos/{video_id}/playlist.m3u8");

    let video = video_ref
        .cast::<HtmlVideoElement>()
        .ok_or("video element unavailable")?;

    // ── Safari: native HLS support via <video src="playlist.m3u8"> ───────────
    // `canPlayType` returns "" (no), "maybe", or "probably".
    if !video.can_play_type("application/vnd.apple.mpegurl").is_empty() {
        video.set_src(&playlist_url);
        status.set(String::new());
        return Ok(());
    }

    // ── Other browsers: fMP4 HLS via the Media Source Extensions API ─────────
    // H.264 Baseline 3.1 / AAC-LC – the most universally supported combination.
    let mime = r#"video/mp4; codecs="avc1.42E01E,mp4a.40.2""#;
    if !MediaSource::is_type_supported(mime) {
        return Err(
            "Your browser does not support the required video codec (H.264 + AAC in fMP4)."
                .into(),
        );
    }

    // Fetch and parse the HLS playlist.
    status.set("Fetching playlist…".into());
    let playlist_bytes = fetch_bytes(&playlist_url).await?;
    let playlist_text = String::from_utf8(playlist_bytes)
        .map_err(|e| format!("playlist UTF-8: {e}"))?;
    let (init_uri, seg_uris) = parse_m3u8(&playlist_text);

    if seg_uris.is_empty() {
        return Err("Playlist contains no segments.".into());
    }

    // Create a MediaSource and attach it to the <video> element via an object URL.
    let ms = MediaSource::new().map_err(|e| format!("MediaSource::new: {e:?}"))?;
    let obj_url =
        web_sys::Url::create_object_url_with_source(&ms).map_err(|e| format!("createObjectURL: {e:?}"))?;
    video.set_src(&obj_url);

    // Wait until the MediaSource transitions to "open".
    sourceopen_future(&ms).await.map_err(|e| format!("sourceopen: {e:?}"))?;

    let sb = ms
        .add_source_buffer(mime)
        .map_err(|e| format!("addSourceBuffer: {e:?}"))?;

    // Append the fMP4 initialisation segment (codec + track info).
    if let Some(init_url) = init_uri {
        status.set("Loading init segment…".into());
        let data = fetch_bytes(&init_url).await?;
        append_segment(&sb, &data).await?;
    }

    // Initialize buffer state tracking
    let buffer_state = Rc::new(RefCell::new(BufferState::new(seg_uris.len())));
    
    // Stream the first few media segments so playback can begin quickly.
    let initial_count = 2.min(seg_uris.len());
    for (i, url) in seg_uris[..initial_count].iter().enumerate() {
        status.set(format!("Buffering segment {}/{}…", i + 1, seg_uris.len()));
        let data = fetch_bytes(url).await?;
        append_segment_with_quota_handling(&sb, &data, &video).await?;
        buffer_state.borrow_mut().mark_loaded(i);
    }

    // Playback is ready – clear the status overlay.
    status.set(String::new());

    // ── Demand-based streaming loop ──────────────────────────────────────────
    // Instead of loading all segments at once, we continuously monitor the
    // playback position and buffer level, loading segments as needed.
    // This prevents QuotaExceededError by maintaining a sliding buffer window.
    
    loop {
        // Check if we need to load more segments
        let current_time = video.current_time();
        let buffer_ahead = get_buffer_ahead(&video);
        let buffer_state_ref = buffer_state.borrow();
        
        // If we've loaded all segments, signal end of stream and exit
        if buffer_state_ref.all_loaded() {
            drop(buffer_state_ref);
            // Wait for any pending updates before ending stream
            while sb.updating() {
                updateend_future(&sb)
                    .await
                    .map_err(|e| format!("waiting for final update: {e:?}"))?;
            }
            ms.end_of_stream().map_err(|e| format!("endOfStream: {e:?}"))?;
            web_sys::Url::revoke_object_url(&obj_url).ok();
            return Ok(());
        }
        
        // If buffer is low, load more segments
        if buffer_ahead < MIN_BUFFER_AHEAD {
            drop(buffer_state_ref);
            
            // First, try to remove old buffered data to make room
            remove_old_buffer(&sb, current_time).await?;
            
            // Calculate which segments we need to load
            let current_segment = time_to_segment_index(current_time, seg_uris.len());
            let target_buffer_end = current_time + BUFFER_AHEAD_SECONDS;
            let target_segment = time_to_segment_index(target_buffer_end, seg_uris.len());
            
            // Load segments from current position up to target
            for seg_idx in current_segment..=target_segment {
                if seg_idx >= seg_uris.len() {
                    break;
                }
                
                // Check if segment is actually in the buffer - this handles the case
                // where a user seeks to a position that was previously loaded but has
                // since been evicted from the buffer by remove_old_buffer.
                if is_segment_buffered(&video, seg_idx) {
                    continue;
                }
                
                let mut state = buffer_state.borrow_mut();
                // Mark as loaded before dropping the borrow
                state.mark_loaded(seg_idx);
                drop(state);
                
                // Fetch and append the segment
                let url = &seg_uris[seg_idx];
                let data = fetch_bytes(url).await.map_err(|e| {
                    format!("Segment {seg_idx} fetch failed: {e}")
                })?;
                
                append_segment_with_quota_handling(&sb, &data, &video).await?;
            }
        } else {
            drop(buffer_state_ref);
        }
        
        // Small delay before checking buffer status again
        // This prevents busy-waiting while still being responsive
        TimeoutFuture::new(BUFFER_CHECK_INTERVAL_MS).await;
    }
}
