use gloo_net::http::Request;
use std::collections::VecDeque;
use std::cell::{Cell, RefCell};
use gloo_net::Error as GlooError;
use gloo_timers::future::TimeoutFuture;
use serde::Deserialize;
use std::rc::Rc;
use wasm_bindgen::prelude::*;
use wasm_bindgen::JsCast;
use wasm_bindgen_futures::spawn_local;
use wasm_bindgen_futures::JsFuture;
use web_sys::{HtmlVideoElement, KeyboardEvent, MediaSource, MouseEvent, Url, window, SourceBuffer};
use yew::prelude::*;
use log::{info, warn, error};

// ── Playback speed options ───────────────────────────────────────────────────
const PLAYBACK_SPEEDS: [f64; 9] = [0.25, 0.5, 0.75, 1.0, 1.25, 1.5, 1.75, 2.0, 3.0];

const SEGMENT_DURATION_F: f64 = 6.0;

/// How many segments ahead of the *playhead* the pump is allowed to buffer.
const LOOKAHEAD: usize = 3;

/// Seconds of already-played data to keep behind the playhead.
const KEEP_BEHIND_SECS: f64 = 10.0;

/// Poll interval (ms) when the pump is waiting for the playhead to advance.
const PLAYBACK_POLL_MS: u32 = 250;

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

#[derive(Debug, Clone)]
pub struct Segment {
    pub url:               String,
    pub representation_id: String,
    pub start_time:        f64,
    pub duration:          f64,
    pub data:              Option<Vec<u8>>,
    pub seq:               Option<u64>,
}

impl Segment {
    pub async fn fetch(&mut self) -> Result<(), GlooError> {
        if self.data.is_some() {
            return Ok(());
        }
        let bytes = Request::get(&self.url)
            .send()
            .await?
            .binary()
            .await?;
        self.data = Some(bytes);
        Ok(())
    }

    pub fn append_to(&mut self, source_buffer: &SourceBuffer) -> Result<(), JsValue> {
        let data = self.data.as_mut().expect("call .fetch() first");
        source_buffer.append_buffer_with_u8_array(data.as_mut_slice())
    }
}

#[derive(Debug, Clone)]
pub struct Representation {
    pub id:       String,
    pub bitrate:  u32,
    pub segments: Vec<Segment>,
}

#[derive(Debug, Clone)]
pub struct Playlist {
    pub representations:  Vec<Representation>,
    pub is_live:          bool,
    pub total_duration:   Option<f64>,
    pub init_segment_url: Option<String>,
}

pub struct VideoPlayerState {
    pub segment_queue:          VecDeque<Segment>,
    pub media_source:           MediaSource,
    pub source_buffer:          Option<SourceBuffer>,
    pub video_element:          NodeRef,
    pub current_representation: String,
    pub playlist:               Option<Playlist>,
}

impl VideoPlayerState {
    pub fn new(video_ref: NodeRef) -> Result<Self, JsValue> {
        let media_source = MediaSource::new()?;
        Ok(Self {
            segment_queue:          VecDeque::with_capacity(32),
            media_source,
            source_buffer:          None,
            video_element:          video_ref,
            current_representation: String::new(),
            playlist:               None,
        })
    }

    pub async fn load_playlist(
        &mut self,
        video_id: String,
        quality:  String,
    ) -> Result<(), JsValue> {
        let playlist_url = format!(
            "/api/videos/{}/playlist.m3u8?quality={}",
            video_id, quality
        );
        let text = Request::get(&playlist_url)
            .send()
            .await
            .map_err(|e| JsValue::from_str(&e.to_string()))?
            .text()
            .await
            .map_err(|e| JsValue::from_str(&e.to_string()))?;

        let parsed = parse_media_playlist(&text, &playlist_url, &quality)
            .map_err(|e| JsValue::from_str(&e))?;

        info!("{:?}", parsed);
        self.playlist               = Some(parsed.clone());
        self.current_representation = quality;

        if let Some(rep) = parsed.representations.first() {
            for seg in &rep.segments {
                self.segment_queue.push_back(seg.clone());
            }
        }

        Ok(())
    }
}

