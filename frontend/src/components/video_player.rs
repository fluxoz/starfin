// DASH video player — powered by dashjs-rs (Rust port of dash.js).
//
// Architecture:
//   dashjs-rs MediaPlayer    → state machine for playback (play/pause/seek/volume)
//   dashjs-rs GapController  → detects & jumps gaps between buffered ranges
//   dashjs-rs BufferController → tracks buffer level, decides when to fetch
//   dashjs-rs ScheduleController → scheduling decisions (should_schedule)
//   dashjs-rs ThroughputController → dual-EWMA bandwidth estimation
//   dashjs-rs parser         → MPD parsing + URI template expansion
//   Browser MSE              → MediaSource + SourceBuffer for actual media pipeline
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

// dashjs-rs imports
use dashjs_rs::MediaPlayer as DashMediaPlayer;
use dashjs_rs::dash::parser as dash_parser;
use dashjs_rs::streaming::controllers::buffer_controller::BufferedRange;

// ── Playback speed options ───────────────────────────────────────────────────
const PLAYBACK_SPEEDS: [f64; 9] = [0.25, 0.5, 0.75, 1.0, 1.25, 1.5, 1.75, 2.0, 3.0];

// ── Segment duration — must stay in sync with SEGMENT_DURATION in main.rs ───
const SEGMENT_DURATION_F: f64 = 6.0;

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

// ── Buffer targets (driven by dashjs-rs ScheduleController) ──────────────────
// Matches dash.js streaming.buffer.bufferTimeAtTopQuality (default 30s).
// Chrome SourceBuffer quota is ~150-200 MB.  At 5 Mbps → 30s ≈ 19 MB which
// is safe.  We use 30s ahead like dash.js.
const BUFFER_TARGET_S: f64 = 30.0;
// dash.js streaming.buffer.bufferToKeep (default 20s) — data behind playhead
// to keep. Anything older is pruned every PRUNING_INTERVAL_MS.
const BACK_BUFFER_S: f64 = 20.0;
// On QuotaExceededError, keep only 5s behind playhead (more aggressive).
const QUOTA_KEEP_BEHIND_S: f64 = 5.0;
// Minimum remove range to bother with (avoids no-op removes).
const MIN_REMOVE_THRESHOLD_S: f64 = 0.5;
// dash.js streaming.buffer.bufferPruningInterval (default 10s).
const PRUNING_INTERVAL_MS: f64 = 10_000.0;
// Max consecutive fetch failures before skipping a segment.
const MAX_FETCH_FAILURES: u32 = 3;
// Max consecutive append failures (non-quota) before the pump exits.
const MAX_APPEND_FAILURES: u32 = 5;
// Max QuotaExceeded retries for a single segment before skipping it.
const MAX_QUOTA_RETRIES: u32 = 3;

// ── Lookahead prefetch (mirrors dash.js StreamProcessor._onMediaFragmentNeeded) ──
/// How many segments ahead of next_seg to prefetch into the SegmentCache.
/// dash.js uses a full bufferTarget/segmentDuration window; we use 3 here.
const LOOKAHEAD_WINDOW: usize = 3;

/// Shared cache of already-fetched segment bytes, keyed by segment index.
/// Populated by background `spawn_local` prefetch tasks.
/// Accessed by both the main pump loop (read/evict) and background tasks (write).
type SegmentCache = Rc<RefCell<std::collections::HashMap<usize, Vec<u8>>>>;

/// Set of segment indices currently being fetched in background prefetch tasks.
/// Prevents duplicate in-flight fetches for the same segment.
type InFlightSet = Rc<RefCell<std::collections::HashSet<usize>>>;

// ══════════════════════════════════════════════════════════════════════════════
// DASH ENGINE — powered by dashjs-rs
// ══════════════════════════════════════════════════════════════════════════════

/// Segment info extracted from the parsed MPD.
struct SegmentInfo {
    url: String,
    duration: f64,
}

/// Probe browser MIME type support — fallback when MPD has no `codecs` attribute.
/// Tries common H.264+AAC codec strings and falls back to plain `video/mp4`.
fn probe_mime_type() -> String {
    let candidates = [
        "video/mp4; codecs=\"avc1.640029,mp4a.40.2\"",
        "video/mp4; codecs=\"avc1.64001F,mp4a.40.2\"",
        "video/mp4; codecs=\"avc1.4D4028,mp4a.40.2\"",
        "video/mp4; codecs=\"avc1.42E01E,mp4a.40.2\"",
        "video/mp4",
    ];
    candidates.iter()
        .find(|m| web_sys::MediaSource::is_type_supported(m))
        .unwrap_or(candidates.last().unwrap())
        .to_string()
}

/// Server commands for future WebSocket/SSE integration with main.rs.
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

