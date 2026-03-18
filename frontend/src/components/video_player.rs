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
    pub representations: Vec<Representation>,
    pub is_live:         bool,
    pub total_duration:  Option<f64>,
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
    playlist_base_url: &str,
    quality:           &str,
) -> Result<Playlist, String> {
    let lines: Vec<&str> = text
        .lines()
        .map(|l| l.trim())
        .filter(|l| !l.is_empty())
        .collect();

    let mut segments     = Vec::new();
    let mut current_time = 0.0f64;
    let mut i            = 0usize;

    while i < lines.len() {
        let line = lines[i];
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

    info!("{:?}", segments);

    let rep            = Representation { id: quality.to_string(), bitrate: 0, segments };
    let total_duration = Some(rep.segments.iter().map(|s| s.duration).sum());

    Ok(Playlist {
        representations: vec![rep],
        is_live:         false,
        total_duration,
    })
}

// ── Segment pump helpers ─────────────────────────────────────────────────────

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
    segments:     Vec<Segment>,
    sb_ref:       Rc<RefCell<Option<SourceBuffer>>>,
    video_ref:    NodeRef,
    media_source: Rc<MediaSource>,
) {
    let total = segments.len();
    info!("pump: {} segments total", total);

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
        let mut data = data_slots[append_cursor].borrow().clone().unwrap();
        let seq      = seqs[append_cursor];

        info!("pump: appending seg {:?} (cursor={}, {} bytes)", seq, append_cursor, data.len());

        let append_ok = {
            let sb_guard = sb_ref.borrow();
            match sb_guard.as_ref() {
                None => { error!("pump: SourceBuffer gone"); break; }
                Some(sb) => {
                    match sb.append_buffer_with_u8_array(data.as_mut_slice()) {
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

    let on_container_click = {
        let video_ref = video_player_ref.clone();
        Callback::from(move |_| {
            if let Some(v) = video_ref.cast::<HtmlVideoElement>() {
                let _ = v.play();
            }
        })
    };
    let on_mouse_leave       = Callback::from(move |_| {});
    let on_mouse_move        = Callback::from(move |_| {});
    let on_speed_toggle      = Callback::from(move |_| { info!("toggling speed!"); });
    let on_quality_toggle    = Callback::from(move |_| { info!("toggling quality!"); });
    let on_quality_select    = Callback::from(move |_: String| { info!("selecting quality!"); });
    let on_speed_select      = Callback::from(move |_: f32| { info!("selecting speed!"); });
    let on_fullscreen_toggle = Callback::from(move |_| { info!("toggling fullscreen!"); });

    let on_video_click = Callback::from(move |e: MouseEvent| {
        if let Some(v) = e.target_dyn_into::<HtmlVideoElement>() {
            let _ = v.play();
        }
    });

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
                        *sb_ref_inner.borrow_mut() = Some(sb);
                        info!("SourceBuffer created ok");
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

        use_effect_with(video_id.clone(), move |_| {
            spawn_local(async move {
                let segments = {
                    let mut vps = match VideoPlayerState::new(NodeRef::default()) {
                        Ok(s)  => s,
                        Err(e) => { error!("VideoPlayerState::new: {:?}", e); return; }
                    };
                    if let Err(e) = vps
                        .load_playlist(video_id.clone(), "original".to_string())
                        .await
                    {
                        error!("load_playlist failed: {:?}", e);
                        return;
                    }
                    match vps.playlist.as_ref().and_then(|p| p.representations.first()) {
                        Some(rep) => rep.segments.clone(),
                        None      => { error!("playlist empty"); return; }
                    }
                };

                info!("playlist loaded — {} segments", segments.len());

                loop {
                    if sb_ref.borrow().is_some() { break; }
                    TimeoutFuture::new(20).await;
                }

                run_segment_pump(segments, sb_ref, video_ref, media_source).await;
            });

            || ()
        });
    }

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
                    onclick={Callback::from(move |_| {})}
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
                <div class="player-progress-container">
                    <div class="player-progress">
                        <div class="player-progress__buffered" />
                        <div class="player-progress__played" />
                        <div class={if false {
                            "player-progress__thumb player-progress__thumb--dragging"
                        } else {
                            "player-progress__thumb"
                        }} />
                    </div>
                </div>

                <div class="player-controls__bottom">
                    <div class="player-controls__left">
                        <button class="player-controls__btn" title="Play/Pause (k)">
                            { icon_play() }
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
                            <button class="player-controls__btn" title="Mute (m)">
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