// ── Playlist parser ──────────────────────────────────────────────────────────

pub fn parse_media_playlist(
    text:              &str,
    _playlist_base_url: &str,
    quality:           &str,
) -> Result<Playlist, String> {
    let lines: Vec<&str> = text
        .lines()
        .map(|l| l.trim())
        .filter(|l| !l.is_empty())
        .collect();

    let mut segments         = Vec::new();
    let mut current_time     = 0.0f64;
    let mut init_segment_url = None;
    let mut i                = 0usize;

    while i < lines.len() {
        let line = lines[i];

        // Parse #EXT-X-MAP:URI="..." for the init segment.
        if line.starts_with("#EXT-X-MAP:") {
            if let Some(start) = line.find("URI=\"") {
                let rest = &line[start + 5..];
                if let Some(end) = rest.find('"') {
                    init_segment_url = Some(rest[..end].to_string());
                    info!("playlist: found init segment URL: {}", rest[..end].to_string());
                }
            }
        }

        if line.starts_with("#EXTINF:")
            && let Some(duration_str) = line.split(',').next()
        {
            let duration_str = duration_str.trim_start_matches("#EXTINF:");
            if let Ok(duration) = duration_str.parse::<f64>() {
                i += 1;
                if i < lines.len() && !lines[i].starts_with('#') {
                    segments.push(Segment {
                        url:               lines[i].to_string(),
                        representation_id: quality.to_string(),
                        start_time:        current_time,
                        duration,
                        data:              None,
                        seq:               Some(segments.len() as u64),
                    });
                    current_time += duration;
                }
            }
        }
        i += 1;
    }

    info!("playlist: {} segments parsed", segments.len());

    let rep            = Representation { id: quality.to_string(), bitrate: 0, segments };
    let total_duration = Some(rep.segments.iter().map(|s| s.duration).sum());

    Ok(Playlist {
        representations:  vec![rep],
        is_live:          false,
        total_duration,
        init_segment_url,
    })
}

// ── Segment pump helpers ─────────────────────────────────────────────────────

/// Strip leading `ftyp` and `moov` boxes from an fMP4 segment, returning
/// the byte offset where `moof+mdat` fragment data begins.
///
/// Each segment produced by ffmpeg with `empty_moov` contains
/// `[ftyp][moov][moof][mdat]`.  The init segment (ftyp+moov) is appended
/// once to the SourceBuffer; including it again in every media segment
/// causes MSE to re-initialize the decode context and the buffer range
/// never extends past the first segment's duration.
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
            pos += size;            // skip this box
        } else {
            break;                  // reached moof/mdat — return from here
        }
    }

    pos
}

/// Wait until sb.updating is false, yielding to the JS event loop each tick.
async fn wait_until_not_updating(sb_ref: &Rc<RefCell<Option<SourceBuffer>>>) {
    loop {
        let updating = sb_ref
            .borrow()
            .as_ref()
            .map(|sb| sb.updating())
            .unwrap_or(false);
        if !updating { return; }
        TimeoutFuture::new(0).await;
    }
}

/// Return the segment index that the playhead currently falls inside.
fn playhead_segment_index(video: &HtmlVideoElement, start_times: &[(f64, f64)]) -> usize {
    let ct = video.current_time();
    start_times
        .iter()
        .position(|(start, dur)| ct < start + dur)
        .unwrap_or(start_times.len().saturating_sub(1))
}

/// Log the current buffered ranges for debugging.
fn log_buffered_ranges(video: &HtmlVideoElement) {
    let buffered = video.buffered();
    let len = buffered.length();
    if len == 0 {
        info!("  buffered: (empty)");
        return;
    }
    for i in 0..len {
        if let (Ok(s), Ok(e)) = (buffered.start(i), buffered.end(i)) {
            info!("  buffered[{}]: {:.2}s – {:.2}s ({:.2}s)", i, s, e, e - s);
        }
    }
}

