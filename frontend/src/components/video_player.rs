use gloo_net::http::Request;
use gloo_timers::callback::Interval;
use gloo_timers::future::TimeoutFuture;
use serde::Deserialize;
use std::cell::{Cell, RefCell};
use std::rc::Rc;
use wasm_bindgen::prelude::*;
use wasm_bindgen::JsCast;
use wasm_bindgen_futures::spawn_local;
use web_sys::{HtmlVideoElement, KeyboardEvent, MouseEvent, window};
use yew::prelude::*;
use log::error;

// ── Playback speed options ───────────────────────────────────────────────────
const PLAYBACK_SPEEDS: [f64; 9] = [0.25, 0.5, 0.75, 1.0, 1.25, 1.5, 1.75, 2.0, 3.0];

const SEGMENT_DURATION_F: f64 = 6.0;

// ── MSE player constants ─────────────────────────────────────────────────────
/// Target seconds of video to keep buffered ahead of the playback position.
const MSE_TARGET_BUFFER_S: f64 = 30.0;
/// Seconds of back-buffer to retain behind the playback position when seeking.
const MSE_BACK_BUFFER_S: f64 = 5.0;

const QUALITY_STORAGE_KEY: &str = "starfin_quality";
const QUALITY_OPTIONS: [(&str, &str); 4] = [
    ("original", "Original (Direct)"),
    ("high",     "High (Transcode)"),
    ("medium",   "Medium (720p)"),
    ("low",      "Low (480p)"),
];

// ── Controls auto-hide timeout (milliseconds of inactivity) ─────────────────
const CONTROL_HIDE_TIMEOUT_MS: f64 = 5000.0;
const CONTROLS_VICINITY_PX: f64 = 80.0;

