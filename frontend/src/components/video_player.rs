use gloo_net::http::Request;
use gloo_timers::callback::Interval;
use gloo_timers::future::TimeoutFuture;
use js_sys::{Array, Function, Promise, Uint8Array};
use serde::Deserialize;
use std::cell::{Cell, RefCell};
use std::rc::Rc;
use wasm_bindgen::prelude::*;
use wasm_bindgen::JsCast;
use wasm_bindgen_futures::{spawn_local, JsFuture};
use web_sys::{window, HtmlVideoElement, KeyboardEvent, MediaSource, MouseEvent, SourceBuffer};
use yew::prelude::*;

// ── Buffer management constants ──────────────────────────────────────────────
const BUFFER_AHEAD_SECONDS: f64 = 30.0;
const BUFFER_BEHIND_SECONDS: f64 = 10.0;
const MIN_BUFFER_AHEAD: f64 = 5.0; // Reduced for faster seeking response
const SEGMENT_DURATION: f64 = 6.0;
const BUFFER_CHECK_INTERVAL_MS: u32 = 100; // Faster checks for responsive seeking
const MIN_BUFFER_CLEANUP_THRESHOLD: f64 = 1.0; // Minimum seconds before we clean up old buffer

// ── Playback speed options ───────────────────────────────────────────────────
const PLAYBACK_SPEEDS: [f64; 9] = [0.25, 0.5, 0.75, 1.0, 1.25, 1.5, 1.75, 2.0, 3.0];

// ── Low-level helpers ────────────────────────────────────────────────────────

fn sourceopen_future(ms: &MediaSource) -> JsFuture {
    let p = Promise::new(&mut |resolve: Function, _: Function| {
        let cb = Closure::once_into_js(move || {
            resolve.call0(&JsValue::NULL).ok();
        });
        ms.set_onsourceopen(Some(cb.unchecked_ref()));
    });
    JsFuture::from(p)
}

fn updateend_future(sb: &SourceBuffer) -> JsFuture {
    let p = Promise::new(&mut |resolve: Function, _: Function| {
        let cb = Closure::once_into_js(move || {
            resolve.call0(&JsValue::NULL).ok();
        });
        sb.set_onupdateend(Some(cb.unchecked_ref()));
    });
    JsFuture::from(p)
}

async fn fetch_bytes(url: &str) -> Result<Vec<u8>, String> {
    let resp = Request::get(url)
        .send()
        .await
        .map_err(|e| format!("fetch error: {e:?}"))?;
    if !resp.ok() {
        return Err(format!("HTTP {} for {url}", resp.status()));
    }
    resp.binary()
        .await
        .map_err(|e| format!("binary error: {e:?}"))
}

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

fn get_buffer_ahead(video: &HtmlVideoElement) -> f64 {
    let buffer_end = get_buffer_end(video);
    let current_time = video.current_time();
    (buffer_end - current_time).max(0.0)
}

fn time_to_segment_index(time: f64, total_segments: usize) -> usize {
    let index = (time / SEGMENT_DURATION).floor() as usize;
    index.min(total_segments.saturating_sub(1))
}

fn is_segment_buffered(video: &HtmlVideoElement, segment_index: usize) -> bool {
    let segment_start = segment_index as f64 * SEGMENT_DURATION;
    let segment_end = segment_start + SEGMENT_DURATION;
    let buffered = video.buffered();
    for i in 0..buffered.length() {
        if let (Ok(start), Ok(end)) = (buffered.start(i), buffered.end(i)) {
            if segment_start >= start && segment_start < end {
                let buffered_portion = (end - segment_start).min(segment_end - segment_start);
                if buffered_portion >= SEGMENT_DURATION * 0.5 {
                    return true;
                }
            }
        }
    }
    false
}