/// Try to start playback.  First attempts unmuted; if the browser rejects
/// (autoplay policy), retries muted.  Returns true if playing.
async fn try_play(video: &HtmlVideoElement) -> bool {
    // Attempt 1: play with current audio state
    let promise = video.play();
    match promise {
        Ok(p) => {
            match JsFuture::from(p).await {
                Ok(_) => {
                    info!("pump: play() succeeded");
                    return true;
                }
                Err(e) => {
                    warn!("pump: play() rejected: {:?} — retrying muted", e);
                }
            }
        }
        Err(e) => {
            warn!("pump: play() threw: {:?} — retrying muted", e);
        }
    }

    // Attempt 2: mute and try again
    video.set_muted(true);
    let promise = video.play();
    match promise {
        Ok(p) => {
            match JsFuture::from(p).await {
                Ok(_) => {
                    info!("pump: play() succeeded (muted)");
                    return true;
                }
                Err(e) => {
                    error!("pump: play() rejected even muted: {:?}", e);
                }
            }
        }
        Err(e) => {
            error!("pump: play() threw even muted: {:?}", e);
        }
    }

    false
}

/// Block until the playhead is within LOOKAHEAD segments of append_cursor.
///
/// Also handles paused/stalled states by allowing a small extra buffer so
/// the pump doesn't deadlock.
async fn wait_for_playhead(
    video_ref:     &NodeRef,
    start_times:   &[(f64, f64)],
    append_cursor: usize,
) {
    if append_cursor < LOOKAHEAD {
        info!("pump gate: cursor={} < LOOKAHEAD={}, allowing immediately", append_cursor, LOOKAHEAD);
        return;
    }

    let mut logged_waiting = false;
    let mut poll_count: u32 = 0;

    loop {
        let (ready, debug_info) = {
            if let Some(video) = video_ref.cast::<HtmlVideoElement>() {
                let ct     = video.current_time();
                let paused = video.paused();
                let ended  = video.ended();
                let ph     = playhead_segment_index(&video, start_times);
                let rs     = video.ready_state();

                let debug = format!(
                    "ct={:.2}s ph_seg={} cursor={} paused={} ended={} readyState={}",
                    ct, ph, append_cursor, paused, ended, rs
                );

                // Allow appending if we're within the lookahead window
                if append_cursor < ph + LOOKAHEAD {
                    (true, debug)
                }
                // If paused/ended/stalled, allow one extra segment so there's
                // data ready when the user resumes.
                else if paused || ended || rs < 3 {
                    let allow = append_cursor < ph + LOOKAHEAD + 2;
                    (allow, debug)
                } else {
                    (false, debug)
                }
            } else {
                (true, "no video element".to_string())
            }
        };

        if ready {
            if logged_waiting {
                info!("pump gate: RESUMING — {}", debug_info);
            }
            return;
        }

        if !logged_waiting {
            info!("pump gate: WAITING — {}", debug_info);
            // Also log buffered ranges on first wait
            if let Some(video) = video_ref.cast::<HtmlVideoElement>() {
                log_buffered_ranges(&video);
            }
            logged_waiting = true;
        }

        // Periodic status while waiting (every ~2 seconds)
        poll_count += 1;
        if poll_count % 8 == 0 {
            info!("pump gate: still waiting — {}", debug_info);
            if let Some(video) = video_ref.cast::<HtmlVideoElement>() {
                log_buffered_ranges(&video);
            }
        }

        TimeoutFuture::new(PLAYBACK_POLL_MS).await;
    }
}