/// Apply a server command to the video element.
fn apply_server_command(video: &HtmlVideoElement, cmd: &ServerCommand) -> bool {
    match cmd {
        ServerCommand::Play => { let _ = video.play(); true }
        ServerCommand::Pause => { let _ = video.pause(); true }
        ServerCommand::Seek { time } => {
            let dur = video.duration();
            if dur.is_finite() && *time >= 0.0 {
                video.set_current_time(time.min(dur));
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

/// The DashPlayer wraps dashjs-rs MediaPlayer + browser MSE state.
/// All playback pipeline decisions flow through dashjs-rs controllers.
struct DashPlayer {
    /// dashjs-rs engine — MPD parsing, ABR, throughput, buffer, schedule, gap
    engine: DashMediaPlayer,
    /// Browser MediaSource
    media_source: web_sys::MediaSource,
    /// Browser SourceBuffer
    source_buffer: web_sys::SourceBuffer,
    /// Blob URL (revoked on cleanup)
    object_url: String,
    /// Segment list from parsed MPD
    segments: Vec<SegmentInfo>,
    /// Next segment index to fetch
    next_seg: usize,
    /// Generation counter for pump cancellation on seek
    pump_gen: u32,
    /// Whether pump loop is running
    pump_running: bool,
    /// Last segment index appended
    last_appended_seg: Option<usize>,
    /// Wall-clock timestamp of last eviction attempt (ms).
    /// Eviction runs INLINE in pump_loop — no separate timer — to avoid
    /// races between sb.remove() and sb.appendBuffer().
    /// Matches dash.js _onWallclockTimeUpdated / bufferPruningInterval=10s.
    last_eviction_ms: f64,
    /// Pre-fetched segment bytes (keyed by segment index).
    /// Background tasks write here; pump_loop reads and evicts.
    seg_cache: SegmentCache,
    /// Set of segment indices currently being fetched in background tasks.
    in_flight: InFlightSet,
    /// Fatal error flag — when set, the pump should NOT be restarted.
    /// Set on unrecoverable errors like Firefox H264 decode failures that
    /// detach the SourceBuffer.  Prevents the periodic timer from restarting
    /// the pump in an infinite loop of identical errors.
    fatal_error: bool,
}

// ── MPD Parsing (via dashjs-rs) ──────────────────────────────────────────────

/// Parsed MPD result: init URL, total duration, segment list, and
/// MIME+codecs string for `MediaSource.addSourceBuffer()`.
///
/// The codec string is built exactly like dash.js
/// `SourceBufferSink._getCodecStringForRepresentation()`:
///   `representation.mimeType + ';codecs="' + representation.codecs + '"'`
struct MpdParseResult {
    init_url: String,
    total_duration: f64,
    segments: Vec<SegmentInfo>,
    /// Full MIME type with codecs for `addSourceBuffer()`, e.g.
    /// `video/mp4;codecs="avc1.640029,mp4a.40.2"`.
    /// `None` if the MPD has no codec info.
    mime_codec: Option<String>,
}

/// Parse MPD using dashjs-rs parser, extract segment list.
fn parse_mpd(text: &str) -> MpdParseResult {
    match dash_parser::parse(text) {
        Ok(mpd) => extract_segments(&mpd, text),
        Err(e) => {
            log::error!("dashjs-rs MPD parse error: {:?} {}", e.code, e.message);
            MpdParseResult {
                init_url: String::new(),
                total_duration: 0.0,
                segments: Vec::new(),
                mime_codec: None,
            }
        }
    }
}

/// Extract segments from dashjs-rs Mpd using template expansion.
///
/// Also extracts the codec string from the MPD `<Representation>` element,
/// matching dash.js `SourceBufferSink._getCodecStringForRepresentation()`.
fn extract_segments(
    mpd: &dashjs_rs::dash::vo::Mpd,
    raw_xml: &str,
) -> MpdParseResult {
    let empty = MpdParseResult {
        init_url: String::new(),
        total_duration: 0.0,
        segments: Vec::new(),
        mime_codec: None,
    };
    if mpd.periods.is_empty() { return empty; }
    let period = &mpd.periods[0];
    if period.adaptation_sets.is_empty() { return empty; }

    let aset_idx = period.adaptation_sets.iter()
        .position(|a| {
            a.content_type.as_deref() == Some("video")
                || a.mime_type.as_deref().is_some_and(|m| m.starts_with("video"))
                || a.content_type.is_none()
        })
        .unwrap_or(0);
    let aset = &period.adaptation_sets[aset_idx];
    if aset.representations.is_empty() { return empty; }

    let rep = &aset.representations[0];
    let total_duration = mpd.media_presentation_duration.unwrap_or(0.0);
    let rep_id = rep.id.as_deref().unwrap_or("");
    let bw = rep.bandwidth.unwrap_or(0);

    // ── Codec string — 1:1 match of dash.js SourceBufferSink ──
    // dash.js: `representation.mimeType + ';codecs="' + representation.codecs + '"'`
    // mimeType inherits from AdaptationSet if not on Representation.
    let mime_type = rep.mime_type.as_deref()
        .or(aset.mime_type.as_deref())
        .unwrap_or("video/mp4");
    let codecs = rep.codecs.as_deref()
        .or(aset.codecs.as_deref());
    let mime_codec = codecs.map(|c| format!("{mime_type};codecs=\"{c}\""));

    // Build init URL via dashjs-rs template expansion
    let init_url = rep.initialization.as_deref()
        .map(|tmpl| dash_parser::process_uri_template(tmpl, Some(rep_id), None, None, Some(bw), None))
        .unwrap_or_default();

    let media_tmpl = rep.media.as_deref().unwrap_or("");
    if media_tmpl.is_empty() {
        return MpdParseResult { init_url, total_duration, segments: Vec::new(), mime_codec };
    }

    let mut segs = Vec::new();

    // Extract SegmentTimeline S elements from raw XML
    let timeline = extract_timeline_from_xml(raw_xml);
    if !timeline.is_empty() {
        let timescale = rep.timescale.max(1);
        let mut num = rep.start_number;
        let mut ct: u64 = 0;
        for entry in &timeline {
            if let Some(tv) = entry.t { ct = tv; }
            let rc = entry.r.unwrap_or(0).max(0) as usize;
            for _ in 0..=rc {
                let url = dash_parser::process_uri_template(
                    media_tmpl, Some(rep_id), Some(num), None, Some(bw), Some(ct),
                );
                segs.push(SegmentInfo {
                    url,
                    duration: entry.d as f64 / timescale as f64,
                });
                ct += entry.d;
                num += 1;
            }
        }
    } else if let Some(sd) = rep.segment_duration {
        let n = if sd > 0.0 { (total_duration / sd).ceil() as usize } else { 0 };
        for i in 0..n {
            segs.push(SegmentInfo {
                url: dash_parser::process_uri_template(
                    media_tmpl, Some(rep_id), Some(rep.start_number + i as u64), None, Some(bw), None,
                ),
                duration: sd,
            });
        }
    } else if total_duration > 0.0 {
        // Fallback: use known segment duration constant
        let n = (total_duration / SEGMENT_DURATION_F).ceil() as usize;
        for i in 0..n {
            segs.push(SegmentInfo {
                url: dash_parser::process_uri_template(
                    media_tmpl, Some(rep_id), Some(rep.start_number + i as u64), None, Some(bw), None,
                ),
                duration: SEGMENT_DURATION_F,
            });
        }
    }

    MpdParseResult { init_url, total_duration, segments: segs, mime_codec }
}

struct TimelineEntry { t: Option<u64>, d: u64, r: Option<i64> }

fn extract_timeline_from_xml(xml: &str) -> Vec<TimelineEntry> {
    fn tag_attr(tag: &str, attr: &str) -> Option<String> {
        let s = format!("{attr}=\"");
        let p = tag.find(&s)?;
        let r = &tag[p + s.len()..];
        Some(r[..r.find('"')?].to_string())
    }
    let mut result = Vec::new();
    let mut s = 0;
    while let Some(i) = xml[s..].find("<S ") {
        let a = s + i;
        if let Some(e) = xml[a..].find("/>") {
            let t = &xml[a..a + e + 2];
            result.push(TimelineEntry {
                t: tag_attr(t, "t").and_then(|s| s.parse().ok()),
                d: tag_attr(t, "d").and_then(|s| s.parse().ok()).unwrap_or(0),
                r: tag_attr(t, "r").and_then(|s| s.parse().ok()),
            });
            s = a + e + 2;
        } else { break; }
    }
    result
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

/// Get the continuous buffered range at `time`, merging adjacent ranges whose
/// gap is ≤ `tolerance`.  This is a 1:1 port of dash.js
/// `BufferController.getRangeAt(time, tolerance)` which merges through small
/// gaps (default `smallGapLimit = 0.15s`).
///
/// Returns `(range_start, range_end)` or `None`.
fn get_range_at(video: &HtmlVideoElement, time: f64, tolerance: f64) -> Option<(f64, f64)> {
    let buffered = video.buffered();
    let mut first_start: Option<f64> = None;
    let mut last_end: f64 = 0.0;

    for i in 0..buffered.length() {
        if let (Ok(start), Ok(end)) = (buffered.start(i), buffered.end(i)) {
            if first_start.is_none() {
                let gap = (start - time).abs();
                if time >= start && time < end {
                    // time is inside this range
                    first_start = Some(start);
                    last_end = end;
                } else if gap <= tolerance {
                    // time is within tolerance of range start
                    first_start = Some(start);
                    last_end = end;
                }
            } else {
                let gap = start - last_end;
                if gap <= tolerance {
                    // merge adjacent ranges with small gap
                    last_end = end;
                } else {
                    break;
                }
            }
        }
    }

    first_start.map(|_| (first_start.unwrap(), last_end))
}

/// dash.js `_getBufferLength(time, tolerance)`: returns the number of seconds
/// buffered ahead of `time`, using `getRangeAt` with gap tolerance.
/// This is the buffer level used by `ScheduleController._shouldBuffer()`.
const BUFFER_RANGE_TOLERANCE: f64 = 0.15; // dash.js streaming.gaps.smallGapLimit

fn get_buffer_level(video: &HtmlVideoElement, time: f64) -> f64 {
    match get_range_at(video, time, BUFFER_RANGE_TOLERANCE) {
        Some((_start, end)) => (end - time).max(0.0),
        None => 0.0,
    }
}

/// Legacy helper — returns the end of the buffered range containing `time`.
fn buffered_end_at(video: &HtmlVideoElement, time: f64) -> f64 {
    match get_range_at(video, time, BUFFER_RANGE_TOLERANCE) {
        Some((_start, end)) => end,
        None => 0.0,
    }
}

fn is_time_buffered(video: &HtmlVideoElement, time: f64) -> bool {
    get_range_at(video, time, BUFFER_RANGE_TOLERANCE).is_some()
}

/// Get buffered ranges as dashjs-rs BufferedRange vec for GapController.
fn get_buffered_ranges(video: &HtmlVideoElement) -> Vec<BufferedRange> {
    let buffered = video.buffered();
    let mut ranges = Vec::new();
    for i in 0..buffered.length() {
        if let (Ok(s), Ok(e)) = (buffered.start(i), buffered.end(i)) {
            ranges.push(BufferedRange { start: s, end: e });
        }
    }
    ranges
}

/// Compute the segment index for a given time position.
fn segment_for_time(t: f64) -> usize {
    if t <= 0.0 { 0 } else { (t / SEGMENT_DURATION_F) as usize }
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
// PLAYBACK PIPELINE — driven by dashjs-rs controllers
// ══════════════════════════════════════════════════════════════════════════════

/// Check whether pump_gen still matches (pump should exit if not).
fn is_pump_current(state: &Rc<RefCell<Option<DashPlayer>>>, pump_id: u32) -> bool {
    let borrow = state.borrow();
    matches!(borrow.as_ref(), Some(s) if s.pump_gen == pump_id)
}

/// Wait for SourceBuffer to finish updating.
/// Uses a small delay between checks to let the browser process events.
async fn wait_for_sb(
    sb: &web_sys::SourceBuffer,
    state: &Rc<RefCell<Option<DashPlayer>>>,
    pump_id: u32,
) -> bool {
    // Up to ~10 seconds (200 × 50ms)
    for _ in 0..200 {
        if !sb.updating() { return true; }
        if !is_pump_current(state, pump_id) { return false; }
        TimeoutFuture::new(50).await;
    }
    log::warn!("wait_for_sb: timed out after 10s");
    false
}

// ── Buffer pruning (matches dash.js BufferController wall-clock pruning) ─────

/// Get total buffered duration across all ranges.
fn get_total_buffered(video: &HtmlVideoElement) -> f64 {
    get_buffered_ranges(video).iter().map(|r| r.end - r.start).sum()
}

/// Inline eviction — called from pump_loop every PRUNING_INTERVAL_MS.
///
/// Matches dash.js BufferController._onWallclockTimeUpdated + pruneBuffer():
///   - Removes data behind (currentTime - BACK_BUFFER_S) matching bufferToKeep
///   - Only the pump_loop owns SourceBuffer mutations, avoiding races
///
/// Returns true if eviction was performed.
async fn evict_back_buffer(
    sb: &web_sys::SourceBuffer,
    video: &HtmlVideoElement,
    state: &Rc<RefCell<Option<DashPlayer>>>,
    pump_id: u32,
) -> bool {
    let current = video.current_time();
    let remove_end = current - BACK_BUFFER_S;
    if remove_end <= MIN_REMOVE_THRESHOLD_S { return false; }

    let ranges = get_buffered_ranges(video);
    if ranges.is_empty() { return false; }
    let buf_start = ranges[0].start;
    if buf_start >= remove_end { return false; }

    // Wait for any in-progress operation to finish first
    if sb.updating() {
        if !wait_for_sb(sb, state, pump_id).await { return false; }
    }

    if sb.remove(buf_start, remove_end).is_err() {
        log::warn!("eviction: remove({buf_start:.1}, {remove_end:.1}) failed");
        return false;
    }

    // Wait for remove to complete before pump continues
    if !wait_for_sb(sb, state, pump_id).await { return false; }

    // Track removal in dashjs-rs
    {
        let mut borrow = state.borrow_mut();
        if let Some(dp) = borrow.as_mut() {
            dp.engine.buffer_controller_mut().remove_data(buf_start, remove_end);
        }
    }

    true
}

/// Aggressive eviction on QuotaExceededError.
///
/// Matches dash.js BufferController._handleQuotaExceededError +
/// clearBuffers(getClearRanges()):
///   1. _handleQuotaExceededError sets criticalBufferLevel = totalBuffered*0.8
///   2. clearBuffers removes data behind currentTime - bufferToKeep
///      where bufferToKeep = max(0.2 * criticalBufferLevel, 1)
///
/// We remove everything from buffer start to (currentTime - QUOTA_KEEP_BEHIND_S)
/// which is more aggressive than normal pruning (BACK_BUFFER_S=20s).
async fn force_evict_for_quota(
    sb: &web_sys::SourceBuffer,
    video: &HtmlVideoElement,
    state: &Rc<RefCell<Option<DashPlayer>>>,
    pump_id: u32,
) -> bool {
    let current = video.current_time();
    let remove_end = (current - QUOTA_KEEP_BEHIND_S).max(0.1);
    if remove_end <= MIN_REMOVE_THRESHOLD_S { return false; }

    let ranges = get_buffered_ranges(video);
    if ranges.is_empty() { return false; }
    let buf_start = ranges[0].start;
    if buf_start >= remove_end { return false; }

    log::info!("force_evict: removing {buf_start:.1}..{remove_end:.1}s (currentTime={current:.1}s)");

    // Wait for any in-progress operation to finish first
    if sb.updating() {
        if !wait_for_sb(sb, state, pump_id).await { return false; }
    }

    if sb.remove(buf_start, remove_end).is_err() {
        log::warn!("force_evict: remove({buf_start:.1}, {remove_end:.1}) failed");
        return false;
    }

    if !wait_for_sb(sb, state, pump_id).await { return false; }

    // Track removal in dashjs-rs
    {
        let mut borrow = state.borrow_mut();
        if let Some(dp) = borrow.as_mut() {
            dp.engine.buffer_controller_mut().remove_data(buf_start, remove_end);
        }
    }

    true
}

/// Spawn background prefetch tasks for segments [next_seg+1, next_seg+LOOKAHEAD_WINDOW).
///
/// Mirrors dash.js `StreamProcessor._onMediaFragmentNeeded` + `FragmentController`
/// background loading pattern: start fetching the next N segments in the background
/// so they are already cached when the main pump loop needs them.
///
/// - Skips segments already in the cache (already fetched)
/// - Skips segments already in-flight (fetch in progress)
/// - Stores result bytes in `seg_cache` on completion
/// - Removes from `in_flight` when done (success or failure)
fn kick_prefetch(
    state: &Rc<RefCell<Option<DashPlayer>>>,
    pump_id: u32,
) {
    let (seg_cache, in_flight, urls): (SegmentCache, InFlightSet, Vec<(usize, String)>) = {
        let borrow = state.borrow();
        let dp = match borrow.as_ref() {
            Some(dp) if dp.pump_gen == pump_id => dp,
            _ => return,
        };
        let next = dp.next_seg;
        let total = dp.segments.len();
        let cache = dp.seg_cache.clone();
        let inflight = dp.in_flight.clone();

        // Collect URLs for segments we need to prefetch
        let cache_borrow = cache.borrow();
        let inflight_borrow = inflight.borrow();
        let lookahead_end = total.min(next.saturating_add(LOOKAHEAD_WINDOW + 1));
        let to_fetch: Vec<(usize, String)> = (next.saturating_add(1)..lookahead_end)
            .filter(|&i| !cache_borrow.contains_key(&i) && !inflight_borrow.contains(&i))
            .map(|i| (i, dp.segments[i].url.clone()))
            .collect();
        drop(cache_borrow);
        drop(inflight_borrow);

        (cache, inflight, to_fetch)
    };

    for (seg_idx, url) in urls {
        // Mark as in-flight before spawning
        in_flight.borrow_mut().insert(seg_idx);

        let cache_clone = seg_cache.clone();
        let inflight_clone = in_flight.clone();

        spawn_local(async move {
            let result = Request::get(&url).send().await;
            let bytes = match result {
                Ok(resp) if resp.ok() => resp.binary().await.ok(),
                _ => None,
            };

            // Store in cache if successful, always remove from in-flight
            if let Some(b) = bytes {
                if !b.is_empty() {
                    cache_clone.borrow_mut().insert(seg_idx, b);
                    log::debug!("prefetch: cached segment {seg_idx}");
                }
            } else {
                log::debug!("prefetch: failed to fetch segment {seg_idx}, will retry inline");
            }
            inflight_clone.borrow_mut().remove(&seg_idx);
        });
    }
}

/// Main pump loop — drives the segment fetch/append pipeline.
///
/// Mirrors dash.js StreamProcessor._onMediaFragmentNeeded:
///   - Uses ScheduleController._shouldBuffer() for buffer-level gating
///   - On QuotaExceeded: evicts behind playhead + retries
///   - Records throughput via ThroughputController
///   - Updates BufferController after each append
///   - Runs inline eviction every PRUNING_INTERVAL_MS (no separate timer)
async fn pump_loop(
    state: Rc<RefCell<Option<DashPlayer>>>,
    video: HtmlVideoElement,
    pump_id: u32,
) {
    let mut consecutive_failures: u32 = 0;

    loop {
        // ── 1. Check pump is still current ──
        if !is_pump_current(&state, pump_id) { return; }

        // ── 2. Check MediaSource is still open ──
        {
            let borrow = state.borrow();
            if let Some(dp) = borrow.as_ref() {
                if dp.media_source.ready_state() != web_sys::MediaSourceReadyState::Open {
                    log::info!("pump[{pump_id}]: MediaSource no longer open, exiting");
                    return;
                }
            } else { return; }
        }

        // ── 3. Inline eviction (matches dash.js _onWallclockTimeUpdated) ──
        // Runs every PRUNING_INTERVAL_MS (10s), gated by wall-clock.
        // Only pump_loop touches sb, avoiding races.
        {
            let should_evict = {
                let borrow = state.borrow();
                borrow.as_ref().map_or(false, |dp| {
                    js_sys::Date::now() - dp.last_eviction_ms >= PRUNING_INTERVAL_MS
                })
            };
            if should_evict {
                let sb = {
                    let borrow = state.borrow();
                    match borrow.as_ref() {
                        Some(dp) if dp.pump_gen == pump_id => dp.source_buffer.clone(),
                        _ => return,
                    }
                };
                evict_back_buffer(&sb, &video, &state, pump_id).await;
                // Update timestamp whether eviction ran or not
                if let Some(dp) = state.borrow_mut().as_mut() {
                    dp.last_eviction_ms = js_sys::Date::now();
                }
            }
        }

        // ── 4. Get next segment info ──
        let (seg_idx, seg_url) = {
            let borrow = state.borrow();
            let dp = match borrow.as_ref() {
                Some(dp) if dp.pump_gen == pump_id => dp,
                _ => return,
            };
            if dp.next_seg >= dp.segments.len() {
                // All segments done — signal end of stream
                if dp.media_source.ready_state() == web_sys::MediaSourceReadyState::Open {
                    let _ = dp.media_source.end_of_stream();
                    log::info!("pump[{pump_id}]: all {} segments appended, EOS", dp.segments.len());
                }
                return;
            }
            (dp.next_seg, dp.segments[dp.next_seg].url.clone())
        };

        // ── 5. Kick background prefetch for upcoming segments ──
        // Mirrors dash.js StreamProcessor._onMediaFragmentNeeded + FragmentController:
        // always keep LOOKAHEAD_WINDOW segments in-flight or cached, REGARDLESS of
        // whether should_schedule currently gates the main append.  Moving this
        // before the schedule gate is critical for post-seek recovery: after a seek
        // the seek-flush remove() is async, so get_buffer_level may still see old
        // data and should_schedule may return false for a few 500ms wait cycles.
        // Without prefetch running during those waits, every post-seek segment is
        // a cold inline fetch.
        kick_prefetch(&state, pump_id);

        // ── 6. Use dashjs-rs ScheduleController to decide if we should append ──
        // Mirrors dash.js ScheduleController._shouldBuffer():
        //   bufferLevel + segmentDuration < bufferTarget
        // Uses get_buffer_level() which matches dash.js _getBufferLength() with
        // 0.15s tolerance through getRangeAt().
        {
            let current_time = video.current_time();
            let buf_ahead = get_buffer_level(&video, current_time);

            let mut borrow = state.borrow_mut();
            if let Some(dp) = borrow.as_mut() {
                dp.engine.buffer_controller_mut().set_buffer_level(buf_ahead);

                if !dp.engine.schedule_controller().should_schedule(buf_ahead, SEGMENT_DURATION_F) {
                    drop(borrow);
                    // Buffer is full enough — wait before checking again
                    TimeoutFuture::new(500).await;
                    continue;
                }
            } else {
                return;
            }
        }

        // ── 7. Obtain segment bytes — from cache or inline fetch ──
        // Check the SegmentCache first: if a background prefetch already
        // has the bytes, use them immediately (no network wait).
        // This is the core fix for segment-transition stutter.
        let bytes: Vec<u8> = {
            // Try cache first
            let cached = {
                let borrow = state.borrow();
                borrow.as_ref().and_then(|dp| {
                    if dp.pump_gen == pump_id {
                        dp.seg_cache.borrow_mut().remove(&seg_idx)
                    } else {
                        None
                    }
                })
            };

            if let Some(b) = cached {
                log::debug!("pump[{pump_id}]: segment {seg_idx} served from prefetch cache");
                b
            } else {
                // Cache miss — fetch inline. fetch_start_ms is measured here so
                // only actual network fetches contribute to throughput estimation.
                let fetch_start_ms = js_sys::Date::now();
                let result = match Request::get(&seg_url).send().await {
                    Ok(resp) => {
                        if !resp.ok() {
                            log::error!("pump[{pump_id}]: segment {seg_idx} HTTP {}", resp.status());
                            consecutive_failures += 1;
                            if consecutive_failures > MAX_FETCH_FAILURES {
                                if let Some(dp) = state.borrow_mut().as_mut() {
                                    if dp.pump_gen == pump_id { dp.next_seg = seg_idx + 1; }
                                }
                                consecutive_failures = 0;
                            }
                            TimeoutFuture::new(1000).await;
                            continue;
                        }
                        match resp.binary().await {
                            Ok(b) => b,
                            Err(e) => {
                                log::error!("pump[{pump_id}]: segment {seg_idx} read error: {e:?}");
                                TimeoutFuture::new(1000).await;
                                continue;
                            }
                        }
                    },
                    Err(e) => {
                        log::error!("pump[{pump_id}]: segment {seg_idx} fetch error: {e:?}");
                        TimeoutFuture::new(1000).await;
                        continue;
                    }
                };

                // Record throughput for inline fetches only (cache hits don't reflect network speed)
                let elapsed_ms = (js_sys::Date::now() - fetch_start_ms).max(1.0);
                let throughput_bps = (result.len() as f64 * 8.0) / (elapsed_ms / 1000.0);
                {
                    let mut borrow = state.borrow_mut();
                    if let Some(dp) = borrow.as_mut() {
                        dp.engine.throughput_controller_mut().add_sample(throughput_bps);
                    }
                }
                result
            }
        };

        if !is_pump_current(&state, pump_id) { return; }

        // ── 8. Record throughput via dashjs-rs ThroughputController ──
        // (Already recorded above for inline fetches; skipped for cache hits)

        // ── 9. Append segment data ──
        if bytes.is_empty() {
            log::warn!("pump[{pump_id}]: segment {seg_idx} empty, skipping");
            if let Some(dp) = state.borrow_mut().as_mut() {
                if dp.pump_gen == pump_id { dp.next_seg = seg_idx + 1; }
            }
            continue;
        }

        // Wait for SourceBuffer to be ready (also waits for any pruning remove)
        let sb = {
            let borrow = state.borrow();
            match borrow.as_ref() {
                Some(dp) if dp.pump_gen == pump_id => dp.source_buffer.clone(),
                _ => return,
            }
        };

        if !wait_for_sb(&sb, &state, pump_id).await { return; }

        // Retry loop for append — does NOT re-fetch the segment data.
        // On QuotaExceeded, evicts behind playhead and retries the same bytes
        // (matching dash.js SourceBufferSink → BufferController._handleQuotaExceededError).
        //
        // Firefox robustness notes:
        //   1. The ArrayBuffer is recreated on every attempt.  Firefox (and the MSE spec)
        //      may transfer/detach the ArrayBuffer on a failed appendBuffer() call, so
        //      reusing the same `ab` across retries causes InvalidStateError on the second
        //      attempt.  Re-creating it from the original `bytes` slice is safe and cheap
        //      because retries are rare.
        //   2. The MediaSource readyState is checked immediately before each appendBuffer()
        //      call.  Firefox transitions the MediaSource to "ended"/"closed" more eagerly
        //      than Chrome; catching this here prevents the "object is no longer usable"
        //      InvalidStateError that would otherwise appear in the error log.
        let mut append_ok = false;
        let mut quota_retries: u32 = 0;
        loop {
            if !wait_for_sb(&sb, &state, pump_id).await { return; }

            // Guard: MediaSource must be Open before appendBuffer (Firefox strict check)
            {
                let borrow = state.borrow();
                match borrow.as_ref() {
                    Some(dp) if dp.pump_gen == pump_id => {
                        if dp.media_source.ready_state() != web_sys::MediaSourceReadyState::Open {
                            log::info!("pump[{pump_id}]: MediaSource closed, exiting");
                            return;
                        }
                    }
                    _ => return,
                }
            }

            // Rebuild ArrayBuffer on every attempt to avoid Firefox detachment issues
            let uint8 = js_sys::Uint8Array::from(bytes.as_slice());
            let ab = uint8.buffer();
            match sb.append_buffer_with_array_buffer(&ab) {
                Ok(()) => {
                    consecutive_failures = 0;
                    append_ok = true;
                    break;
                }
                Err(e) => {
                    let is_quota = e.dyn_ref::<web_sys::DomException>()
                        .map_or(false, |ex| ex.name() == "QuotaExceededError");

                    if is_quota {
                        quota_retries += 1;
                        if quota_retries <= MAX_QUOTA_RETRIES {
                            log::warn!("pump[{pump_id}]: QuotaExceededError on segment {seg_idx} \
                                (attempt {quota_retries}/{MAX_QUOTA_RETRIES}), evicting");
                            force_evict_for_quota(&sb, &video, &state, pump_id).await;
                            TimeoutFuture::new(200).await;
                            // Retry the append with the SAME bytes (no re-fetch)
                        } else {
                            log::error!("pump[{pump_id}]: QuotaExceeded persists after \
                                {MAX_QUOTA_RETRIES} evictions, skipping segment {seg_idx}");
                            break;
                        }
                    } else {
                        // Check for InvalidStateError specifically — this means the
                        // SourceBuffer is no longer usable (e.g. Firefox MEDIA_FATAL_ERR
                        // caused by H264 decode failure has detached the SourceBuffer
                        // from the MediaSource).  Unlike QuotaExceeded, this is NOT
                        // retriable — the entire MediaSource pipeline is dead.
                        let is_invalid_state = e.dyn_ref::<web_sys::DomException>()
                            .map_or(false, |ex| ex.name() == "InvalidStateError");

                        if is_invalid_state {
                            log::error!("pump[{pump_id}]: InvalidStateError on segment {seg_idx} — \
                                SourceBuffer detached (likely decode error), exiting");
                            // Mark as fatal so the periodic timer doesn't restart us
                            if let Some(dp) = state.borrow_mut().as_mut() {
                                dp.fatal_error = true;
                            }
                            return;
                        }

                        log::error!("pump[{pump_id}]: append failed for segment {seg_idx}: {:?}", e);
                        consecutive_failures += 1;
                        if consecutive_failures > MAX_APPEND_FAILURES {
                            log::error!("pump[{pump_id}]: too many consecutive failures, exiting");
                            // Mark as fatal after too many failures
                            if let Some(dp) = state.borrow_mut().as_mut() {
                                dp.fatal_error = true;
                            }
                            return;
                        }
                        TimeoutFuture::new(500).await;
                        break;
                    }
                }
            }
        }

        if !append_ok {
            // Skip this segment and move on
            if let Some(dp) = state.borrow_mut().as_mut() {
                if dp.pump_gen == pump_id { dp.next_seg = seg_idx + 1; }
            }
            continue;
        }

        // Wait for append to complete
        if !wait_for_sb(&sb, &state, pump_id).await { return; }

        // ── 10. Update dashjs-rs state ──
        {
            let mut borrow = state.borrow_mut();
            if let Some(dp) = borrow.as_mut() {
                if dp.pump_gen != pump_id { return; }
                dp.next_seg = seg_idx + 1;
                dp.last_appended_seg = Some(seg_idx);

                // Update dashjs-rs buffer controller with new append
                dp.engine.buffer_controller_mut().append_data(seg_idx as i64);

                // Update buffer level
                let buf_level = get_buffer_level(&video, video.current_time());
                dp.engine.buffer_controller_mut().set_buffer_level(buf_level);
            }
        }

        // Small yield to let browser process
        TimeoutFuture::new(10).await;
    }
}

/// Start the pump loop if not already running.
fn start_pump(state: &Rc<RefCell<Option<DashPlayer>>>, video: &HtmlVideoElement) {
    let pump_id = {
        let mut borrow = state.borrow_mut();
        let dp = match borrow.as_mut() {
            Some(dp) => dp,
            None => return,
        };
        if dp.pump_running || dp.fatal_error { return; }
        // Don't start if MediaSource is no longer open
        if dp.media_source.ready_state() != web_sys::MediaSourceReadyState::Open {
            return;
        }
        dp.pump_running = true;
        dp.pump_gen
    };

    let state = state.clone();
    let video = video.clone();
    spawn_local(async move {
        pump_loop(state.clone(), video, pump_id).await;
        if let Some(dp) = state.borrow_mut().as_mut() {
            if dp.pump_gen == pump_id { dp.pump_running = false; }
        }
    });
}

/// Force-start pump (cancels existing, increments gen).
fn force_start_pump(state: &Rc<RefCell<Option<DashPlayer>>>, video: &HtmlVideoElement) {
    {
        let mut borrow = state.borrow_mut();
        if let Some(dp) = borrow.as_mut() {
            dp.pump_gen = dp.pump_gen.wrapping_add(1);
            dp.pump_running = false;
            // Replace prefetch state with fresh instances on seek.
            // Background tasks hold Rc clones of the old instances; by
            // replacing them here, any in-flight prefetch writes go to the
            // old (now unreferenced) cache and are silently discarded.
            dp.seg_cache = Rc::new(RefCell::new(std::collections::HashMap::new()));
            dp.in_flight = Rc::new(RefCell::new(std::collections::HashSet::new()));
        }
    }
    start_pump(state, video);
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

    // DashPlayer state (Rc<RefCell<>> for async access)
    let dash_state = use_mut_ref(|| Option::<DashPlayer>::None);

    // ── Initialize dashjs-rs player ──────────────────────────────────────────
    {
        let video_ref = video_ref.clone();
        let status = status.clone();
        let error = error.clone();
        let thumbnail_info = thumbnail_info.clone();
        let thumbnail_image = thumbnail_image.clone();
        let subtitle_tracks = subtitle_tracks.clone();
        let dash_state = dash_state.clone();
        let selected_quality = selected_quality.clone();
        let resume_position = resume_position.clone();

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

                // Initialize dashjs-rs + MSE
                let video_ref_clone = video_ref.clone();
                let status_clone = status.clone();
                let error_clone = error.clone();
                let dash_state_clone = dash_state.clone();

                spawn_local(async move {
                    TimeoutFuture::new(50).await;

                    let video = match video_ref_clone.cast::<HtmlVideoElement>() {
                        Some(v) => v,
                        None => { error_clone.set(Some("Video element not found".into())); return; }
                    };

                    let manifest_url = format!("/api/videos/{}/manifest.mpd?quality={}", video_id, quality);

                    // Create MediaSource
                    let media_source = match web_sys::MediaSource::new() {
                        Ok(ms) => ms,
                        Err(_) => {
                            error_clone.set(Some("MSE not supported".into()));
                            return;
                        }
                    };

                    let object_url = match web_sys::Url::create_object_url_with_source(&media_source) {
                        Ok(u) => u,
                        Err(_) => {
                            error_clone.set(Some("Failed to create MediaSource URL".into()));
                            return;
                        }
                    };
                    video.set_src(&object_url);
                    status_clone.set("Loading stream…".to_string());

                    // sourceopen callback
                    let manifest_url_open = manifest_url.clone();
                    let video_open = video.clone();
                    let status_open = status_clone.clone();
                    let error_open = error_clone.clone();
                    let dash_state_open = dash_state_clone.clone();
                    let media_source_open = media_source.clone();
                    let object_url_open = object_url.clone();

                    let sourceopen_cb = Closure::once(Box::new(move || {
                        let manifest_url = manifest_url_open;
                        let video = video_open;
                        let status = status_open;
                        let error = error_open;
                        let dash_state = dash_state_open;
                        let media_source = media_source_open;
                        let object_url = object_url_open;

                        spawn_local(async move {
                            // Fetch MPD manifest
                            let resp = match Request::get(&manifest_url).send().await {
                                Ok(r) => r,
                                Err(e) => { error.set(Some(format!("Manifest fetch failed: {e:?}"))); return; }
                            };
                            let text = match resp.text().await {
                                Ok(t) => t,
                                Err(e) => { error.set(Some(format!("Manifest read failed: {e:?}"))); return; }
                            };

                            // Parse via dashjs-rs
                            let mpd_result = parse_mpd(&text);
                            if mpd_result.segments.is_empty() {
                                error.set(Some("Manifest contains no segments.".into()));
                                return;
                            }
                            if mpd_result.init_url.is_empty() {
                                error.set(Some("Manifest missing init segment URL.".into()));
                                return;
                            }
                            let init_url = mpd_result.init_url;
                            let total_duration = mpd_result.total_duration;
                            let segments = mpd_result.segments;

                            // ── Determine MIME+codecs for addSourceBuffer() ──
                            // 1:1 match of dash.js SourceBufferSink._getCodecStringForRepresentation():
                            //   `representation.mimeType + ';codecs="' + representation.codecs + '"'`
                            // Falls back to probing only when the MPD has no codecs attribute.
                            let mime: String = if let Some(ref codec_from_mpd) = mpd_result.mime_codec {
                                // Codec from MPD — verify the browser supports it
                                if web_sys::MediaSource::is_type_supported(codec_from_mpd) {
                                    codec_from_mpd.clone()
                                } else {
                                    log::warn!("MSE: MPD codec {codec_from_mpd} not supported, falling back to probe");
                                    probe_mime_type()
                                }
                            } else {
                                probe_mime_type()
                            };
                            log::info!("MSE: using MIME type: {mime}");

                            let source_buffer = match media_source.add_source_buffer(&mime) {
                                Ok(sb) => sb,
                                Err(e) => {
                                    error.set(Some(format!("Unsupported format ({e:?})")));
                                    return;
                                }
                            };

                            if total_duration > 0.0 {
                                media_source.set_duration(total_duration);
                            }

                            // ── Fetch & append init segment ──
                            // Matches dash.js HTTPLoader: check status 200-299, retry on failure.
                            let mut init_bytes: Option<Vec<u8>> = None;
                            for attempt in 0..MAX_FETCH_FAILURES {
                                let resp = match Request::get(&init_url).send().await {
                                    Ok(r) => r,
                                    Err(e) => {
                                        log::warn!("Init segment fetch error (attempt {}/{}): {e:?}",
                                            attempt + 1, MAX_FETCH_FAILURES);
                                        TimeoutFuture::new(1000).await;
                                        continue;
                                    }
                                };
                                if !resp.ok() {
                                    log::warn!("Init segment HTTP {} (attempt {}/{})",
                                        resp.status(), attempt + 1, MAX_FETCH_FAILURES);
                                    TimeoutFuture::new(1000).await;
                                    continue;
                                }
                                match resp.binary().await {
                                    Ok(b) if !b.is_empty() => {
                                        init_bytes = Some(b);
                                        break;
                                    }
                                    Ok(_) => {
                                        log::warn!("Init segment empty (attempt {}/{})",
                                            attempt + 1, MAX_FETCH_FAILURES);
                                        TimeoutFuture::new(1000).await;
                                    }
                                    Err(e) => {
                                        log::warn!("Init segment read error (attempt {}/{}): {e:?}",
                                            attempt + 1, MAX_FETCH_FAILURES);
                                        TimeoutFuture::new(1000).await;
                                    }
                                }
                            }
                            let init_bytes = match init_bytes {
                                Some(b) => b,
                                None => {
                                    error.set(Some("Failed to fetch init segment after retries.".into()));
                                    return;
                                }
                            };

                            let uint8 = js_sys::Uint8Array::from(init_bytes.as_slice());
                            let ab = uint8.buffer();
                            if source_buffer.append_buffer_with_array_buffer(&ab).is_err() {
                                error.set(Some("Failed to append init segment.".into()));
                                return;
                            }

                            // Wait for init append to complete.
                            // Also check readyState — Firefox may close the MediaSource
                            // if the init segment contains invalid codec data (e.g.
                            // malformed avcC → NS_ERROR_DOM_MEDIA_FATAL_ERR).
                            for _ in 0..200 {
                                if !source_buffer.updating() { break; }
                                TimeoutFuture::new(5).await;
                            }

                            // Verify MediaSource is still open after init append.
                            // If Firefox rejected the init segment (bad H264 parameters),
                            // the MediaSource transitions to "closed"/"ended" and all
                            // subsequent operations will fail with InvalidStateError.
                            if media_source.ready_state() != web_sys::MediaSourceReadyState::Open {
                                error.set(Some(
                                    "Init segment rejected by browser (codec/format mismatch). \
                                     Try clearing server cache or switching quality.".into()
                                ));
                                return;
                            }

                            let start_seg = if start_pos > 0.0 { segment_for_time(start_pos) } else { 0 };

                            // Create dashjs-rs MediaPlayer
                            let mut engine = DashMediaPlayer::create();
                            engine.initialize(Some(&manifest_url), false);

                            // Configure dashjs-rs controllers
                            engine.buffer_controller_mut().initialize("video", "stream0");
                            engine.buffer_controller_mut().set_buffer_level(0.0);
                            engine.schedule_controller_mut().initialize("video", "stream0", true);
                            engine.schedule_controller_mut().set_buffer_target(BUFFER_TARGET_S);
                            engine.schedule_controller_mut().start_scheduling();
                            engine.playback_controller_mut().initialize("stream0", false);
                            engine.playback_controller_mut().set_duration(total_duration);
                            engine.gap_controller_mut().initialize();

                            // Store DashPlayer
                            *dash_state.borrow_mut() = Some(DashPlayer {
                                engine,
                                media_source,
                                source_buffer: source_buffer.clone(),
                                object_url,
                                segments,
                                next_seg: start_seg,
                                pump_gen: 0,
                                pump_running: false,
                                last_appended_seg: None,
                                last_eviction_ms: js_sys::Date::now(),
                                seg_cache: Rc::new(RefCell::new(std::collections::HashMap::new())),
                                in_flight: Rc::new(RefCell::new(std::collections::HashSet::new())),
                                fatal_error: false,
                            });

                            status.set(String::new());
                            if start_pos > 0.0 {
                                video.set_current_time(start_pos);
                            }

                            start_pump(&dash_state, &video);

                            // Auto-play when the user opens the player
                            let _ = video.play();
                        });
                    }) as Box<dyn FnOnce()>);

                    // MUST use {once: true} to auto-remove after first fire.
                    // Without this, Closure::once panics with 'FnOnce called
                    // more than once' when MediaSource transitions ended→open
                    // on replay/seek after endOfStream().
                    let opts = web_sys::AddEventListenerOptions::new();
                    opts.set_once(true);
                    media_source
                        .add_event_listener_with_callback_and_add_event_listener_options(
                            "sourceopen",
                            sourceopen_cb.as_ref().unchecked_ref(),
                            &opts,
                        )
                        .ok();
                    sourceopen_cb.forget();
                });

                // Cleanup
                let dash_state_cleanup = dash_state.clone();
                let video_ref_cleanup = video_ref.clone();
                move || {
                    if let Some(dp) = dash_state_cleanup.borrow_mut().take() {
                        let _ = dp.media_source.end_of_stream();
                        let _ = web_sys::Url::revoke_object_url(&dp.object_url);
                        if let Some(video) = video_ref_cleanup.cast::<HtmlVideoElement>() {
                            video.set_src("");
                        }
                    }
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

    // ── Periodic time update + dashjs-rs gap controller + pump restart ────────
    {
        let video_ref = video_ref.clone();
        let current_time = current_time.clone();
        let duration = duration.clone();
        let buffered_end = buffered_end.clone();
        let is_playing = is_playing.clone();
        let is_dragging = is_dragging.clone();
        let video_ended = video_ended.clone();
        let dash_state = dash_state.clone();

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

                    // dashjs-rs GapController: detect & jump gaps
                    if !video.paused() && !video.ended() && !video.seeking() && video.ready_state() <= 2 {
                        let ranges = get_buffered_ranges(&video);
                        let ct = video.current_time();
                        let mut borrow = dash_state.borrow_mut();
                        if let Some(dp) = borrow.as_mut() {
                            dp.engine.playback_controller_mut().set_time(ct);
                            if let Some(jump_to) = dp.engine.gap_controller_mut().jump_gap(&ranges, ct) {
                                log::info!("dashjs-rs GapController: jumping to {jump_to:.3}s");
                                video.set_current_time(jump_to + 0.001);
                            }
                        }
                    }

                    // Pump restart safety net
                    let needs_restart = {
                        let borrow = dash_state.borrow();
                        if let Some(dp) = borrow.as_ref() {
                            // Never restart after a fatal error (e.g. H264
                            // decode failure that detached the SourceBuffer).
                            if dp.fatal_error || dp.pump_running || dp.next_seg >= dp.segments.len() {
                                false
                            } else {
                                let buf_ahead = buffered_end_at(&video, video.current_time()) - video.current_time();
                                buf_ahead < BUFFER_TARGET_S
                            }
                        } else { false }
                    };
                    if needs_restart {
                        start_pump(&dash_state, &video);
                    }
                }
            });
            move || drop(interval)
        });
    }

    // ── Buffering detection via waiting/playing events ───────────────────────
    {
        let video_ref = video_ref.clone();
        let is_buffering = is_buffering.clone();
        let dash_state = dash_state.clone();

        use_effect_with(video_ref.clone(), move |video_ref| {
            let video_opt = video_ref.cast::<HtmlVideoElement>();

            let waiting_cb = video_opt.as_ref().map(|video| {
                let is_buffering = is_buffering.clone();
                let video_for_gap = video.clone();
                let dash_state = dash_state.clone();
                let cb = Closure::<dyn Fn()>::new(move || {
                    // Try dashjs-rs gap jump first
                    if !video_for_gap.seeking() {
                        let ranges = get_buffered_ranges(&video_for_gap);
                        let ct = video_for_gap.current_time();
                        let mut borrow = dash_state.borrow_mut();
                        let jumped = if let Some(dp) = borrow.as_mut() {
                            dp.engine.gap_controller_mut().jump_gap(&ranges, ct).map(|t| {
                                video_for_gap.set_current_time(t + 0.001);
                            }).is_some()
                        } else { false };
                        drop(borrow);
                        if !jumped {
                            is_buffering.set(true);
                        }
                    } else {
                        is_buffering.set(true);
                    }
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

    // ── Seek handling — uses dashjs-rs PlaybackController ────────────────────
    {
        let video_ref = video_ref.clone();
        let dash_state = dash_state.clone();

        use_effect_with(video_ref.clone(), move |video_ref| {
            let video_opt = video_ref.cast::<HtmlVideoElement>();

            let seeking_cb = video_opt.as_ref().map(|video| {
                let dash_state = dash_state.clone();
                let video_for_seek = video.clone();

                let cb = Closure::<dyn Fn()>::new(move || {
                    let seek_time = video_for_seek.current_time();
                    let target_seg = segment_for_time(seek_time);

                    // Update dashjs-rs playback controller
                    {
                        let mut borrow = dash_state.borrow_mut();
                        if let Some(dp) = borrow.as_mut() {
                            dp.engine.playback_controller_mut().seek(seek_time);
                            dp.engine.buffer_controller_mut().set_seek_target(Some(seek_time));
                        }
                    }

                    if is_time_buffered(&video_for_seek, seek_time) {
                        let need_pump = {
                            let borrow = dash_state.borrow();
                            borrow.as_ref().map_or(false, |dp| !dp.pump_running && !dp.fatal_error)
                        };
                        if need_pump { start_pump(&dash_state, &video_for_seek); }
                    } else {
                        log::info!("seek: target {seek_time:.1}s not buffered, restarting from segment {target_seg}");
                        {
                            let mut borrow = dash_state.borrow_mut();
                            if let Some(dp) = borrow.as_mut() {
                                dp.pump_gen = dp.pump_gen.wrapping_add(1);
                                dp.pump_running = false;
                                dp.next_seg = target_seg;
                                dp.last_appended_seg = None;
                                dp.last_eviction_ms = js_sys::Date::now();
                                // Clear fatal_error on seek — user is retrying.
                                dp.fatal_error = false;
                                // Abort any in-progress SourceBuffer operations.
                                // Matches dash.js BufferController.prepareForPlaybackSeek()
                                // which calls sourceBufferSink.abort() to cancel pending
                                // appends.  dash.js does NOT flush the entire buffer on
                                // seek — it relies on MSE's coded-frame-removal algorithm
                                // to handle overlapping data when new segments are appended.
                                let _ = dp.source_buffer.abort();
                            }
                        }

                        // ── Do NOT flush the entire SourceBuffer ──
                        // dash.js keeps existing buffered data and just starts
                        // fetching from the seek target.  Flushing removes all
                        // buffered data, creating a visible blank/freeze until
                        // the first new segment is fetched and appended.
                        //
                        // The browser's MSE implementation handles the overlap:
                        // new segments will replace any stale data at the same
                        // presentation time via the coded-frame-removal algorithm
                        // (MSE spec §3.5.1 Coded Frame Processing).

                        force_start_pump(&dash_state, &video_for_seek);
                    }

                    // Auto-play after seek (matches dash.js PlaybackController
                    // which resumes playback on seek, including after ended state)
                    if video_for_seek.paused() {
                        let _ = video_for_seek.play();
                    }
                });

                video.add_event_listener_with_callback("seeking", cb.as_ref().unchecked_ref()).ok();
                cb
            });

            move || {
                if let (Some(cb), Some(video)) = (seeking_cb, video_opt) {
                    video.remove_event_listener_with_callback("seeking", cb.as_ref().unchecked_ref()).ok();
                    drop(cb);
                }
            }
        });
    }

    // ── Server integration: WebSocket for playback state reporting ────────────
    // Connects to /api/player/ws and reports playback position every 2s.
    // Also fetches initial resume position from /api/player/position/{id}.
    // The server can send ServerCommand messages back to control playback.
    {
        let video_ref = video_ref.clone();
        let _dash_state = dash_state.clone();

        use_effect_with(props.video_id.clone(), move |video_id| {
            let video_id = video_id.clone();
            let video_ref = video_ref.clone();

            // Fetch resume position from server on mount
            let video_id_resume = video_id.clone();
            let video_ref_resume = video_ref.clone();
            spawn_local(async move {
                let url = format!("/api/player/position/{}", video_id_resume);
                if let Ok(resp) = Request::get(&url).send().await {
                    if resp.ok() {
                        if let Ok(json) = resp.json::<serde_json::Value>().await {
                            if let Some(time) = json.get("time").and_then(|t| t.as_f64()) {
                                if time > 1.0 {
                                    // Wait for video element and MSE to be ready
                                    TimeoutFuture::new(500).await;
                                    if let Some(video) = video_ref_resume.cast::<HtmlVideoElement>() {
                                        let dur = video.duration();
                                        if dur.is_finite() && time < dur - 5.0 {
                                            video.set_current_time(time);
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
                    if ws.ready_state() == 1 { // OPEN
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
                let onmessage = Closure::<dyn Fn(web_sys::MessageEvent)>::new(move |e: web_sys::MessageEvent| {
                    if let Some(text) = e.data().as_string() {
                        if let Ok(cmd) = serde_json::from_str::<ServerCommand>(&text) {
                            if let Some(video) = video_ref_cmd.cast::<HtmlVideoElement>() {
                                apply_server_command(&video, &cmd);
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
                        if video.paused() { let _ = video.play(); } else { let _ = video.pause(); }
                    }
                    "ArrowLeft" => {
                        e.prevent_default();
                        let skip = if e.shift_key() { 10.0 } else { 5.0 };
                        video.set_current_time((video.current_time() - skip).max(0.0));
                        skip_indicator.set(Some(("backward".to_string(), 25.0)));
                        let si = skip_indicator.clone();
                        spawn_local(async move { TimeoutFuture::new(500).await; si.set(None); });
                    }
                    "j" | "J" => {
                        e.prevent_default();
                        video.set_current_time((video.current_time() - 10.0).max(0.0));
                        skip_indicator.set(Some(("backward".to_string(), 25.0)));
                        let si = skip_indicator.clone();
                        spawn_local(async move { TimeoutFuture::new(500).await; si.set(None); });
                    }
                    "ArrowRight" => {
                        e.prevent_default();
                        let skip = if e.shift_key() { 10.0 } else { 5.0 };
                        let dur = video.duration();
                        if dur.is_finite() { video.set_current_time((video.current_time() + skip).min(dur)); }
                        skip_indicator.set(Some(("forward".to_string(), 75.0)));
                        let si = skip_indicator.clone();
                        spawn_local(async move { TimeoutFuture::new(500).await; si.set(None); });
                    }
                    "l" | "L" => {
                        e.prevent_default();
                        let dur = video.duration();
                        if dur.is_finite() { video.set_current_time((video.current_time() + 10.0).min(dur)); }
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
                        if dur.is_finite() { video.set_current_time(dur * (num / 10.0)); }
                    }
                    "<" | "," => {
                        e.prevent_default();
                        let current = *playback_speed;
                        if let Some(pos) = PLAYBACK_SPEEDS.iter().position(|&s| (s - current).abs() < 0.01) {
                            if pos > 0 { let ns = PLAYBACK_SPEEDS[pos - 1]; playback_speed.set(ns); video.set_playback_rate(ns); }
                        }
                    }
                    ">" | "." => {
                        e.prevent_default();
                        let current = *playback_speed;
                        if let Some(pos) = PLAYBACK_SPEEDS.iter().position(|&s| (s - current).abs() < 0.01) {
                            if pos < PLAYBACK_SPEEDS.len() - 1 { let ns = PLAYBACK_SPEEDS[pos + 1]; playback_speed.set(ns); video.set_playback_rate(ns); }
                        }
                    }
                    "Home" => { e.prevent_default(); video.set_current_time(0.0); }
                    "End" => { e.prevent_default(); let dur = video.duration(); if dur.is_finite() { video.set_current_time(dur); } }
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
        Callback::from(move |_| {
            if let Some(video) = video_ref.cast::<HtmlVideoElement>() {
                if *video_ended { video.set_current_time(0.0); }
                if video.paused() { let _ = video.play(); } else { let _ = video.pause(); }
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
        Callback::from(move |speed: f64| {
            playback_speed.set(speed);
            speed_menu_open.set(false);
            if let Some(video) = video_ref.cast::<HtmlVideoElement>() {
                video.set_playback_rate(speed);
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
                if let Some(video) = video_ref_up.cast::<HtmlVideoElement>() { video.set_current_time(t); }
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
        Callback::from(move |e: MouseEvent| {
            if *just_dragged { just_dragged.set(false); return; }
            if let Some(el) = progress_ref.cast::<web_sys::HtmlElement>() {
                if let Some(video) = video_ref.cast::<HtmlVideoElement>() {
                    if let Some((t, _)) = calculate_seek_time(&e, &el, video.duration()) {
                        video.set_current_time(t);
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
        Callback::from(move |e: MouseEvent| {
            let now = js_sys::Date::now();
            let x = e.client_x() as f64;
            if now - *last_tap_time < 300.0 {
                if let Some(video) = video_ref.cast::<HtmlVideoElement>() {
                    let rect = video.get_bounding_client_rect();
                    let w = rect.width();
                    let rx = x - rect.left();
                    if rx < w / 3.0 {
                        video.set_current_time((video.current_time() - 10.0).max(0.0));
                        skip_indicator.set(Some(("backward".to_string(), 25.0)));
                        let si = skip_indicator.clone();
                        spawn_local(async move { TimeoutFuture::new(500).await; si.set(None); });
                    } else if rx > w * 2.0 / 3.0 {
                        let dur = video.duration();
                        if dur.is_finite() { video.set_current_time((video.current_time() + 10.0).min(dur)); }
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
                spawn_local(async move {
                    TimeoutFuture::new(300).await;
                    if *last_tap_time != 0.0 {
                        if let Some(video) = video_ref.cast::<HtmlVideoElement>() {
                            if video.paused() { let _ = video.play(); } else { let _ = video.pause(); }
                        }
                    }
                });
            }
        })
    };

    let on_replay = {
        let video_ref = video_ref.clone();
        Callback::from(move |_| {
            if let Some(video) = video_ref.cast::<HtmlVideoElement>() {
                video.set_current_time(0.0);
                let _ = video.play();
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