async fn remove_old_buffer(sb: &SourceBuffer, current_time: f64) -> Result<(), String> {
    while sb.updating() {
        updateend_future(sb)
            .await
            .map_err(|e| format!("waiting for update before remove: {e:?}"))?;
    }

    let remove_end = (current_time - BUFFER_BEHIND_SECONDS).max(0.0);

    if remove_end <= 0.0 {
        return Ok(());
    }

    if let Err(e) = sb.remove(0.0, remove_end) {
        log::warn!("remove buffer failed: {e:?}");
        return Ok(());
    }

    while sb.updating() {
        updateend_future(sb)
            .await
            .map_err(|e| format!("waiting for remove to complete: {e:?}"))?;
    }

    Ok(())
}

fn is_quota_exceeded_error(error: &JsValue) -> bool {
    if let Some(err_str) = error.as_string() {
        return err_str.contains("QuotaExceededError") || err_str.contains("quota");
    }
    if let Ok(name) = js_sys::Reflect::get(error, &JsValue::from_str("name")) {
        if let Some(name_str) = name.as_string() {
            return name_str == "QuotaExceededError";
        }
    }
    let debug_str = format!("{error:?}");
    debug_str.contains("QuotaExceededError") || debug_str.contains("Quota")
}

async fn append_segment(sb: &SourceBuffer, data: &[u8]) -> Result<(), String> {
    while sb.updating() {
        updateend_future(sb)
            .await
            .map_err(|e| format!("waiting for update: {e:?}"))?;
    }

    let updateend_p = Promise::new(&mut |resolve: Function, _: Function| {
        let cb = Closure::once_into_js(move || {
            resolve.call0(&JsValue::NULL).ok();
        });
        sb.set_onupdateend(Some(cb.unchecked_ref()));
    });
    let error_p = Promise::new(&mut |_: Function, reject: Function| {
        let cb = Closure::once_into_js(move || {
            reject
                .call1(&JsValue::NULL, &JsValue::from_str("SourceBuffer error event"))
                .ok();
        });
        sb.set_onerror(Some(cb.unchecked_ref()));
    });
    let race = Promise::race(&Array::of2(updateend_p.as_ref(), error_p.as_ref()));

    let arr = Uint8Array::from(data);
    if let Err(e) = sb.append_buffer_with_array_buffer_view(arr.unchecked_ref()) {
        sb.set_onupdateend(None);
        sb.set_onerror(None);
        return Err(format!("appendBuffer: {e:?}"));
    }

    let result = JsFuture::from(race).await;
    sb.set_onupdateend(None);
    sb.set_onerror(None);
    result.map_err(|e| format!("SourceBuffer decode error: {e:?}"))?;
    Ok(())
}

async fn append_segment_with_quota_handling(
    sb: &SourceBuffer,
    data: &[u8],
    video: &HtmlVideoElement,
) -> Result<(), String> {
    match try_append_segment(sb, data).await {
        Ok(()) => Ok(()),
        Err((err_str, err_val)) => {
            if is_quota_exceeded_error(&err_val) {
                log::info!("QuotaExceededError detected, removing old buffer data...");
                remove_old_buffer(sb, video.current_time()).await?;
                append_segment(sb, data).await
            } else {
                Err(err_str)
            }
        }
    }
}