/// Remove buffered data that is safely behind the playhead.
async fn evict_behind_playhead(
    video_ref: &NodeRef,
    sb_ref:    &Rc<RefCell<Option<SourceBuffer>>>,
) {
    let evict_end = {
        if let Some(video) = video_ref.cast::<HtmlVideoElement>() {
            let target = video.current_time() - KEEP_BEHIND_SECS;
            if target <= 0.5 { return; }
            target
        } else {
            return;
        }
    };

    let has_data_to_remove = {
        let sb_guard = sb_ref.borrow();
        sb_guard.as_ref()
            .and_then(|sb| sb.buffered().ok())
            .filter(|r| r.length() > 0)
            .and_then(|r| r.start(0).ok())
            .map(|buf_start| buf_start < evict_end)
            .unwrap_or(false)
    };

    if !has_data_to_remove { return; }

    let remove_ok = {
        let sb_guard = sb_ref.borrow();
        match sb_guard.as_ref() {
            None => false,
            Some(sb) => match sb.remove(0.0, evict_end) {
                Ok(())  => true,
                Err(e)  => { error!("pump: remove failed: {:?}", e); false }
            }
        }
    };

    if remove_ok {
        wait_until_not_updating(sb_ref).await;
        info!("pump: evicted up to {:.2}s", evict_end);
    }
}

// ── Segment pump ─────────────────────────────────────────────────────────────

