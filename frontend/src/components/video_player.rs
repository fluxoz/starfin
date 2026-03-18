 use gloo_net::http::Request;
use std::collections::VecDeque;
use gloo_net::Error as GlooError;
use gloo_timers::future::TimeoutFuture;
use std::cell::RefCell;
use std::rc::Rc;
use wasm_bindgen::prelude::*;
use wasm_bindgen::JsCast;
use wasm_bindgen_futures::spawn_local;
use wasm_bindgen_futures::JsFuture;
use web_sys::{HtmlVideoElement, MediaSource, MouseEvent, Url, window, SourceBuffer};
use yew::prelude::*;
use log::{info, warn, error};

// ── Playback speed options ───────────────────────────────────────────────────
const PLAYBACK_SPEEDS: [f32; 9] = [0.25, 0.5, 0.75, 1.0, 1.25, 1.5, 1.75, 2.0, 3.0];

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

fn format_time(seconds: u32) -> String {
    if seconds == 0 {
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
    let is_muted              = use_state(|| false);
    let volume                = use_state(|| 0.0_f64);
    let current_time          = use_state(|| 0u32);
    let duration              = use_state(|| 0u32);
    let volume_slider_visible = use_state(|| false);
    let is_fullscreen         = use_state(|| false);
    let playback_speed        = use_state(|| 1_f32);
    let speed_menu_open       = use_state(|| false);
    let quality_menu_open     = use_state(|| false);

    let media_source = use_memo((), |_| MediaSource::new().expect("MediaSource::new"));

    let source_buffer_ref: Rc<RefCell<Option<SourceBuffer>>> =
        use_memo((), |_| Rc::new(RefCell::new(None)))
            .as_ref()
            .clone();

    let initial_quality = window()
        .and_then(|w| w.local_storage().ok())
        .flatten()
        .and_then(|s| s.get_item(QUALITY_STORAGE_KEY).ok())
        .flatten()
        .filter(|q| QUALITY_OPTIONS.iter().any(|(v, _)| v == q))
        .unwrap_or_else(|| "original".to_string());
    let selected_quality = use_state(|| initial_quality);

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

    let container_ref   = use_node_ref();
    let container_class = if *is_fullscreen {
        "player-overlay player-overlay--fullscreen"
    } else {
        "player-overlay"
    };

    // ── Play/pause toggle ───────────────────────────────────────────
    let is_playing = use_state(|| false);

    let on_container_click = {
        let video_ref = video_player_ref.clone();
        let is_playing = is_playing.clone();
        Callback::from(move |_: MouseEvent| {
            if let Some(v) = video_ref.cast::<HtmlVideoElement>() {
                if v.paused() {
                    let _ = v.play();
                    is_playing.set(true);
                } else {
                    v.pause().ok();
                    is_playing.set(false);
                }
            }
        })
    };
    let on_mouse_leave       = Callback::from(move |_| {});
    let on_mouse_move        = Callback::from(move |_| {});

    let on_play_pause = {
        let video_ref = video_player_ref.clone();
        let is_playing = is_playing.clone();
        Callback::from(move |e: MouseEvent| {
            e.stop_propagation();
            if let Some(v) = video_ref.cast::<HtmlVideoElement>() {
                if v.paused() {
                    let _ = v.play();
                    is_playing.set(true);
                } else {
                    v.pause().ok();
                    is_playing.set(false);
                }
            }
        })
    };

    // ── Speed menu ───────────────────────────────────────────────
    let on_speed_toggle = {
        let speed_menu_open = speed_menu_open.clone();
        Callback::from(move |e: MouseEvent| {
            e.stop_propagation();
            speed_menu_open.set(!*speed_menu_open);
        })
    };
    let on_speed_select = {
        let playback_speed = playback_speed.clone();
        let speed_menu_open = speed_menu_open.clone();
        let video_ref = video_player_ref.clone();
        Callback::from(move |speed: f32| {
            playback_speed.set(speed);
            speed_menu_open.set(false);
            if let Some(v) = video_ref.cast::<HtmlVideoElement>() {
                v.set_playback_rate(speed as f64);
            }
        })
    };

    // ── Quality menu ─────────────────────────────────────────────
    let on_quality_toggle = {
        let quality_menu_open = quality_menu_open.clone();
        Callback::from(move |e: MouseEvent| {
            e.stop_propagation();
            quality_menu_open.set(!*quality_menu_open);
        })
    };
    let on_quality_select = {
        let selected_quality = selected_quality.clone();
        let quality_menu_open = quality_menu_open.clone();
        Callback::from(move |q: String| {
            // Persist to localStorage.
            if let Some(storage) = window()
                .and_then(|w| w.local_storage().ok())
                .flatten()
            {
                let _ = storage.set_item(QUALITY_STORAGE_KEY, &q);
            }
            selected_quality.set(q);
            quality_menu_open.set(false);
            // Quality change takes effect on next video load.
        })
    };

    // ── Fullscreen ───────────────────────────────────────────────
    let on_fullscreen_toggle = {
        let container_ref = container_ref.clone();
        let is_fullscreen = is_fullscreen.clone();
        Callback::from(move |e: MouseEvent| {
            e.stop_propagation();
            if let Some(el) = container_ref.cast::<web_sys::HtmlElement>() {
                if *is_fullscreen {
                    if let Some(doc) = web_sys::window().and_then(|w| w.document()) {
                        let _ = doc.exit_fullscreen();
                    }
                    is_fullscreen.set(false);
                } else {
                    let _ = el.request_fullscreen();
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
        Callback::from(move |e: MouseEvent| {
            e.stop_propagation();
            if let Some(v) = video_ref.cast::<HtmlVideoElement>() {
                let new_muted = !v.muted();
                v.set_muted(new_muted);
                is_muted.set(new_muted);
                if !new_muted && *volume == 0.0 {
                    v.set_volume(1.0);
                    volume.set(1.0);
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
        let is_playing = is_playing.clone();
        Callback::from(move |e: MouseEvent| {
            e.stop_propagation();
            if let Some(v) = e.target_dyn_into::<HtmlVideoElement>() {
                if v.paused() {
                    let _ = v.play();
                    is_playing.set(true);
                } else {
                    v.pause().ok();
                    is_playing.set(false);
                }
            }
        })
    };

    // ── Progress bar seeking (click + drag) ─────────────────────
    let progress_ref    = use_node_ref();
    let is_seeking      = use_state(|| false);
    let seek_preview_pct = use_state(|| 0.0_f64);

    // Helper: compute seek percentage from a MouseEvent relative to the
    // progress bar element.
    let calc_seek_pct = {
        let progress_ref = progress_ref.clone();
        Rc::new(move |e: &MouseEvent| -> Option<f64> {
            progress_ref.cast::<web_sys::HtmlElement>().map(|el| {
                let rect = el.get_bounding_client_rect();
                let x = e.client_x() as f64 - rect.left();
                (x / rect.width()).clamp(0.0, 1.0)
            })
        })
    };

    // mousedown on the progress bar → start drag
    let on_progress_mousedown = {
        let is_seeking = is_seeking.clone();
        let seek_preview_pct = seek_preview_pct.clone();
        let calc = calc_seek_pct.clone();
        Callback::from(move |e: MouseEvent| {
            e.stop_propagation();
            e.prevent_default();
            is_seeking.set(true);
            if let Some(pct) = calc(&e) {
                seek_preview_pct.set(pct * 100.0);
            }
        })
    };

    // Effect: while dragging, attach document-level mousemove/mouseup
    // listeners so that dragging outside the bar still works.
    {
        let is_seeking       = is_seeking.clone();
        let seek_preview_pct = seek_preview_pct.clone();
        let video_ref        = video_player_ref.clone();
        let duration         = duration.clone();
        let current_time     = current_time.clone();
        let progress_ref     = progress_ref.clone();

        use_effect_with(*is_seeking, move |seeking| {
            // Shared cleanup handles — always created so every return path
            // returns the same closure type.
            let move_ref: Rc<RefCell<Option<Closure<dyn FnMut(MouseEvent)>>>> =
                Rc::new(RefCell::new(None));
            let up_ref: Rc<RefCell<Option<Closure<dyn FnMut(MouseEvent)>>>> =
                Rc::new(RefCell::new(None));
            let doc_ref: Rc<RefCell<Option<web_sys::Document>>> =
                Rc::new(RefCell::new(None));

            if *seeking {
                if let Some(doc) = web_sys::window().and_then(|w| w.document()) {
                    // mousemove → update preview position
                    let progress_ref2 = progress_ref.clone();
                    let seek_preview_pct2 = seek_preview_pct.clone();
                    let on_move = Closure::wrap(Box::new(move |e: MouseEvent| {
                        if let Some(el) = progress_ref2.cast::<web_sys::HtmlElement>() {
                            let rect = el.get_bounding_client_rect();
                            let x = e.client_x() as f64 - rect.left();
                            let pct = (x / rect.width()).clamp(0.0, 1.0);
                            seek_preview_pct2.set(pct * 100.0);
                        }
                    }) as Box<dyn FnMut(_)>);

                    // mouseup → commit seek + stop drag
                    let is_seeking2 = is_seeking.clone();
                    let seek_preview_pct3 = seek_preview_pct.clone();
                    let progress_ref3 = progress_ref.clone();
                    let video_ref2 = video_ref.clone();
                    let duration2 = duration.clone();
                    let current_time2 = current_time.clone();
                    let doc2 = doc.clone();
                    let move_ref2 = move_ref.clone();
                    let up_ref2   = up_ref.clone();

                    let on_up = Closure::wrap(Box::new(move |e: MouseEvent| {
                        if let Some(el) = progress_ref3.cast::<web_sys::HtmlElement>() {
                            let rect = el.get_bounding_client_rect();
                            let x = e.client_x() as f64 - rect.left();
                            let pct = (x / rect.width()).clamp(0.0, 1.0);
                            seek_preview_pct3.set(pct * 100.0);
                            if *duration2 > 0 {
                                let seek_to = pct * *duration2 as f64;
                                if let Some(v) = video_ref2.cast::<HtmlVideoElement>() {
                                    v.set_current_time(seek_to);
                                    info!("seek: dragged to {:.2}s", seek_to);
                                }
                                current_time2.set(seek_to as u32);
                            }
                        }
                        is_seeking2.set(false);

                        // Remove document listeners.
                        if let Some(cb) = move_ref2.borrow().as_ref() {
                            let _ = doc2.remove_event_listener_with_callback(
                                "mousemove", cb.as_ref().unchecked_ref(),
                            );
                        }
                        if let Some(cb) = up_ref2.borrow().as_ref() {
                            let _ = doc2.remove_event_listener_with_callback(
                                "mouseup", cb.as_ref().unchecked_ref(),
                            );
                        }
                    }) as Box<dyn FnMut(_)>);

                    let _ = doc.add_event_listener_with_callback(
                        "mousemove", on_move.as_ref().unchecked_ref(),
                    );
                    let _ = doc.add_event_listener_with_callback(
                        "mouseup", on_up.as_ref().unchecked_ref(),
                    );

                    *move_ref.borrow_mut() = Some(on_move);
                    *up_ref.borrow_mut()   = Some(on_up);
                    *doc_ref.borrow_mut()  = Some(doc);
                }
            }

            // Cleanup — same type regardless of branch.
            let move_ref_cleanup = move_ref;
            let up_ref_cleanup   = up_ref;
            let doc_cleanup      = doc_ref;
            move || {
                if let Some(doc) = doc_cleanup.borrow().as_ref() {
                    if let Some(cb) = move_ref_cleanup.borrow().as_ref() {
                        let _ = doc.remove_event_listener_with_callback(
                            "mousemove", cb.as_ref().unchecked_ref(),
                        );
                    }
                    if let Some(cb) = up_ref_cleanup.borrow().as_ref() {
                        let _ = doc.remove_event_listener_with_callback(
                            "mouseup", cb.as_ref().unchecked_ref(),
                        );
                    }
                }
            }
        });
    }

    // Click on the progress bar (for non-drag single clicks).
    let on_progress_click = {
        let video_ref = video_player_ref.clone();
        let progress_ref = progress_ref.clone();
        let duration = duration.clone();
        Callback::from(move |e: MouseEvent| {
            e.stop_propagation();
            if *duration == 0 { return; }
            if let Some(el) = progress_ref.cast::<web_sys::HtmlElement>() {
                let rect = el.get_bounding_client_rect();
                let x = e.client_x() as f64 - rect.left();
                let pct = (x / rect.width()).clamp(0.0, 1.0);
                let seek_to = pct * *duration as f64;
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
        let video_ref  = video_player_ref.clone();
        let current_time = current_time.clone();
        let duration     = duration.clone();
        let is_playing   = is_playing.clone();
        let volume       = volume.clone();

        use_effect_with((), move |_| {
            use gloo_timers::callback::Interval;

            let interval = Interval::new(250, move || {
                if let Some(v) = video_ref.cast::<HtmlVideoElement>() {
                    let ct = v.current_time() as u32;
                    if ct != *current_time { current_time.set(ct); }

                    // duration may change once MSE has enough data
                    let dur = v.duration();
                    if dur.is_finite() {
                        let d = dur as u32;
                        if d != *duration { duration.set(d); }
                    }

                    let playing = !v.paused() && !v.ended();
                    if playing != *is_playing { is_playing.set(playing); }

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
    let played_pct = if *duration > 0 {
        (*current_time as f64 / *duration as f64 * 100.0).min(100.0)
    } else {
        0.0
    };

    // Play/pause icon
    let play_pause_icon: Html = if *is_playing {
        icon_pause()
    } else {
        icon_play()
    };

    html! {
        <div
            ref={container_ref}
            class={container_class}
            onclick={on_container_click}
            onmousemove={on_mouse_move}
            onmouseleave={on_mouse_leave}
        >
            <div class={if true { "player-header" } else { "player-header player-header--hidden" }}>
                <button
                    class="btn btn--back"
                    onclick={props.on_close.reform(|_| ())}
                >
                    { icon_arrow_back() }
                    { " Back" }
                </button>
                <span class="player-title">{ title }</span>
            </div>

            if let Some(err) = Some(false) {
                <div class="notice notice--error">
                    <div class="notice__title">{ "Playback error" }</div>
                    <div class="notice__body">{ err }</div>
                </div>
            }

            <video
                ref={video_player_ref}
                class="video-el"
                onclick={on_video_click}
            />

            <div class={"player-controls"}>
                <div class="player-progress-container"
                    ref={progress_ref}
                    onclick={on_progress_click}
                    onmousedown={on_progress_mousedown}
                >
                    <div class="player-progress">
                        <div class="player-progress__buffered" />
                        <div class="player-progress__played"
                            style={format!("width: {:.1}%",
                                if *is_seeking { *seek_preview_pct } else { played_pct })}
                        />
                        <div class={if *is_seeking {
                            "player-progress__thumb player-progress__thumb--dragging"
                        } else {
                            "player-progress__thumb"
                        }}
                            style={format!("left: {:.1}%",
                                if *is_seeking { *seek_preview_pct } else { played_pct })}
                        />
                    </div>
                </div>

                <div class="player-controls__bottom">
                    <div class="player-controls__left">
                        <button
                            class="player-controls__btn"
                            title="Play/Pause (k)"
                            onclick={on_play_pause}
                        >
                            { play_pause_icon }
                        </button>

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
                            <button
                                class="player-controls__btn"
                                title="Mute (m)"
                                onclick={on_mute_toggle}
                            >
                                { volume_icon }
                            </button>
                            <div class={if *volume_slider_visible {
                                "player-volume__slider player-volume__slider--visible"
                            } else {
                                "player-volume__slider"
                            }}>
                                <input
                                    type="range"
                                    min="0"
                                    max="1"
                                    step="0.05"
                                    value={volume.to_string()}
                                    class="player-volume__input"
                                    oninput={on_volume_change}
                                />
                            </div>
                        </div>

                        <span class="player-controls__time">{ time_display }</span>
                    </div>

                    <div class="player-controls__right">
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
                                        let is_active = (*playback_speed - speed) == 0.0;
                                        html! {
                                            <button
                                                class={if is_active {
                                                    "player-speed__option player-speed__option--active"
                                                } else {
                                                    "player-speed__option"
                                                }}
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
                                                class={if is_active {
                                                    "player-quality__option player-quality__option--active"
                                                } else {
                                                    "player-quality__option"
                                                }}
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

                        <button
                            class="player-controls__btn"
                            onclick={on_fullscreen_toggle}
                            title="Fullscreen (f)"
                        >
                            { fullscreen_icon }
                        </button>
                    </div>
                </div>
            </div>
        </div>
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