async fn try_append_segment(sb: &SourceBuffer, data: &[u8]) -> Result<(), (String, JsValue)> {
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
            reject
                .call1(&JsValue::NULL, &JsValue::from_str("SourceBuffer error event"))
                .ok();
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

// ── Quality Level Info ───────────────────────────────────────────────────────

#[derive(Clone, Debug, PartialEq)]
pub struct QualityLevel {
    pub index: usize,
    pub name: String,
    pub bitrate: u64,
    pub resolution: Option<(u32, u32)>,
}

impl QualityLevel {
    pub fn display_name(&self) -> String {
        if let Some((_, height)) = self.resolution {
            format!("{}p", height)
        } else if self.bitrate > 0 {
            format!("{}kbps", self.bitrate / 1000)
        } else {
            self.name.clone()
        }
    }
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
    let last_mouse_move = use_state(|| js_sys::Date::now());
    let settings_open = use_state(|| false);
    let speed_menu_open = use_state(|| false);
    let volume_slider_visible = use_state(|| false);

    // Fullscreen state
    let is_fullscreen = use_state(|| false);

    // Playback speed
    let playback_speed = use_state(|| 1.0_f64);

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

    // Quality level state for adaptive streaming
    let quality_levels = use_state(|| Vec::<QualityLevel>::new());
    let current_quality = use_state(|| -1_i32); // -1 = auto, otherwise level index
    let quality_menu_open = use_state(|| false);
    #[allow(unused_variables)]
    let bandwidth_estimate = use_state(|| 0_u64); // bits per second (used for ABR display)

    // Run the MSE player logic
    {
        let video_ref = video_ref.clone();
        let video_id = props.video_id.clone();
        let status = status.clone();
        let error = error.clone();
        let thumbnail_info = thumbnail_info.clone();
        let thumbnail_image = thumbnail_image.clone();
        let subtitle_tracks = subtitle_tracks.clone();

        use_effect_with(props.video_id.clone(), move |_| {
            // Fetch thumbnail info and load sprite
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

            spawn_local(async move {
                if let Err(msg) = run_player(video_ref, &video_id, status).await {
                    error.set(Some(msg));
                }
            });
            || ()
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
            // 150ms gives good responsiveness while being more efficient than 100ms
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
        let is_playing = is_playing.clone();
        let settings_open = settings_open.clone();

        use_effect_with(
            ((*is_playing).clone(), (*settings_open).clone()),
            move |_| {
                let controls_visible = controls_visible.clone();
                let last_mouse_move = last_mouse_move.clone();
                let is_playing = is_playing.clone();
                let settings_open = settings_open.clone();

                let interval = Interval::new(1000, move || {
                    if *is_playing && !*settings_open {
                        let now = js_sys::Date::now();
                        if now - *last_mouse_move > 3000.0 {
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
                        video.set_current_time((video.current_time() - skip).max(0.0));
                        skip_indicator.set(Some(("backward".to_string(), 25.0)));
                        // Clear indicator after animation
                        let skip_indicator_clone = skip_indicator.clone();
                        spawn_local(async move {
                            TimeoutFuture::new(500).await;
                            skip_indicator_clone.set(None);
                        });
                    }
                    "j" | "J" => {
                        e.prevent_default();
                        video.set_current_time((video.current_time() - 10.0).max(0.0));
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
                            video.set_current_time((video.current_time() + skip).min(dur));
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
                            video.set_current_time((video.current_time() + 10.0).min(dur));
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
                            video.set_current_time(dur * (num / 10.0));
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

    // Mouse move handler for showing controls
    let on_mouse_move = {
        let controls_visible = controls_visible.clone();
        let last_mouse_move = last_mouse_move.clone();
        Callback::from(move |_: MouseEvent| {
            controls_visible.set(true);
            last_mouse_move.set(js_sys::Date::now());
        })
    };

    // Mouse leave handler
    let on_mouse_leave = {
        let is_playing = is_playing.clone();
        let controls_visible = controls_visible.clone();
        Callback::from(move |_: MouseEvent| {
            if *is_playing {
                controls_visible.set(false);
            }
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
        Callback::from(move |e: MouseEvent| {
            e.stop_propagation();
            speed_menu_open.set(!*speed_menu_open);
            settings_open.set(false);
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
        let settings_open = settings_open.clone();
        let speed_menu_open = speed_menu_open.clone();
        Callback::from(move |e: MouseEvent| {
            e.stop_propagation();
            quality_menu_open.set(!*quality_menu_open);
            settings_open.set(false);
            speed_menu_open.set(false);
        })
    };

    // Quality selection
    let on_quality_select = {
        let current_quality = current_quality.clone();
        let quality_menu_open = quality_menu_open.clone();
        Callback::from(move |level: i32| {
            current_quality.set(level);
            quality_menu_open.set(false);
            // Note: Actual quality switching would be handled by the HLS controller
            // For now, this just updates the UI state
            log::info!("Quality level selected: {}", if level < 0 { "Auto".to_string() } else { format!("{}", level) });
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
                    video.set_current_time(seek_time);
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
                        video.set_current_time(seek_time);
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
                        video.set_current_time((video.current_time() - 10.0).max(0.0));
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
                            video.set_current_time((video.current_time() + 10.0).min(dur));
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
        Callback::from(move |e: MouseEvent| {
            e.stop_propagation();
            captions_menu_open.set(!*captions_menu_open);
            settings_open.set(false);
            speed_menu_open.set(false);
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
    let play_pause_icon = if *video_ended {
        "↻"
    } else if *is_playing {
        "⏸"
    } else {
        "▶"
    };

    let volume_icon = if *is_muted || *volume == 0.0 {
        "🔇"
    } else if *volume < 0.5 {
        "🔉"
    } else {
        "🔊"
    };

    let fullscreen_icon = if *is_fullscreen { "⧉" } else { "⛶" };

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
                    onclick={Callback::from(move |_| on_close.emit(()))}
                >
                    { "← Back" }
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
                        <span class="skip-indicator__icon">{ "▶▶" }</span>
                        <span class="skip-indicator__text">{ "10s" }</span>
                    } else {
                        <span class="skip-indicator__icon">{ "◀◀" }</span>
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
                        <span class="replay-icon">{ "↻" }</span>
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

                        // Quality selector button
                        <div class="player-quality">
                            <button
                                class="player-controls__btn player-controls__btn--text"
                                onclick={on_quality_toggle}
                                title="Quality"
                            >
                                { if *current_quality < 0 { "Auto".to_string() } else { 
                                    quality_levels.get(*current_quality as usize)
                                        .map(|l| l.display_name())
                                        .unwrap_or_else(|| "Auto".to_string())
                                }}
                            </button>
                            if *quality_menu_open {
                                <div class="player-quality__menu">
                                    <button
                                        class={if *current_quality < 0 { "player-quality__option player-quality__option--active" } else { "player-quality__option" }}
                                        onclick={Callback::from({
                                            let on_select = on_quality_select.clone();
                                            move |e: MouseEvent| {
                                                e.stop_propagation();
                                                on_select.emit(-1);
                                            }
                                        })}
                                    >
                                        { "Auto" }
                                    </button>
                                    { for quality_levels.iter().enumerate().map(|(i, level)| {
                                        let on_select = on_quality_select.clone();
                                        let is_active = *current_quality == i as i32;
                                        let level_clone = level.clone();
                                        html! {
                                            <button
                                                class={if is_active { "player-quality__option player-quality__option--active" } else { "player-quality__option" }}
                                                onclick={Callback::from(move |e: MouseEvent| {
                                                    e.stop_propagation();
                                                    on_select.emit(i as i32);
                                                })}
                                            >
                                                { level_clone.display_name() }
                                            </button>
                                        }
                                    })}
                                </div>
                            }
                        </div>

                        // Settings button
                        <div class="player-settings">
                            <button
                                class="player-controls__btn"
                                onclick={on_settings_toggle}
                                title="Settings"
                            >
                                { "⚙" }
                            </button>
                            if *settings_open {
                                <div class="player-settings__menu">
                                    <div class="player-settings__item">
                                        <span>{ "Quality" }</span>
                                        <span class="player-settings__value">{ 
                                            if *current_quality < 0 { 
                                                "Auto".to_string() 
                                            } else { 
                                                quality_levels.get(*current_quality as usize)
                                                    .map(|l| l.display_name())
                                                    .unwrap_or_else(|| "Auto".to_string())
                                            }
                                        }</span>
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
                            { "🖼" }
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

// ── Player Logic ─────────────────────────────────────────────────────────────

/// Track which segments have been fetched in the current session
/// This helps avoid re-fetching but we still check actual buffer state
struct FetchedSegments {
    fetched: std::collections::HashSet<usize>,
    total_segments: usize,
}

impl FetchedSegments {
    fn new(total_segments: usize) -> Self {
        Self {
            fetched: std::collections::HashSet::new(),
            total_segments,
        }
    }

    fn mark_fetched(&mut self, index: usize) {
        self.fetched.insert(index);
    }

    fn all_fetched(&self) -> bool {
        self.fetched.len() >= self.total_segments
    }
}

async fn run_player(
    video_ref: NodeRef,
    video_id: &str,
    status: UseStateHandle<String>,
) -> Result<(), String> {
    let playlist_url = format!("/api/videos/{video_id}/playlist.m3u8");

    let video = video_ref
        .cast::<HtmlVideoElement>()
        .ok_or("video element unavailable")?;

    // Safari: native HLS support
    if !video
        .can_play_type("application/vnd.apple.mpegurl")
        .is_empty()
    {
        video.set_src(&playlist_url);
        status.set(String::new());
        return Ok(());
    }

    // Other browsers: fMP4 HLS via MSE
    let mime = r#"video/mp4; codecs="avc1.42E01E,mp4a.40.2""#;
    if !MediaSource::is_type_supported(mime) {
        return Err(
            "Your browser does not support the required video codec (H.264 + AAC in fMP4).".into(),
        );
    }

    // Fetch and parse playlist
    status.set("Fetching playlist…".into());
    let playlist_bytes = fetch_bytes(&playlist_url).await?;
    let playlist_text =
        String::from_utf8(playlist_bytes).map_err(|e| format!("playlist UTF-8: {e}"))?;
    let (init_uri, seg_uris) = parse_m3u8(&playlist_text);

    if seg_uris.is_empty() {
        return Err("Playlist contains no segments.".into());
    }

    // Create MediaSource
    let ms = MediaSource::new().map_err(|e| format!("MediaSource::new: {e:?}"))?;
    let obj_url = web_sys::Url::create_object_url_with_source(&ms)
        .map_err(|e| format!("createObjectURL: {e:?}"))?;
    video.set_src(&obj_url);

    // Helper to revoke URL on error - wrap inner logic
    let result = run_player_inner(&ms, &video, &obj_url, mime, init_uri, seg_uris, status).await;
    
    // Always revoke the object URL when done (success or error)
    web_sys::Url::revoke_object_url(&obj_url).ok();
    
    result
}

/// Inner player logic - separated so we can ensure URL cleanup in the outer function
async fn run_player_inner(
    ms: &MediaSource,
    video: &HtmlVideoElement,
    _obj_url: &str,
    mime: &str,
    init_uri: Option<String>,
    seg_uris: Vec<String>,
    status: UseStateHandle<String>,
) -> Result<(), String> {
    // Wait for MediaSource to open
    sourceopen_future(ms)
        .await
        .map_err(|e| format!("sourceopen: {e:?}"))?;

    let sb = ms
        .add_source_buffer(mime)
        .map_err(|e| format!("addSourceBuffer: {e:?}"))?;

    // Append init segment
    if let Some(init_url) = init_uri {
        status.set("Loading init segment…".into());
        let data = fetch_bytes(&init_url).await?;
        append_segment(&sb, &data).await?;
    }

    // Track which segments have been fetched (to know when we can call end_of_stream)
    let fetched_segments = Rc::new(RefCell::new(FetchedSegments::new(seg_uris.len())));

    // Load initial segments to start playback
    let initial_count = 2.min(seg_uris.len());
    for (i, url) in seg_uris[..initial_count].iter().enumerate() {
        status.set(format!(
            "Buffering segment {}/{}…",
            i + 1,
            seg_uris.len()
        ));
        let data = fetch_bytes(url).await?;
        append_segment_with_quota_handling(&sb, &data, video).await?;
        fetched_segments.borrow_mut().mark_fetched(i);
    }

    // Clear status - playback ready
    status.set(String::new());

    // Track if we've signaled end of stream
    let mut end_of_stream_signaled = false;

    // Demand-based streaming loop - runs continuously while video is active
    loop {
        let current_time = video.current_time();
        let video_duration = video.duration();
        
        // Check if video element is still valid
        if !video_duration.is_finite() || video_duration <= 0.0 {
            // Video might be detached, wait and check again
            TimeoutFuture::new(BUFFER_CHECK_INTERVAL_MS).await;
            continue;
        }

        // Calculate which segments we need around current position
        let current_segment = time_to_segment_index(current_time, seg_uris.len());
        let target_buffer_end = current_time + BUFFER_AHEAD_SECONDS;
        let target_segment = time_to_segment_index(target_buffer_end, seg_uris.len());

        // Check buffer state and fetch needed segments
        let buffer_ahead = get_buffer_ahead(video);
        
        // If buffer is low, fetch segments around current position
        if buffer_ahead < MIN_BUFFER_AHEAD {
            // First, clean up old buffer data (but be careful not to remove too much)
            let remove_before = (current_time - BUFFER_BEHIND_SECONDS).max(0.0);
            if remove_before > MIN_BUFFER_CLEANUP_THRESHOLD {
                // Only remove if there's significant data behind to avoid thrashing
                let _ = safe_remove_buffer(&sb, 0.0, remove_before).await;
            }

            // Fetch segments from current position forward
            for seg_idx in current_segment..=target_segment {
                if seg_idx >= seg_uris.len() {
                    break;
                }

                // Always check actual buffer state - segment might have been evicted
                if is_segment_buffered(video, seg_idx) {
                    // Segment is already in buffer, mark as fetched and skip
                    fetched_segments.borrow_mut().mark_fetched(seg_idx);
                    continue;
                }

                // Fetch and append the segment
                let url = &seg_uris[seg_idx];
                match fetch_bytes(url).await {
                    Ok(data) => {
                        if let Err(e) = append_segment_with_quota_handling(&sb, &data, video).await {
                            // Log error but continue - might recover
                            log::warn!("Failed to append segment {}: {}", seg_idx, e);
                        } else {
                            fetched_segments.borrow_mut().mark_fetched(seg_idx);
                        }
                    }
                    Err(e) => {
                        log::warn!("Failed to fetch segment {}: {}", seg_idx, e);
                    }
                }
            }
        }

        // Check if we should signal end of stream
        // Only do this when ALL segments have been fetched AND video is near the end
        // We check near-end first to avoid expensive all_buffered check on every iteration
        if !end_of_stream_signaled 
            && current_time > video_duration * 0.9  // Only check when near end (90%+)
            && fetched_segments.borrow().all_fetched() 
        {
            // Verify all segments are actually in the buffer
            let all_buffered = (0..seg_uris.len()).all(|i| is_segment_buffered(video, i));
            
            if all_buffered {
                // Wait for any pending operations
                while sb.updating() {
                    updateend_future(&sb)
                        .await
                        .map_err(|e| format!("waiting for final update: {e:?}"))?;
                }
                
                if let Err(e) = ms.end_of_stream() {
                    log::warn!("end_of_stream failed: {:?}", e);
                } else {
                    end_of_stream_signaled = true;
                }
            }
        }

        // Check if video has ended
        if video.ended() {
            return Ok(());
        }

        // Wait before next check
        TimeoutFuture::new(BUFFER_CHECK_INTERVAL_MS).await;
    }
}

/// Safely remove buffer data, handling errors gracefully.
/// Buffer removal errors are non-fatal - the browser may reject removal for various
/// reasons (invalid range, buffer not appendable, etc.) but playback can continue.
async fn safe_remove_buffer(sb: &SourceBuffer, start: f64, end: f64) -> Result<(), String> {
    // Wait for any pending operations
    while sb.updating() {
        updateend_future(sb)
            .await
            .map_err(|e| format!("waiting for update before remove: {e:?}"))?;
    }

    // Try to remove - this might fail if range is invalid or buffer is in wrong state.
    // This is safe to ignore because:
    // 1. The buffer will eventually be cleaned up by quota management
    // 2. Playback continues normally even with extra buffered data
    // 3. The browser handles memory limits automatically
    if let Err(e) = sb.remove(start, end) {
        log::debug!("Buffer removal skipped (non-fatal): {:?} - range [{:.1}s, {:.1}s]", e, start, end);
        return Ok(());
    }

    // Wait for remove to complete
    while sb.updating() {
        updateend_future(sb)
            .await
            .map_err(|e| format!("waiting for remove to complete: {e:?}"))?;
    }

    Ok(())
}