fn format_time(seconds: f64) -> String {
    if !seconds.is_finite() || seconds < 0.0 {
        return "0:00".to_string();
    }
    let total_secs = seconds as u64;
    let hours = total_secs / 3600;
    let mins  = (total_secs % 3600) / 60;
    let secs  = total_secs % 60;
    if hours > 0 {
        format!("{hours}:{mins:02}:{secs:02}")
    } else {
        format!("{mins}:{secs:02}")
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

// ── Data structures ──────────────────────────────────────────────────────────

#[derive(Properties, PartialEq)]
pub struct VideoPlayerProps {
    pub video_id: String,
    pub title:    String,
    pub on_close: Callback<()>,
}

struct SegmentInfo {
    url: String,
    #[allow(dead_code)]
    duration: f64,
}

struct MseState {
    media_source: web_sys::MediaSource,
    source_buffer: web_sys::SourceBuffer,
    /// Blob URL created for this MediaSource; revoked on cleanup.
    object_url: String,
    /// Parsed segment list from the M3U8 playlist.
    segments: Vec<SegmentInfo>,
    /// URL of the fMP4 init segment (ftyp+moov).
    init_segment_url: Option<String>,
    /// Whether the init segment has been appended.
    init_appended: bool,
    /// Index of the next segment to fetch.
    next_seg: usize,
    /// True while `SourceBuffer.appendBuffer` is in progress.
    is_appending: bool,
}

// ── Helpers ──────────────────────────────────────────────────────────────────

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

/// Resolve a relative segment path against the playlist URL.
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

/// Parse an HLS M3U8 playlist and return the list of segments plus an
/// optional init segment URL (from `#EXT-X-MAP`).
fn parse_m3u8(text: &str, playlist_url: &str) -> (Vec<SegmentInfo>, Option<String>) {
    let mut segments = Vec::new();
    let mut pending_duration: Option<f64> = None;
    let mut init_segment_url: Option<String> = None;

    for line in text.lines() {
        let line = line.trim();

        // Parse #EXT-X-MAP:URI="..." for the init segment.
        if line.starts_with("#EXT-X-MAP:") {
            if let Some(start) = line.find("URI=\"") {
                let rest = &line[start + 5..];
                if let Some(end) = rest.find('"') {
                    init_segment_url = Some(resolve_segment_url(playlist_url, &rest[..end]));
                }
            }
            continue;
        }

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
    (segments, init_segment_url)
}

/// Strip leading `ftyp` and `moov` boxes from an fMP4 segment, returning
/// the byte offset where `moof+mdat` fragment data begins.
fn strip_init_offset(data: &[u8]) -> usize {
    let mut pos: usize = 0;
    while pos + 8 <= data.len() {
        let size = u32::from_be_bytes([
            data[pos], data[pos + 1], data[pos + 2], data[pos + 3],
        ]) as usize;
        if size < 8 || pos + size > data.len() {
            break;
        }
        let box_type = &data[pos + 4..pos + 8];
        if box_type == b"ftyp" || box_type == b"moov" {
            pos += size;
        } else {
            break;
        }
    }
    pos
}

// ── Segment pump ─────────────────────────────────────────────────────────────
/// Pump the next HLS segment into the MSE SourceBuffer.
///
/// Returns immediately if:
/// - `state` is `None` (not yet initialised),
/// - a previous append is still in flight (`is_appending`), or
/// - the forward buffer already exceeds `MSE_TARGET_BUFFER_S`.
///
/// When all segments have been fed, signals end-of-stream on the MediaSource.
fn pump_segments(state: Rc<RefCell<Option<MseState>>>, video: HtmlVideoElement) {
    // --- Phase 0: If the init segment hasn't been appended yet, do that first.
    {
        let mut borrow = state.borrow_mut();
        let mse = match borrow.as_mut() {
            Some(s) => s,
            None => return,
        };
        if mse.is_appending {
            return;
        }
        if !mse.init_appended {
            if let Some(init_url) = mse.init_segment_url.clone() {
                mse.is_appending = true;
                let state_clone = state.clone();
                let video_clone = video.clone();
                drop(borrow); // release borrow before async

                spawn_local(async move {
                    let bytes = match Request::get(&init_url).send().await {
                        Ok(r) => match r.binary().await {
                            Ok(b) => b,
                            Err(e) => {
                                error!("Failed to read init segment: {e:?}");
                                if let Some(s) = state_clone.borrow_mut().as_mut() {
                                    s.is_appending = false;
                                }
                                return;
                            }
                        },
                        Err(e) => {
                            error!("Failed to fetch init segment: {e:?}");
                            if let Some(s) = state_clone.borrow_mut().as_mut() {
                                s.is_appending = false;
                            }
                            return;
                        }
                    };

                    let source_buffer = {
                        let borrow = state_clone.borrow();
                        match borrow.as_ref() {
                            Some(s) => s.source_buffer.clone(),
                            None => return,
                        }
                    };

                    // updateend listener for init segment
                    let state_for_end = state_clone.clone();
                    let video_for_end = video_clone.clone();
                    let updateend_cb = Closure::once(Box::new(move || {
                        {
                            let mut borrow = state_for_end.borrow_mut();
                            if let Some(s) = borrow.as_mut() {
                                s.is_appending = false;
                                s.init_appended = true;
                            }
                        }
                        pump_segments(state_for_end, video_for_end);
                    }) as Box<dyn FnOnce()>);
                    source_buffer
                        .add_event_listener_with_callback(
                            "updateend",
                            updateend_cb.as_ref().unchecked_ref(),
                        )
                        .ok();
                    updateend_cb.forget();

                    let uint8_array = js_sys::Uint8Array::from(bytes.as_slice());
                    let array_buffer = uint8_array.buffer();
                    if source_buffer
                        .append_buffer_with_array_buffer(&array_buffer)
                        .is_err()
                    {
                        if let Some(s) = state_clone.borrow_mut().as_mut() {
                            s.is_appending = false;
                            s.init_appended = true; // skip init if it fails
                        }
                    }
                });
                return;
            } else {
                // No init segment URL — mark as done
                mse.init_appended = true;
            }
        }
    }

    // --- Phase 1: Regular segment pumping
    let seg_url = {
        let mut borrow = state.borrow_mut();
        let mse = match borrow.as_mut() {
            Some(s) => s,
            None => return,
        };
        if mse.is_appending {
            return;
        }
        let buffered_ahead = get_buffer_end(&video) - video.current_time();
        if buffered_ahead >= MSE_TARGET_BUFFER_S && mse.next_seg != 0 {
            return;
        }
        if mse.next_seg >= mse.segments.len() {
            if mse.media_source.ready_state() == web_sys::MediaSourceReadyState::Open {
                let _ = mse.media_source.end_of_stream();
            }
            return;
        }
        let url = mse.segments[mse.next_seg].url.clone();
        mse.is_appending = true;
        url
    };

    let state_clone = state.clone();
    let video_clone = video.clone();
    spawn_local(async move {
        let bytes = match Request::get(&seg_url).send().await {
            Ok(r) => match r.binary().await {
                Ok(b) => b,
                Err(e) => {
                    error!("Failed to read segment bytes: {e:?}");
                    if let Some(s) = state_clone.borrow_mut().as_mut() {
                        s.is_appending = false;
                    }
                    return;
                }
            },
            Err(e) => {
                error!("Failed to fetch segment: {e:?}");
                if let Some(s) = state_clone.borrow_mut().as_mut() {
                    s.is_appending = false;
                }
                return;
            }
        };

        let source_buffer = {
            let borrow = state_clone.borrow();
            match borrow.as_ref() {
                Some(s) => s.source_buffer.clone(),
                None => return,
            }
        };

        // One-shot updateend listener: advance segment pointer and re-pump.
        let state_for_end = state_clone.clone();
        let video_for_end = video_clone.clone();
        let updateend_cb = Closure::once(Box::new(move || {
            {
                let mut borrow = state_for_end.borrow_mut();
                if let Some(s) = borrow.as_mut() {
                    s.is_appending = false;
                    s.next_seg += 1;
                }
            }
            let s = state_for_end;
            let v = video_for_end;
            // Re-pump on next microtask to avoid stack overflow with many segments
            spawn_local(async move {
                pump_segments(s, v);
            });
        }) as Box<dyn FnOnce()>);
        source_buffer
            .add_event_listener_with_callback(
                "updateend",
                updateend_cb.as_ref().unchecked_ref(),
            )
            .ok();
        updateend_cb.forget();

        // Strip the leading ftyp+moov boxes — the init segment was already
        // appended once; including them again resets MSE's decode context.
        let offset = strip_init_offset(&bytes);
        let data_to_append = &bytes[offset..];

        let uint8_array = js_sys::Uint8Array::from(data_to_append);
        let array_buffer = uint8_array.buffer();
        if source_buffer
            .append_buffer_with_array_buffer(&array_buffer)
            .is_err()
        {
            if let Some(s) = state_clone.borrow_mut().as_mut() {
                s.is_appending = false;
            }
        }
    });
}

// ── Component ────────────────────────────────────────────────────────────────

#[function_component(VideoPlayer)]
pub fn video_player(props: &VideoPlayerProps) -> Html {

    let video_player_ref      = use_node_ref();
    let progress_ref          = use_node_ref();
    let container_ref         = use_node_ref();
    let thumbnail_canvas_ref  = use_node_ref();

    // Player state
    let current_time          = use_state(|| 0.0_f64);
    let duration              = use_state(|| 0.0_f64);
    let is_playing            = use_state(|| false);
    let is_buffering          = use_state(|| false);

    // Volume state
    let volume                = use_state(|| 1.0_f64);
    let is_muted              = use_state(|| false);
    let prev_volume           = use_state(|| 1.0_f64);

    // Drag/Seek state
    let is_dragging           = use_state(|| false);
    let drag_time             = use_state(|| 0.0_f64);
    let just_dragged          = use_state(|| false);

    // Hover preview state
    let is_hovering_progress  = use_state(|| false);
    let hover_time            = use_state(|| 0.0_f64);
    let hover_position        = use_state(|| 0.0_f64);

    // UI visibility state
    let controls_visible      = use_state(|| true);
    let last_mouse_move       = use_mut_ref(|| js_sys::Date::now());
    let is_near_controls      = use_mut_ref(|| false);
    let speed_menu_open       = use_state(|| false);
    let quality_menu_open     = use_state(|| false);
    let volume_slider_visible = use_state(|| false);

    // Fullscreen state
    let is_fullscreen         = use_state(|| false);

    // Playback speed
    let playback_speed        = use_state(|| 1.0_f64);

    // Stream quality
    let initial_quality = window()
        .and_then(|w| w.local_storage().ok())
        .flatten()
        .and_then(|s| s.get_item(QUALITY_STORAGE_KEY).ok())
        .flatten()
        .filter(|q| QUALITY_OPTIONS.iter().any(|(v, _)| v == q))
        .unwrap_or_else(|| "original".to_string());
    let selected_quality = use_state(|| initial_quality);

    // Skip indicator state
    let skip_indicator        = use_state(|| Option::<(String, f64)>::None);

    // Video ended state
    let video_ended           = use_state(|| false);

    // Thumbnail sprite state
    let thumbnail_info        = use_state(|| Option::<ThumbnailInfo>::None);
    let thumbnail_image       = use_state(|| Option::<web_sys::HtmlImageElement>::None);

    // MSE state — stored in use_mut_ref (Rc<RefCell<…>>) rather than
    // use_state, because the MSE state is set asynchronously inside a
    // spawn_local block. use_mut_ref wraps a single Rc<RefCell<T>> shared
    // by all clones, so writes made by the async task are immediately
    // visible to the cleanup closure.
    let mse_state = use_mut_ref(|| Option::<MseState>::None);

    // Buffered end state for UI display
    let buffered_end = use_state(|| 0.0_f64);

    let volume_icon: Html = if *is_muted || *volume == 0.0 {
        icon_volume_muted()
    } else if *volume < 0.5 {
        icon_volume_low()
    } else {
        icon_volume_high()
    };

    let time_display = format!(
        "{} / {}",
        format_time(*current_time),
        format_time(*duration)
    );

    let container_class = if *is_fullscreen {
        "player-overlay player-overlay--fullscreen"
    } else {
        "player-overlay"
    };

    // ── Close menus when clicking outside ────────────────────────────
    let on_container_click = {
        let speed_menu_open = speed_menu_open.clone();
        let quality_menu_open = quality_menu_open.clone();
        Callback::from(move |_: MouseEvent| {
            speed_menu_open.set(false);
            quality_menu_open.set(false);
        })
    };

    // ── Mouse move handler — show controls and update vicinity flag ──
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
                let dist_from_bottom = (rect.bottom() - mouse_y).max(0.0);
                let dist_from_top = (mouse_y - rect.top()).max(0.0);
                *is_near_controls.borrow_mut() =
                    dist_from_bottom < CONTROLS_VICINITY_PX || dist_from_top < CONTROLS_VICINITY_PX;
            }
        })
    };

    let on_mouse_leave = {
        let is_near_controls = is_near_controls.clone();
        Callback::from(move |_: MouseEvent| {
            *is_near_controls.borrow_mut() = false;
        })
    };

    let on_play_pause = {
        let video_ref = video_player_ref.clone();
        let video_ended = video_ended.clone();
        Callback::from(move |e: MouseEvent| {
            e.stop_propagation();
            if let Some(v) = video_ref.cast::<HtmlVideoElement>() {
                if *video_ended {
                    v.set_current_time(0.0);
                }
                if v.paused() {
                    let _ = v.play();
                } else {
                    let _ = v.pause();
                }
            }
        })
    };

    // ── Speed menu ───────────────────────────────────────────────
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
        let playback_speed = playback_speed.clone();
        let speed_menu_open = speed_menu_open.clone();
        let video_ref = video_player_ref.clone();
        Callback::from(move |speed: f64| {
            playback_speed.set(speed);
            speed_menu_open.set(false);
            if let Some(v) = video_ref.cast::<HtmlVideoElement>() {
                v.set_playback_rate(speed);
            }
        })
    };

    // ── Quality menu ─────────────────────────────────────────────
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
        Callback::from(move |quality: String| {
            quality_menu_open.set(false);
            // Persist preference.
            if let Some(storage) = window()
                .and_then(|w| w.local_storage().ok())
                .flatten()
            {
                let _ = storage.set_item(QUALITY_STORAGE_KEY, &quality);
            }
            selected_quality.set(quality);
            // Quality change takes effect on next video load.
        })
    };

    // ── Fullscreen ───────────────────────────────────────────────
    let on_fullscreen_toggle = {
        let container_ref = container_ref.clone();
        let is_fullscreen = is_fullscreen.clone();
        Callback::from(move |e: MouseEvent| {
            e.stop_propagation();
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

    // ── Volume / mute ────────────────────────────────────────────
    let on_mute_toggle = {
        let video_ref = video_player_ref.clone();
        let is_muted = is_muted.clone();
        let volume = volume.clone();
        let prev_volume = prev_volume.clone();
        Callback::from(move |e: MouseEvent| {
            e.stop_propagation();
            if let Some(v) = video_ref.cast::<HtmlVideoElement>() {
                if *is_muted {
                    is_muted.set(false);
                    v.set_muted(false);
                    volume.set(*prev_volume);
                    v.set_volume(*prev_volume);
                } else {
                    prev_volume.set(*volume);
                    is_muted.set(true);
                    v.set_muted(true);
                }
            }
        })
    };

    let on_volume_change = {
        let video_ref = video_player_ref.clone();
        let volume = volume.clone();
        let is_muted = is_muted.clone();
        Callback::from(move |e: web_sys::InputEvent| {
            if let Some(input) = e.target_dyn_into::<web_sys::HtmlInputElement>() {
                if let Ok(val) = input.value().parse::<f64>() {
                    volume.set(val);
                    if let Some(v) = video_ref.cast::<HtmlVideoElement>() {
                        v.set_volume(val);
                        if val > 0.0 {
                            v.set_muted(false);
                            is_muted.set(false);
                        }
                    }
                }
            }
        })
    };

    // ── Video click → play/pause ─────────────────────────────────
    let on_video_click = {
        Callback::from(move |e: MouseEvent| {
            e.stop_propagation();
            if let Some(v) = e.target_dyn_into::<HtmlVideoElement>() {
                if v.paused() {
                    let _ = v.play();
                } else {
                    let _ = v.pause();
                }
            }
        })
    };

    // ── Replay ───────────────────────────────────────────────────
    let on_replay = {
        let video_ref = video_player_ref.clone();
        let video_ended = video_ended.clone();
        Callback::from(move |_: MouseEvent| {
            if let Some(v) = video_ref.cast::<HtmlVideoElement>() {
                v.set_current_time(0.0);
                let _ = v.play();
                video_ended.set(false);
            }
        })
    };

    // ── Thumbnail sprite loading ─────────────────────────────────
    {
        let video_id = props.video_id.clone();
        let thumbnail_info = thumbnail_info.clone();
        let thumbnail_image = thumbnail_image.clone();

        use_effect_with(video_id.clone(), move |_| {
            let thumbnail_info_clone = thumbnail_info.clone();
            let thumbnail_image_clone = thumbnail_image.clone();
            let video_id_clone = video_id.clone();

            spawn_local(async move {
                if let Ok(info) = fetch_thumbnail_info(&video_id_clone).await {
                    // Load the sprite image
                    let img = web_sys::HtmlImageElement::new().unwrap();
                    let img_clone = img.clone();
                    let thumbnail_image_onload = thumbnail_image_clone.clone();
                    let onload = Closure::once(move || {
                        thumbnail_image_onload.set(Some(img_clone));
                    });
                    img.set_onload(Some(onload.as_ref().unchecked_ref()));
                    onload.forget();
                    img.set_src(&info.url);
                    thumbnail_info_clone.set(Some(info));
                }
            });

            || ()
        });
    }

    // ── Effect to draw thumbnail on canvas when hovering ─────────
    {
        let thumbnail_canvas_ref = thumbnail_canvas_ref.clone();
        let thumbnail_info = thumbnail_info.clone();
        let thumbnail_image = thumbnail_image.clone();
        let hover_time = hover_time.clone();
        let is_hovering_progress = is_hovering_progress.clone();
        let is_dragging = is_dragging.clone();
        use_effect_with(
            (*hover_time, *is_hovering_progress, *is_dragging),
            move |_| {
                if !*is_hovering_progress && !*is_dragging {
                    return;
                }
                if let (Some(info), Some(img)) = (&*thumbnail_info, &*thumbnail_image) {
                    if let Some(canvas) = thumbnail_canvas_ref.cast::<web_sys::HtmlCanvasElement>() {
                        if let Ok(Some(ctx)) = canvas.get_context("2d") {
                            if let Ok(ctx) = ctx.dyn_into::<web_sys::CanvasRenderingContext2d>() {
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

    // ── Progress bar interaction handlers ─────────────────────────
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
        let video_ref = video_player_ref.clone();
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

    // Click on the progress bar (for non-drag single clicks).
    let on_progress_click = {
        let video_ref = video_player_ref.clone();
        let progress_ref = progress_ref.clone();
        let duration = duration.clone();
        let just_dragged = just_dragged.clone();
        Callback::from(move |e: MouseEvent| {
            e.stop_propagation();
            // Skip the click if we just finished dragging.
            if *just_dragged {
                just_dragged.set(false);
                return;
            }
            if *duration <= 0.0 { return; }
            if let Some(el) = progress_ref.cast::<web_sys::HtmlElement>() {
                let rect = el.get_bounding_client_rect();
                let x = e.client_x() as f64 - rect.left();
                let pct = (x / rect.width()).clamp(0.0, 1.0);
                let seek_to = pct * *duration;
                if let Some(v) = video_ref.cast::<HtmlVideoElement>() {
                    v.set_current_time(seek_to);
                }
            }
        })
    };

    let fullscreen_icon: Html = if *is_fullscreen {
        icon_fullscreen_exit()
    } else {
        icon_fullscreen_enter()
    };

    let title = props.title.clone();

    // ── Keyboard hotkeys ─────────────────────────────────────────
    {
        let video_ref = video_player_ref.clone();
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
                    " " | "k" | "K" => {
                        e.prevent_default();
                        if video.paused() {
                            let _ = video.play();
                        } else {
                            let _ = video.pause();
                        }
                    }
                    "ArrowLeft" => {
                        e.prevent_default();
                        let skip = if e.shift_key() { 10.0 } else { 5.0 };
                        let current = video.current_time();
                        video.set_current_time((current - skip).max(0.0));
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
                        video.set_current_time((current - 10.0).max(0.0));
                        skip_indicator.set(Some(("backward".to_string(), 25.0)));
                        let skip_indicator_clone = skip_indicator.clone();
                        spawn_local(async move {
                            TimeoutFuture::new(500).await;
                            skip_indicator_clone.set(None);
                        });
                    }
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
                    "ArrowDown" => {
                        e.prevent_default();
                        let new_vol = (*volume - 0.1).max(0.0);
                        volume.set(new_vol);
                        video.set_volume(new_vol);
                    }
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
                    "0" | "1" | "2" | "3" | "4" | "5" | "6" | "7" | "8" | "9" => {
                        e.prevent_default();
                        let num: f64 = key.parse().unwrap_or(0.0);
                        let dur = video.duration();
                        if dur.is_finite() {
                            video.set_current_time(dur * (num / 10.0));
                        }
                    }
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
                    "Home" => {
                        e.prevent_default();
                        video.set_current_time(0.0);
                    }
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
            }
        });
    }

    // ── Controls auto-hide ───────────────────────────────────────
    {
        let is_playing = is_playing.clone();
        let quality_menu_open = quality_menu_open.clone();
        let is_near_controls = is_near_controls.clone();
        let controls_visible = controls_visible.clone();
        let last_mouse_move = last_mouse_move.clone();

        use_effect_with(
            ((*is_playing).clone(), (*quality_menu_open).clone()),
            move |_| {
                use gloo_timers::callback::Interval;

                let is_playing = is_playing.clone();
                let quality_menu_open = quality_menu_open.clone();
                let is_near_controls = is_near_controls.clone();
                let controls_visible = controls_visible.clone();
                let last_mouse_move = last_mouse_move.clone();

                let interval = Interval::new(1000, move || {
                    if *is_playing && !*quality_menu_open && !*is_near_controls.borrow() {
                        let elapsed = js_sys::Date::now() - *last_mouse_move.borrow();
                        if elapsed > CONTROL_HIDE_TIMEOUT_MS {
                            controls_visible.set(false);
                        }
                    }
                });

                move || drop(interval)
            },
        );
    }

    // ──────────────────────────────────────────────────────────────
    // MSE Effect: Initialize MSE player (re-initialise on video ID or quality change)
    // ──────────────────────────────────────────────────────────────
    {
        let video_ref = video_player_ref.clone();
        let mse_state = mse_state.clone();
        let selected_quality = selected_quality.clone();

        use_effect_with(
            (props.video_id.clone(), (*selected_quality).clone()),
            move |(video_id, quality)| {
                let video_id = video_id.clone();
                let quality = quality.clone();
                let video_ref_clone = video_ref.clone();
                let mse_state_clone = mse_state.clone();

                spawn_local(async move {
                    // Give time for video element to be created
                    TimeoutFuture::new(50).await;

                    let video = match video_ref_clone.cast::<HtmlVideoElement>() {
                        Some(v) => v,
                        None => {
                            error!("Video element not found");
                            return;
                        }
                    };

                    let playlist_url = format!(
                        "/api/videos/{}/playlist.m3u8?quality={}",
                        video_id, quality
                    );

                    // Check if the browser has native HLS support (Safari)
                    if !video
                        .can_play_type("application/vnd.apple.mpegurl")
                        .is_empty()
                    {
                        video.set_src(&playlist_url);
                        let _ = video.play();
                        return;
                    }

                    // Create a MediaSource
                    let media_source = match web_sys::MediaSource::new() {
                        Ok(ms) => ms,
                        Err(_) => {
                            error!("Browser does not support Media Source Extensions");
                            return;
                        }
                    };

                    // Attach the MediaSource to the video element via a blob URL.
                    let object_url = match web_sys::Url::create_object_url_with_source(&media_source) {
                        Ok(u) => u,
                        Err(_) => {
                            error!("Failed to create MediaSource URL");
                            return;
                        }
                    };
                    video.set_src(&object_url);

                    // All SourceBuffer setup must happen inside the sourceopen callback.
                    let playlist_url_for_open = playlist_url.clone();
                    let video_for_open = video.clone();
                    let mse_state_for_open = mse_state_clone.clone();
                    let media_source_for_open = media_source.clone();
                    let object_url_for_open = object_url.clone();

                    let sourceopen_cb = Closure::once(Box::new(move || {
                        let playlist_url = playlist_url_for_open;
                        let video = video_for_open;
                        let mse_state = mse_state_for_open;
                        let media_source = media_source_for_open;
                        let object_url = object_url_for_open;

                        spawn_local(async move {
                            // Fetch the M3U8 playlist.
                            let resp = match Request::get(&playlist_url).send().await {
                                Ok(r) => r,
                                Err(e) => {
                                    error!("Failed to fetch playlist: {e:?}");
                                    return;
                                }
                            };
                            let text = match resp.text().await {
                                Ok(t) => t,
                                Err(e) => {
                                    error!("Failed to read playlist: {e:?}");
                                    return;
                                }
                            };

                            // Parse segment list.
                            let (segments, init_segment_url) = parse_m3u8(&text, &playlist_url);
                            if segments.is_empty() {
                                error!("Playlist contains no segments");
                                return;
                            }

                            // Create the SourceBuffer with fMP4 MIME type.
                            let mime = "video/mp4; codecs=\"avc1.42E01E,mp4a.40.2\"";
                            let source_buffer = match media_source.add_source_buffer(mime) {
                                Ok(sb) => sb,
                                Err(e) => {
                                    error!("add_source_buffer failed: {:?}", e);
                                    return;
                                }
                            };

                            // Store MSE state.
                            *mse_state.borrow_mut() = Some(MseState {
                                media_source,
                                source_buffer,
                                object_url,
                                segments,
                                init_segment_url,
                                init_appended: false,
                                next_seg: 0,
                                is_appending: false,
                            });

                            // Kick off the segment pump.
                            pump_segments(mse_state, video);
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

                // Cleanup: tear down old MSE state when deps change or component unmounts.
                let mse_state_for_cleanup = mse_state.clone();
                let video_ref_for_cleanup = video_ref.clone();
                move || {
                    if let Some(state) = mse_state_for_cleanup.borrow_mut().take() {
                        let _ = state.media_source.end_of_stream();
                        let _ = web_sys::Url::revoke_object_url(&state.object_url);
                        if let Some(video) = video_ref_for_cleanup.cast::<HtmlVideoElement>() {
                            video.set_src("");
                        }
                    }
                }
            },
        );
    }

    // ──────────────────────────────────────────────────────────────
    // Effect: periodic UI update + MSE pump top-up
    // ──────────────────────────────────────────────────────────────
    {
        let video_ref    = video_player_ref.clone();
        let current_time = current_time.clone();
        let duration     = duration.clone();
        let is_playing   = is_playing.clone();
        let volume       = volume.clone();
        let video_ended  = video_ended.clone();
        let is_dragging  = is_dragging.clone();
        let mse_state    = mse_state.clone();
        let buffered_end = buffered_end.clone();

        use_effect_with(video_ref.clone(), move |video_ref| {
            let video_ref = video_ref.clone();
            let mse_state_for_interval = mse_state.clone();

            let interval = Interval::new(150, move || {
                if let Some(v) = video_ref.cast::<HtmlVideoElement>() {
                    // Don't update current_time while dragging (user is controlling it)
                    if !*is_dragging {
                        let ct = v.current_time();
                        if (ct - *current_time).abs() > 0.25 { current_time.set(ct); }
                    }

                    // duration may change once MSE has enough data
                    let dur = v.duration();
                    if dur.is_finite() && dur > 0.0 {
                        if (dur - *duration).abs() > 0.5 { duration.set(dur); }
                    }

                    // Update buffered end for progress bar display
                    let buf_end = get_buffer_end(&v);
                    if (buf_end - *buffered_end).abs() > 0.5 { buffered_end.set(buf_end); }

                    let playing = !v.paused() && !v.ended();
                    if playing != *is_playing { is_playing.set(playing); }

                    // Track video ended state
                    let ended = v.ended();
                    if ended != *video_ended { video_ended.set(ended); }

                    // Sync volume state on first tick.
                    let vol = v.volume();
                    if (*volume - vol).abs() > 0.01 { volume.set(vol); }

                    // Top up the MSE buffer as playback advances
                    pump_segments(mse_state_for_interval.clone(), v);
                }
            });

            move || drop(interval)
        });
    }

    // ──────────────────────────────────────────────────────────────
    // Effect: Handle seeks to unbuffered positions for the MSE player
    // ──────────────────────────────────────────────────────────────
    {
        let video_ref = video_player_ref.clone();
        let mse_state = mse_state.clone();

        use_effect_with(video_ref.clone(), move |video_ref| {
            let video_opt = video_ref.cast::<HtmlVideoElement>();

            let seeked_cb = video_opt.as_ref().map(|video| {
                let mse_state_for_seeked = mse_state.clone();
                let video_for_seeked = video.clone();

                let cb = Closure::<dyn Fn()>::new(move || {
                    let current_time = video_for_seeked.current_time();
                    let buf_end = get_buffer_end(&video_for_seeked);

                    // If the seek target is already buffered, just re-pump.
                    if buf_end > current_time {
                        pump_segments(mse_state_for_seeked.clone(), video_for_seeked.clone());
                        return;
                    }

                    // Seeking to unbuffered territory — reset segment pointer and re-pump.
                    {
                        let mut borrow = mse_state_for_seeked.borrow_mut();
                        if let Some(mse) = borrow.as_mut() {
                            if !mse.source_buffer.updating() {
                                let remove_end = (current_time - MSE_BACK_BUFFER_S).max(0.0);
                                if remove_end > 0.0 {
                                    let _ = mse.source_buffer.remove(0.0, remove_end);
                                }
                            }
                            mse.next_seg = (current_time / SEGMENT_DURATION_F) as usize;
                            mse.is_appending = false;
                        }
                    }
                    pump_segments(mse_state_for_seeked.clone(), video_for_seeked.clone());
                });

                video
                    .add_event_listener_with_callback("seeked", cb.as_ref().unchecked_ref())
                    .ok();
                cb
            });

            move || {
                if let (Some(cb), Some(video)) = (&seeked_cb, video_opt) {
                    let _ = video.remove_event_listener_with_callback(
                        "seeked",
                        cb.as_ref().unchecked_ref(),
                    );
                }
            }
        });
    }

    // ── Compute progress bar percentages ─────────────────────────
    let progress_percent = if *duration > 0.0 {
        if *is_dragging {
            (*drag_time / *duration * 100.0).min(100.0)
        } else {
            (*current_time / *duration * 100.0).min(100.0)
        }
    } else {
        0.0
    };

    let buffered_percent = if *duration > 0.0 {
        (*buffered_end / *duration * 100.0).min(100.0)
    } else {
        0.0
    };

    let controls_class = if *controls_visible {
        "player-controls"
    } else {
        "player-controls player-controls--hidden"
    };

    let header_class = if *controls_visible {
        "player-header"
    } else {
        "player-header player-header--hidden"
    };

    // Play/pause icon
    let play_pause_icon: Html = if *video_ended {
        icon_replay()
    } else if *is_playing {
        icon_pause()
    } else {
        icon_play()
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

    let on_close = props.on_close.clone();

    html! {
        <div
            ref={container_ref}
            class={container_class}
            onclick={on_container_click}
            onmousemove={on_mouse_move}
            onmouseleave={on_mouse_leave}
        >
            // Header
            <div class={header_class}>
                <button
                    class="btn btn--back"
                    onclick={Callback::from(move |_| {
                        on_close.emit(());
                    })}
                >
                    { icon_arrow_back() }
                    { " Back" }
                </button>
                <span class="player-title">{ title }</span>
            </div>

            // Buffering indicator
            if *is_buffering {
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
                ref={video_player_ref}
                class="video-el"
                onclick={on_video_click}
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
                            <button class="player-controls__btn" onclick={on_mute_toggle} title="Mute (m)">
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
                                    .unwrap_or("Original (Direct)") }
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