async fn run_segment_pump(
    segments:         Vec<Segment>,
    sb_ref:           Rc<RefCell<Option<SourceBuffer>>>,
    video_ref:        NodeRef,
    media_source:     Rc<MediaSource>,
    init_segment_url: Option<String>,
) {
    let total = segments.len();
    info!("pump: {} segments total", total);

    // ── 0. Append init segment (ftyp+moov) ─────────────────────────────
    if let Some(ref init_url) = init_segment_url {
        info!("pump: fetching init segment from {}", init_url);
        match Request::get(init_url).send().await {
            Ok(resp) => match resp.binary().await {
                Ok(mut bytes) => {
                    info!("pump: init segment fetched ({} bytes)", bytes.len());
                    wait_until_not_updating(&sb_ref).await;
                    let append_ok = {
                        let sb_guard = sb_ref.borrow();
                        match sb_guard.as_ref() {
                            None => { error!("pump: SourceBuffer gone before init"); false }
                            Some(sb) => {
                                match sb.append_buffer_with_u8_array(bytes.as_mut_slice()) {
                                    Ok(()) => { info!("pump: init segment appended OK"); true }
                                    Err(e) => { error!("pump: init append failed: {:?}", e); false }
                                }
                            }
                        }
                    };
                    if append_ok {
                        wait_until_not_updating(&sb_ref).await;
                        info!("pump: init segment append complete");
                    }
                }
                Err(e) => error!("pump: init segment binary() failed: {e}"),
            },
            Err(e) => error!("pump: init segment fetch failed: {e}"),
        }
    }

    // Per-segment data slots: each independently borrowable.
    let data_slots: Vec<Rc<RefCell<Option<Vec<u8>>>>> = (0..total)
        .map(|_| Rc::new(RefCell::new(None)))
        .collect();

    // Read-only metadata.
    let urls: Vec<String>        = segments.iter().map(|s| s.url.clone()).collect();
    let seqs: Vec<Option<u64>>   = segments.iter().map(|s| s.seq).collect();
    let start_times: Vec<(f64, f64)> = segments
        .iter()
        .map(|s| (s.start_time, s.duration))
        .collect();

    let mut append_cursor: usize = 0;
    let mut fetch_cursor:  usize = 0;
    let mut playing = false;

    loop {
        // ── 1. Prefetch ────────────────────────────────────────────────────
        {
            let fetch_up_to = (append_cursor + 2).min(total);
            while fetch_cursor < fetch_up_to {
                let idx  = fetch_cursor;
                let slot = data_slots[idx].clone();
                let url  = urls[idx].clone();

                let already_has_data = slot.borrow().is_some();
                if !already_has_data {
                    info!("pump: prefetching seg {}", idx);
                    spawn_local(async move {
                        match Request::get(&url).send().await {
                            Ok(resp) => match resp.binary().await {
                                Ok(bytes) => {
                                    info!("pump: fetched seg {} ({} bytes)", idx, bytes.len());
                                    *slot.borrow_mut() = Some(bytes);
                                }
                                Err(e) => error!("prefetch binary failed seg {idx}: {e}"),
                            },
                            Err(e) => error!("prefetch send failed seg {idx}: {e}"),
                        }
                    });
                }
                fetch_cursor += 1;
            }
        }

        // ── 2. Done? ───────────────────────────────────────────────────────
        if append_cursor >= total {
            if media_source.ready_state() == web_sys::MediaSourceReadyState::Open {
                let _ = media_source.end_of_stream();
                info!("pump: end_of_stream");
            }
            break;
        }

        // ── 3. PLAYHEAD GATE ───────────────────────────────────────────────
        wait_for_playhead(&video_ref, &start_times, append_cursor).await;

        // ── 4. Wait for segment bytes ──────────────────────────────────────
        info!("pump: waiting for data seg {}", append_cursor);
        loop {
            let has_data = data_slots[append_cursor].borrow().is_some();
            if has_data { break; }
            TimeoutFuture::new(10).await;
        }
        info!("pump: data ready for seg {}", append_cursor);

        // ── 5. Ensure SourceBuffer is idle ─────────────────────────────────
        wait_until_not_updating(&sb_ref).await;

        // ── 6. appendBuffer ────────────────────────────────────────────────
        let mut data_full = data_slots[append_cursor].borrow().clone().unwrap();
        let seq           = seqs[append_cursor];

        // Strip the leading ftyp+moov boxes — the init segment was already
        // appended once; including them again resets MSE's decode context
        // and prevents the buffer from growing past the first segment.
        let offset = strip_init_offset(&data_full);

        info!(
            "pump: appending seg {:?} (cursor={}, {} raw bytes, {} after strip)",
            seq, append_cursor, data_full.len(), data_full.len() - offset
        );

        let append_ok = {
            let sb_guard = sb_ref.borrow();
            match sb_guard.as_ref() {
                None => { error!("pump: SourceBuffer gone"); break; }
                Some(sb) => {
                    match sb.append_buffer_with_u8_array(&mut data_full[offset..]) {
                        Ok(()) => {
                            info!("pump: appended seg {:?} (cursor={}) OK", seq, append_cursor);
                            true
                        }
                        Err(e) => {
                            error!("pump: appendBuffer failed seg {:?}: {:?}", seq, e);
                            false
                        }
                    }
                }
            }
        };

        if !append_ok {
            append_cursor += 1;
            continue;
        }

        // ── 7. Wait for append to finish ──────────────────────────────────
        wait_until_not_updating(&sb_ref).await;
        info!("pump: append complete for seg {:?}", seq);

        // Log video state after append
        if let Some(video) = video_ref.cast::<HtmlVideoElement>() {
            info!(
                "pump: after append — ct={:.2}s paused={} readyState={} networkState={}",
                video.current_time(),
                video.paused(),
                video.ready_state(),
                video.network_state()
            );
            log_buffered_ranges(&video);
        }

        // ── 8. Free fetched bytes ─────────────────────────────────────────
        *data_slots[append_cursor].borrow_mut() = None;

        // ── 9. Start playback ─────────────────────────────────────────────
        if !playing {
            if let Some(video) = video_ref.cast::<HtmlVideoElement>() {
                playing = try_play(&video).await;
                if playing {
                    info!(
                        "pump: playback started — ct={:.2}s paused={} readyState={}",
                        video.current_time(),
                        video.paused(),
                        video.ready_state()
                    );
                } else {
                    warn!("pump: could not start playback");
                }
            }
        }

        // ── 10. Evict old data ────────────────────────────────────────────
        evict_behind_playhead(&video_ref, &sb_ref).await;

        append_cursor += 1;
    }

    info!("pump: finished");
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

    // MSE state
    let media_source = use_memo((), |_| MediaSource::new().expect("MediaSource::new"));

    let source_buffer_ref: Rc<RefCell<Option<SourceBuffer>>> =
        use_memo((), |_| Rc::new(RefCell::new(None)))
            .as_ref()
            .clone();

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
            ((*hover_time).clone(), (*is_hovering_progress).clone(), (*is_dragging).clone()),
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
                        let doc = win.document().unwrap();
                        let _ = doc.remove_event_listener_with_callback(
                            "mousemove",
                            mousemove_closure.as_ref().unchecked_ref(),
                        );
                        let _ = doc.remove_event_listener_with_callback(
                            "mouseup",
                            mouseup_closure.as_ref().unchecked_ref(),
                        );
                    }
                }
            });

            if let Some(win) = window() {
                let doc = win.document().unwrap();
                let _ = doc.add_event_listener_with_callback(
                    "mousemove",
                    on_mousemove.as_ref().unchecked_ref(),
                );
                let _ = doc.add_event_listener_with_callback(
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
    // Effect 1: attach MediaSource, create SourceBuffer in sourceopen
    // ──────────────────────────────────────────────────────────────
    {
        let video_ref    = video_player_ref.clone();
        let media_source = media_source.clone();
        let sb_ref       = source_buffer_ref.clone();

        use_effect_with((), move |_| {
            let sb_ref_inner = sb_ref.clone();
            let ms_for_open  = media_source.clone();

            let on_source_open = Closure::wrap(Box::new(move |_: web_sys::Event| {
                let mime = "video/mp4; codecs=\"avc1.42E01E,mp4a.40.2\"";
                match ms_for_open.add_source_buffer(mime) {
                    Ok(sb) => {
                        // Use "sequence" mode so MSE chains fragments
                        // sequentially regardless of in-fragment PTS values.
                        // This is critical: cached segments may still have
                        // PTS starting at 0, and sequence mode ensures each
                        // fragment is placed after the previous one.
                        sb.set_mode(web_sys::SourceBufferAppendMode::Sequence);
                        info!("SourceBuffer created ok (mode=sequence)");
                        *sb_ref_inner.borrow_mut() = Some(sb);
                    }
                    Err(e) => error!("add_source_buffer failed: {:?}", e),
                }
            }) as Box<dyn FnMut(_)>);

            media_source.set_onsourceopen(Some(on_source_open.as_ref().unchecked_ref()));
            on_source_open.forget();

            if let Some(video) = video_ref.cast::<HtmlVideoElement>() {
                match Url::create_object_url_with_source(&*media_source) {
                    Ok(url) => video.set_src(&url),
                    Err(e)  => error!("create_object_url failed: {:?}", e),
                }
            }

            || ()
        });
    }

    // ──────────────────────────────────────────────────────────────
    // Effect 2: load playlist → wait for SB → run pump
    // ──────────────────────────────────────────────────────────────
    {
        let video_id     = props.video_id.clone();
        let video_ref    = video_player_ref.clone();
        let sb_ref       = source_buffer_ref.clone();
        let media_source = media_source.clone();
        let selected_quality = selected_quality.clone();

        use_effect_with(video_id.clone(), move |_| {
            spawn_local(async move {
                let (segments, init_url) = {
                    let mut vps = match VideoPlayerState::new(NodeRef::default()) {
                        Ok(s)  => s,
                        Err(e) => { error!("VideoPlayerState::new: {:?}", e); return; }
                    };
                    if let Err(e) = vps
                        .load_playlist(video_id.clone(), (*selected_quality).clone())
                        .await
                    {
                        error!("load_playlist failed: {:?}", e);
                        return;
                    }
                    let segs = match vps.playlist.as_ref().and_then(|p| p.representations.first()) {
                        Some(rep) => rep.segments.clone(),
                        None      => { error!("playlist empty"); return; }
                    };
                    let init = vps.playlist.as_ref().and_then(|p| p.init_segment_url.clone());
                    (segs, init)
                };

                info!("playlist loaded — {} segments, init={:?}", segments.len(), init_url);

                loop {
                    if sb_ref.borrow().is_some() { break; }
                    TimeoutFuture::new(20).await;
                }

                run_segment_pump(segments, sb_ref, video_ref, media_source, init_url).await;
            });

            || ()
        });
    }

    // ──────────────────────────────────────────────────────────────
    // Effect 3: periodic UI update (current time, duration, play state)
    // ──────────────────────────────────────────────────────────────
    {
        let video_ref    = video_player_ref.clone();
        let current_time = current_time.clone();
        let duration     = duration.clone();
        let is_playing   = is_playing.clone();
        let volume       = volume.clone();
        let video_ended  = video_ended.clone();
        let is_dragging  = is_dragging.clone();

        use_effect_with((), move |_| {
            use gloo_timers::callback::Interval;

            let interval = Interval::new(250, move || {
                if let Some(v) = video_ref.cast::<HtmlVideoElement>() {
                    // Don't update current_time while dragging (user is controlling it)
                    if !*is_dragging {
                        let ct = v.current_time();
                        if (ct - *current_time).abs() > 0.5 { current_time.set(ct); }
                    }

                    // duration may change once MSE has enough data
                    let dur = v.duration();
                    if dur.is_finite() {
                        if (dur - *duration).abs() > 0.5 { duration.set(dur); }
                    }

                    let playing = !v.paused() && !v.ended();
                    if playing != *is_playing { is_playing.set(playing); }

                    // Track video ended state
                    let ended = v.ended();
                    if ended != *video_ended { video_ended.set(ended); }

                    // Sync volume state on first tick.
                    let vol = v.volume();
                    if (*volume - vol).abs() > 0.01 { volume.set(vol); }
                }
            });

            move || drop(interval)
        });
    }

    // ──────────────────────────────────────────────────────────────
    // Effect 4: robust video event logging for diagnostics
    // ──────────────────────────────────────────────────────────────
    {
        let video_ref = video_player_ref.clone();

        use_effect_with((), move |_| {
            let mut handles: Vec<(&str, Closure<dyn FnMut(web_sys::Event)>)> = Vec::new();
            let video_for_cleanup: Option<HtmlVideoElement> = video_ref.cast::<HtmlVideoElement>();

            if let Some(ref video) = video_for_cleanup {
                // Helper: create a closure that logs video state for a named event.
                macro_rules! log_event {
                    ($name:expr, $video:expr) => {{
                        let v = $video.clone();
                        let name = $name;
                        Closure::wrap(Box::new(move |_: web_sys::Event| {
                            info!(
                                "video event: {} — ct={:.2}s dur={:.1}s rs={} ns={} paused={} ended={}",
                                name,
                                v.current_time(),
                                v.duration(),
                                v.ready_state(),
                                v.network_state(),
                                v.paused(),
                                v.ended(),
                            );
                        }) as Box<dyn FnMut(_)>)
                    }};
                }

                let events: Vec<(&str, Closure<dyn FnMut(web_sys::Event)>)> = vec![
                    ("waiting",        log_event!("waiting",        video)),
                    ("playing",        log_event!("playing",        video)),
                    ("pause",          log_event!("pause",          video)),
                    ("seeking",        log_event!("seeking",        video)),
                    ("seeked",         log_event!("seeked",         video)),
                    ("stalled",        log_event!("stalled",        video)),
                    ("error",          log_event!("error",          video)),
                    ("ended",          log_event!("ended",          video)),
                    ("canplay",        log_event!("canplay",        video)),
                    ("canplaythrough", log_event!("canplaythrough", video)),
                    ("loadeddata",     log_event!("loadeddata",     video)),
                ];

                let target: &web_sys::EventTarget = video.as_ref();
                for (name, closure) in events {
                    let _ = target.add_event_listener_with_callback(
                        name, closure.as_ref().unchecked_ref(),
                    );
                    handles.push((name, closure));
                }
            }

            // Cleanup: remove all listeners (same closure type on all paths).
            move || {
                if let Some(ref video) = video_for_cleanup {
                    let target: &web_sys::EventTarget = video.as_ref();
                    for (name, closure) in &handles {
                        let _ = target.remove_event_listener_with_callback(
                            name, closure.as_ref().unchecked_ref(),
                        );
                    }
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
        // TODO: read actual buffered end from video element
        progress_percent
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
