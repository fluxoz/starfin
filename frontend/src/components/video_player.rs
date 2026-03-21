// New DASH infrastructure types are additive — suppress warnings for code
// that will be consumed as the player evolves.

use gloo_net::http::Request;
use gloo_timers::callback::Interval;
use gloo_timers::future::TimeoutFuture;
use serde::Deserialize;
use std::cell::{Cell, RefCell};
use std::collections::HashMap;
use std::rc::Rc;
use wasm_bindgen::prelude::*;
use wasm_bindgen::JsCast;
use wasm_bindgen_futures::spawn_local;
use web_sys::{window, HtmlVideoElement, KeyboardEvent, MouseEvent};
use yew::prelude::*;

// ── Playback speed options ───────────────────────────────────────────────────
const PLAYBACK_SPEEDS: [f64; 9] = [0.25, 0.5, 0.75, 1.0, 1.25, 1.5, 1.75, 2.0, 3.0];

// ── Seek-anchor constants ────────────────────────────────────────────────────
// Segment duration must stay in sync with SEGMENT_DURATION in `src/main.rs`.
const SEGMENT_DURATION_F: f64 = 6.0;

/// Compute the segment index for a given time position.
fn segment_for_time(t: f64) -> usize {
    if t <= 0.0 { 0 } else { (t / SEGMENT_DURATION_F) as usize }
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
//
// These constants mirror the defaults used by well-known DASH/MSE reference
// implementations:
//
//  • dash.js (DASH-IF reference client)
//    ScheduleController — bufferTimeDefault = 12 s, stableBufferTime = 12 s,
//    bufferToKeep = 20 s.  Buffer gate: fetch when
//    `bufferLevel + segmentDuration < bufferTarget`.
//    Source: https://github.com/Dash-Industry-Forum/dash.js
//    Docs:   https://dashif.org/dash.js/pages/usage/buffer-management.html
//
//  • Shaka Player (Google)
//    StreamingEngine — bufferingGoal = 10 s, bufferBehind = 30 s,
//    rebufferingGoal = 2 s.  Buffer gate via `getBufferAhead_()`.
//    Source: https://github.com/shaka-project/shaka-player
//    Docs:   https://shaka-player-demo.appspot.com/docs/api/tutorial-network-and-buffering-config.html
//
//  • hls.js (video-dev)
//    BufferController — maxBufferLength = 30 s, maxBufferHole = 0.1 s,
//    backBufferLength = 30 s.
//    Source: https://github.com/video-dev/hls.js
//    Docs:   https://github.com/video-dev/hls.js/blob/master/docs/API.md
//
//  • DASH-IF IOP v4.3 §3.2.3 (buffer model), §3.2.4 (seeking), §3.2.8
//    (buffer management & eviction).
//    Spec:   https://dashif.org/docs/DASH-IF-IOP-v4.3.pdf

/// Target seconds of video to keep buffered ahead of the playback position.
/// dash.js uses 12 s by default (`bufferTimeAtTopQuality`);
/// Shaka Player uses 10 s; hls.js uses 30 s.
/// We use 30 s for comfortable VOD buffering.
const MSE_TARGET_BUFFER_S: f64 = 30.0;

/// Maximum seconds of already-played data to keep behind the playhead.
/// Data older than this is evicted via `SourceBuffer.remove()` to bound
/// memory usage and prevent the browser from hitting its SourceBuffer
/// quota (which triggers emergency eviction near the playhead — the
/// root cause of audio dropout at segment boundaries).
///
/// dash.js `bufferToKeep` defaults to 20 s; Shaka Player `bufferBehind`
/// defaults to 30 s.  We use 20 s to match dash.js and keep total buffer
/// size well within browser quotas.
const MSE_BACK_BUFFER_S: f64 = 20.0;

/// Maximum gap size (in seconds) that will be automatically jumped over.
/// dash.js GapController uses `smallGapLimit = 0.8` by default.
/// Shaka Player uses 0.5 s.  We use 0.8 to match dash.js.
/// Ref: dash.js/src/streaming/controllers/GapController.js `_jumpGap()`
const SMALL_GAP_LIMIT_S: f64 = 0.8;

/// Tolerance (in seconds) for matching the playhead to a buffered range.
/// A small tolerance prevents false negatives when the playhead sits at
/// the exact edge of a buffered range due to floating-point imprecision.
const PLAYHEAD_RANGE_TOLERANCE_S: f64 = 0.1;

/// Minimum amount of data (in seconds) worth evicting.  Avoids issuing
/// tiny SourceBuffer.remove() calls that add overhead without benefit.
const MIN_EVICT_S: f64 = 0.5;

/// Number of segments to keep pre-fetched ahead of the current append
/// position.  Background fetch tasks populate a shared segment cache so
/// that the sequential append loop never blocks on HTTP latency.
///
/// This is critical when segments are generated on-demand by the backend —
/// the first fetch of each segment triggers server-side muxing which can
/// take hundreds of milliseconds.  With deep prefetch, those generations
/// happen well before the data is needed.
///
/// dash.js achieves this implicitly via CDN pre-segmented content; our
/// on-demand backend requires explicit lookahead.
const LOOKAHEAD_SEGMENTS: usize = 5;

// ══════════════════════════════════════════════════════════════════════════════
// NEW DASH INFRASTRUCTURE — dash.js parity additions.
// These types and functions are purely additive — the existing playback
// pipeline (MseState, pump_loop, seeking, gap-jumping) is UNCHANGED.
// ══════════════════════════════════════════════════════════════════════════════

// ── ABR / schedule / live constants ──────────────────────────────────────────
const ABR_BANDWIDTH_SAFETY_FACTOR: f64 = 0.9;
const EWMA_HALF_LIFE_FAST: f64 = 3.0;
const EWMA_HALF_LIFE_SLOW: f64 = 8.0;
const ABR_MIN_SAMPLES: usize = 2;
const DROPPED_FRAMES_MIN_SAMPLE: u32 = 300;
const DROPPED_FRAMES_THRESHOLD: f64 = 0.15;
const SWITCH_HISTORY_SAMPLE_SIZE: usize = 8;
const SWITCH_HISTORY_THRESHOLD: f64 = 0.075;
const ABANDON_DURATION_MULTIPLIER: f64 = 1.8;
const BOLA_MINIMUM_BUFFER_S: f64 = 10.0;
const BOLA_MINIMUM_BUFFER_PER_LEVEL_S: f64 = 2.0;
const BOLA_PLACEHOLDER_DECAY: f64 = 0.99;
const STABLE_BUFFER_TIME_S: f64 = 12.0;
const BUFFER_TIME_DEFAULT_S: f64 = 12.0;
const QUOTA_EXCEEDED_ERR_CODE: u16 = 22;
const LIVE_DEFAULT_PRESENTATION_DELAY_S: f64 = 4.0;
const LIVE_CATCHUP_RATE_MAX: f64 = 0.5;
const LIVE_CATCHUP_RATE_MIN: f64 = -0.5;
const LIVE_MPD_REFRESH_INTERVAL_MS: u32 = 5000;
const FAST_SWITCH_ENABLED: bool = true;

// ── §1 Event System — mirrors MediaPlayerEvents.js ───────────────────────────

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum PlayerEvent {
    ManifestLoadingStarted, ManifestLoadingFinished, ManifestLoaded,
    ManifestValidityChanged,
    StreamInitializing, StreamInitialized, StreamUpdated,
    StreamActivated, StreamDeactivated, StreamTeardownComplete,
    PeriodSwitchStarted, PeriodSwitchCompleted,
    QualityChangeRequested, QualityChangeRendered,
    RepresentationSwitch, AdaptationSetRemovedNoCapabilities,
    BufferEmpty, BufferLoaded, BufferLevelStateChanged, BufferLevelUpdated,
    FragmentLoadingStarted, FragmentLoadingCompleted,
    FragmentLoadingProgress, FragmentLoadingAbandoned,
    PlaybackPlaying, PlaybackPaused, PlaybackSeeking, PlaybackSeeked,
    PlaybackStarted, PlaybackTimeUpdated, PlaybackProgress,
    PlaybackRateChanged, PlaybackEnded, PlaybackWaiting, PlaybackStalled,
    PlaybackNotAllowed, PlaybackError,
    PlaybackMetadataLoaded, PlaybackLoadedData,
    PlaybackInitialized, PlaybackVolumeChanged,
    MetricsChanged, MetricChanged, MetricAdded, MetricUpdated,
    ThroughputMeasurementStored,
    NewTrackSelected, TrackChangeRendered,
    TextTracksAdded, TextTrackAdded, CueEnter, CueExit, CaptionRendered,
    CanPlay, CanPlayThrough, Error, Log,
    DynamicToStatic, AstInFuture, BaseUrlsUpdated, InbandPrft,
    ManagedMediaSourceStartStreaming, ManagedMediaSourceEndStreaming,
}

impl PlayerEvent {
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::ManifestLoadingStarted => "manifestLoadingStarted",
            Self::ManifestLoadingFinished => "manifestLoadingFinished",
            Self::ManifestLoaded => "manifestLoaded",
            Self::ManifestValidityChanged => "manifestValidityChanged",
            Self::StreamInitializing => "streamInitializing",
            Self::StreamInitialized => "streamInitialized",
            Self::StreamUpdated => "streamUpdated",
            Self::StreamActivated => "streamActivated",
            Self::StreamDeactivated => "streamDeactivated",
            Self::StreamTeardownComplete => "streamTeardownComplete",
            Self::PeriodSwitchStarted => "periodSwitchStarted",
            Self::PeriodSwitchCompleted => "periodSwitchCompleted",
            Self::QualityChangeRequested => "qualityChangeRequested",
            Self::QualityChangeRendered => "qualityChangeRendered",
            Self::RepresentationSwitch => "representationSwitch",
            Self::AdaptationSetRemovedNoCapabilities => "adaptationSetRemovedNoCapabilities",
            Self::BufferEmpty => "bufferStalled",
            Self::BufferLoaded => "bufferLoaded",
            Self::BufferLevelStateChanged => "bufferStateChanged",
            Self::BufferLevelUpdated => "bufferLevelUpdated",
            Self::FragmentLoadingStarted => "fragmentLoadingStarted",
            Self::FragmentLoadingCompleted => "fragmentLoadingCompleted",
            Self::FragmentLoadingProgress => "fragmentLoadingProgress",
            Self::FragmentLoadingAbandoned => "fragmentLoadingAbandoned",
            Self::PlaybackPlaying => "playbackPlaying",
            Self::PlaybackPaused => "playbackPaused",
            Self::PlaybackSeeking => "playbackSeeking",
            Self::PlaybackSeeked => "playbackSeeked",
            Self::PlaybackStarted => "playbackStarted",
            Self::PlaybackTimeUpdated => "playbackTimeUpdated",
            Self::PlaybackProgress => "playbackProgress",
            Self::PlaybackRateChanged => "playbackRateChanged",
            Self::PlaybackEnded => "playbackEnded",
            Self::PlaybackWaiting => "playbackWaiting",
            Self::PlaybackStalled => "playbackStalled",
            Self::PlaybackNotAllowed => "playbackNotAllowed",
            Self::PlaybackError => "playbackError",
            Self::PlaybackMetadataLoaded => "playbackMetaDataLoaded",
            Self::PlaybackLoadedData => "playbackLoadedData",
            Self::PlaybackInitialized => "playbackInitialized",
            Self::PlaybackVolumeChanged => "playbackVolumeChanged",
            Self::MetricsChanged => "metricsChanged",
            Self::MetricChanged => "metricChanged",
            Self::MetricAdded => "metricAdded",
            Self::MetricUpdated => "metricUpdated",
            Self::ThroughputMeasurementStored => "throughputMeasurementStored",
            Self::NewTrackSelected => "newTrackSelected",
            Self::TrackChangeRendered => "trackChangeRendered",
            Self::TextTracksAdded => "allTextTracksAdded",
            Self::TextTrackAdded => "textTrackAdded",
            Self::CueEnter => "cueEnter",
            Self::CueExit => "cueExit",
            Self::CaptionRendered => "captionRendered",
            Self::CanPlay => "canPlay",
            Self::CanPlayThrough => "canPlayThrough",
            Self::Error => "error",
            Self::Log => "log",
            Self::DynamicToStatic => "dynamicToStatic",
            Self::AstInFuture => "astInFuture",
            Self::BaseUrlsUpdated => "baseUrlsUpdated",
            Self::InbandPrft => "inbandPrft",
            Self::ManagedMediaSourceStartStreaming => "managedMediaSourceStartStreaming",
            Self::ManagedMediaSourceEndStreaming => "managedMediaSourceEndStreaming",
        }
    }
}

#[derive(Debug, Clone)]
pub struct EventPayload { pub data: serde_json::Value }
impl Default for EventPayload { fn default() -> Self { Self { data: serde_json::Value::Null } } }

type EventCallback = Rc<dyn Fn(&EventPayload)>;

#[derive(Clone)]
pub struct EventBus {
    listeners: Rc<RefCell<HashMap<PlayerEvent, Vec<EventCallback>>>>,
}
impl EventBus {
    pub fn new() -> Self { Self { listeners: Rc::new(RefCell::new(HashMap::new())) } }
    pub fn on(&self, event: PlayerEvent, cb: EventCallback) {
        self.listeners.borrow_mut().entry(event).or_default().push(cb);
    }
    pub fn off(&self, event: PlayerEvent) { self.listeners.borrow_mut().remove(&event); }
    pub fn emit(&self, event: PlayerEvent, payload: &EventPayload) {
        let listeners = self.listeners.borrow();
        if let Some(cbs) = listeners.get(&event) { for cb in cbs { cb(payload); } }
    }
    pub fn emit_simple(&self, event: PlayerEvent) { self.emit(event, &EventPayload::default()); }
}

// ── §2 Metrics — DashMetrics.js ──────────────────────────────────────────────

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum MediaType { Video, Audio }

#[derive(Debug, Clone)]
pub struct ThroughputSample {
    pub timestamp_ms: f64, pub throughput_kbps: f64, pub latency_ms: f64,
    pub bytes: usize, pub duration_ms: f64, pub media_type: MediaType,
}

#[derive(Debug, Clone)]
pub struct BufferLevelEntry { pub timestamp_ms: f64, pub level_s: f64, pub media_type: MediaType }
#[derive(Debug, Clone, Default)]
pub struct DroppedFrameEntry { pub total_frames: u32, pub dropped_frames: u32, pub timestamp_ms: f64 }
#[derive(Debug, Clone, Default)]
pub struct SwitchHistoryEntry { pub drops: usize, pub no_drops: usize }
#[derive(Debug, Clone)]
pub struct LatencyEntry { pub timestamp_ms: f64, pub latency_s: f64 }
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BufferState { Loaded, Empty }

#[derive(Debug, Clone)]
pub struct DashMetrics {
    pub throughput_history: Vec<ThroughputSample>,
    pub buffer_levels: Vec<BufferLevelEntry>,
    pub dropped_frames: DroppedFrameEntry,
    pub switch_history: HashMap<usize, SwitchHistoryEntry>,
    pub latency_history: Vec<LatencyEntry>,
    pub current_buffer_state: HashMap<MediaType, BufferState>,
}
impl DashMetrics {
    pub fn new() -> Self {
        let mut cbs = HashMap::new();
        cbs.insert(MediaType::Video, BufferState::Empty);
        cbs.insert(MediaType::Audio, BufferState::Empty);
        Self { throughput_history: Vec::new(), buffer_levels: Vec::new(),
            dropped_frames: DroppedFrameEntry::default(), switch_history: HashMap::new(),
            latency_history: Vec::new(), current_buffer_state: cbs }
    }
    pub fn add_throughput_sample(&mut self, s: ThroughputSample) {
        self.throughput_history.push(s);
        if self.throughput_history.len() > 100 { self.throughput_history.drain(..self.throughput_history.len() - 100); }
    }
    pub fn add_buffer_level(&mut self, e: BufferLevelEntry) {
        self.buffer_levels.push(e);
        if self.buffer_levels.len() > 200 { self.buffer_levels.drain(..self.buffer_levels.len() - 200); }
    }
    pub fn update_dropped_frames(&mut self, total: u32, dropped: u32) {
        self.dropped_frames = DroppedFrameEntry { total_frames: total, dropped_frames: dropped, timestamp_ms: js_sys::Date::now() };
    }
    pub fn set_buffer_state(&mut self, mt: MediaType, st: BufferState) { self.current_buffer_state.insert(mt, st); }
    pub fn get_buffer_state(&self, mt: MediaType) -> BufferState { self.current_buffer_state.get(&mt).copied().unwrap_or(BufferState::Empty) }
    pub fn record_switch(&mut self, idx: usize, was_drop: bool) {
        let e = self.switch_history.entry(idx).or_default();
        if was_drop { e.drops += 1; } else { e.no_drops += 1; }
    }
    pub fn add_latency(&mut self, e: LatencyEntry) {
        self.latency_history.push(e);
        if self.latency_history.len() > 100 { self.latency_history.drain(..self.latency_history.len() - 100); }
    }
    pub fn current_buffer_level(&self, mt: MediaType) -> f64 {
        self.buffer_levels.iter().rev().find(|e| e.media_type == mt).map(|e| e.level_s).unwrap_or(0.0)
    }
    pub fn get_average_throughput(&self, mt: MediaType) -> f64 {
        let s: Vec<_> = self.throughput_history.iter().filter(|s| s.media_type == mt).collect();
        if s.is_empty() { 0.0 } else { s.iter().map(|s| s.throughput_kbps).sum::<f64>() / s.len() as f64 }
    }
}

// ── §3 Throughput Controller — ThroughputController.js ───────────────────────

#[derive(Debug, Clone)]
struct EwmaState { total_weight: f64, fast_estimate: f64, slow_estimate: f64 }
impl EwmaState {
    fn new() -> Self { Self { total_weight: 0.0, fast_estimate: 0.0, slow_estimate: 0.0 } }
    fn add_sample(&mut self, value: f64) {
        let w = 1.0;
        let af = 1.0 - 0.5_f64.powf(w / EWMA_HALF_LIFE_FAST);
        let as_ = 1.0 - 0.5_f64.powf(w / EWMA_HALF_LIFE_SLOW);
        self.fast_estimate = af * value + (1.0 - af) * self.fast_estimate;
        self.slow_estimate = as_ * value + (1.0 - as_) * self.slow_estimate;
        self.total_weight += w;
    }
    fn get_estimate(&self, use_min: bool) -> f64 {
        if self.total_weight <= 0.0 { return f64::NAN; }
        let cf = 1.0 - 0.5_f64.powf(self.total_weight / EWMA_HALF_LIFE_FAST);
        let cs = 1.0 - 0.5_f64.powf(self.total_weight / EWMA_HALF_LIFE_SLOW);
        let fast = self.fast_estimate / cf;
        let slow = self.slow_estimate / cs;
        if use_min { fast.min(slow) } else { fast.max(slow) }
    }
}

#[derive(Debug, Clone)]
pub struct ThroughputController {
    throughput_ewma: HashMap<MediaType, EwmaState>,
    latency_ewma: HashMap<MediaType, EwmaState>,
    sample_count: HashMap<MediaType, usize>,
}
impl ThroughputController {
    pub fn new() -> Self {
        let mut te = HashMap::new(); let mut le = HashMap::new(); let mut sc = HashMap::new();
        for mt in [MediaType::Video, MediaType::Audio] {
            te.insert(mt, EwmaState::new()); le.insert(mt, EwmaState::new()); sc.insert(mt, 0);
        }
        Self { throughput_ewma: te, latency_ewma: le, sample_count: sc }
    }
    pub fn add_measurement(&mut self, mt: MediaType, throughput_kbps: f64, latency_ms: f64) {
        if let Some(e) = self.throughput_ewma.get_mut(&mt) { e.add_sample(throughput_kbps); }
        if let Some(e) = self.latency_ewma.get_mut(&mt) { e.add_sample(latency_ms); }
        *self.sample_count.entry(mt).or_insert(0) += 1;
    }
    pub fn get_safe_average_throughput(&self, mt: MediaType) -> f64 {
        let avg = self.throughput_ewma.get(&mt).map(|e| e.get_estimate(true)).unwrap_or(f64::NAN);
        if avg.is_nan() { f64::NAN } else { avg * ABR_BANDWIDTH_SAFETY_FACTOR }
    }
    pub fn get_average_throughput(&self, mt: MediaType) -> f64 {
        self.throughput_ewma.get(&mt).map(|e| e.get_estimate(true)).unwrap_or(f64::NAN)
    }
    pub fn get_average_latency(&self, mt: MediaType) -> f64 {
        self.latency_ewma.get(&mt).map(|e| e.get_estimate(false)).unwrap_or(f64::NAN)
    }
    pub fn get_sample_count(&self, mt: MediaType) -> usize { self.sample_count.get(&mt).copied().unwrap_or(0) }
    pub fn reset(&mut self) {
        for mt in [MediaType::Video, MediaType::Audio] {
            self.throughput_ewma.insert(mt, EwmaState::new());
            self.latency_ewma.insert(mt, EwmaState::new());
            self.sample_count.insert(mt, 0);
        }
    }
}

// ── §4 Full MPD Parser — DashParser.js + DashAdapter.js ──────────────────────

#[derive(Debug, Clone)]
pub struct Mpd {
    pub mpd_type: MpdType, pub media_presentation_duration: f64,
    pub min_buffer_time: f64, pub availability_start_time: Option<String>,
    pub time_shift_buffer_depth: Option<f64>, pub minimum_update_period: Option<f64>,
    pub suggested_presentation_delay: Option<f64>, pub publish_time: Option<String>,
    pub base_urls: Vec<String>, pub utc_timing: Vec<UtcTiming>, pub periods: Vec<MpdPeriod>,
}
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MpdType { Static, Dynamic }
#[derive(Debug, Clone)]
pub struct UtcTiming { pub scheme_id_uri: String, pub value: String }
#[derive(Debug, Clone)]
pub struct MpdPeriod {
    pub id: Option<String>, pub start: Option<f64>, pub duration: Option<f64>,
    pub base_urls: Vec<String>, pub adaptation_sets: Vec<MpdAdaptationSet>,
    pub xlink_href: Option<String>,
}
#[derive(Debug, Clone)]
pub struct MpdAdaptationSet {
    pub id: Option<String>, pub content_type: Option<String>,
    pub mime_type: Option<String>, pub codecs: Option<String>,
    pub lang: Option<String>, pub segment_alignment: bool,
    pub subsegment_alignment: bool, pub bitstream_switching: bool,
    pub roles: Vec<MpdDescriptor>, pub accessibility: Vec<MpdDescriptor>,
    pub labels: Vec<String>, pub content_protection: Vec<MpdContentProtection>,
    pub supplemental_properties: Vec<MpdDescriptor>,
    pub essential_properties: Vec<MpdDescriptor>,
    pub segment_template: Option<MpdSegmentTemplate>,
    pub segment_base: Option<MpdSegmentBase>, pub segment_list: Option<MpdSegmentList>,
    pub base_urls: Vec<String>, pub representations: Vec<MpdRepresentation>,
}
#[derive(Debug, Clone)]
pub struct MpdRepresentation {
    pub id: Option<String>, pub bandwidth: u64,
    pub width: Option<u32>, pub height: Option<u32>,
    pub codecs: Option<String>, pub mime_type: Option<String>,
    pub frame_rate: Option<String>, pub sar: Option<String>,
    pub audio_sampling_rate: Option<u32>,
    pub segment_template: Option<MpdSegmentTemplate>,
    pub segment_base: Option<MpdSegmentBase>, pub segment_list: Option<MpdSegmentList>,
    pub base_urls: Vec<String>, pub content_protection: Vec<MpdContentProtection>,
    pub absolute_index: usize,
}
impl MpdRepresentation {
    pub fn bitrate_kbps(&self) -> f64 { self.bandwidth as f64 / 1000.0 }
}
#[derive(Debug, Clone)]
pub struct MpdSegmentTemplate {
    pub initialization: Option<String>, pub media: Option<String>,
    pub start_number: usize, pub timescale: f64, pub duration: Option<f64>,
    pub presentation_time_offset: Option<f64>, pub timeline: Vec<MpdTimelineEntry>,
}
#[derive(Debug, Clone)]
pub struct MpdSegmentBase {
    pub index_range: Option<String>, pub initialization_range: Option<String>,
    pub timescale: f64, pub presentation_time_offset: Option<f64>,
}
#[derive(Debug, Clone)]
pub struct MpdSegmentList {
    pub initialization: Option<String>, pub timescale: f64,
    pub duration: Option<f64>, pub start_number: usize,
    pub segment_urls: Vec<MpdSegmentUrl>,
}
#[derive(Debug, Clone)]
pub struct MpdSegmentUrl { pub media: String, pub media_range: Option<String> }
#[derive(Debug, Clone)]
pub struct MpdTimelineEntry { pub t: Option<u64>, pub d: u64, pub r: i64 }
#[derive(Debug, Clone)]
pub struct MpdDescriptor { pub scheme_id_uri: String, pub value: Option<String> }
#[derive(Debug, Clone)]
pub struct MpdContentProtection {
    pub scheme_id_uri: String, pub value: Option<String>,
    pub default_kid: Option<String>, pub cenc_pssh: Option<String>,
}
#[derive(Clone, Debug, PartialEq)]
pub struct AudioTrackInfo {
    pub adaptation_set_idx: usize, pub lang: Option<String>,
    pub label: Option<String>, pub codecs: Option<String>, pub roles: Vec<String>,
}

/// Full MPD parser — supports multiple Periods, AdaptationSets, all segment
/// addressing schemes, live attributes, etc.
pub fn parse_mpd_full(text: &str) -> Mpd {
    let mpd_type = if mpd_attr(text, "MPD", "type").as_deref() == Some("dynamic") { MpdType::Dynamic } else { MpdType::Static };
    let duration = mpd_attr(text, "MPD", "mediaPresentationDuration").map(|s| parse_iso8601_duration(&s)).unwrap_or(0.0);
    let min_buf = mpd_attr(text, "MPD", "minBufferTime").map(|s| parse_iso8601_duration(&s)).unwrap_or(1.5);
    Mpd {
        mpd_type, media_presentation_duration: duration, min_buffer_time: min_buf,
        availability_start_time: mpd_attr(text, "MPD", "availabilityStartTime"),
        time_shift_buffer_depth: mpd_attr(text, "MPD", "timeShiftBufferDepth").map(|s| parse_iso8601_duration(&s)),
        minimum_update_period: mpd_attr(text, "MPD", "minimumUpdatePeriod").map(|s| parse_iso8601_duration(&s)),
        suggested_presentation_delay: mpd_attr(text, "MPD", "suggestedPresentationDelay").map(|s| parse_iso8601_duration(&s)),
        publish_time: mpd_attr(text, "MPD", "publishTime"),
        base_urls: mpd_base_urls(text), utc_timing: mpd_utc(text),
        periods: mpd_periods(text),
    }
}

fn mpd_attr(xml: &str, tag: &str, attr: &str) -> Option<String> {
    let ts = xml.find(&format!("<{tag}"))?;
    let te = xml[ts..].find('>')?;
    let c = &xml[ts..ts + te]; let s = format!("{attr}=\"");
    let p = c.find(&s)?; let r = &c[p + s.len()..]; Some(r[..r.find('"')?].to_string())
}
fn mpd_tag_attr(tag: &str, attr: &str) -> Option<String> {
    let s = format!("{attr}=\""); let p = tag.find(&s)?;
    let r = &tag[p + s.len()..]; Some(r[..r.find('"')?].to_string())
}
fn mpd_base_urls(xml: &str) -> Vec<String> {
    let mut v = Vec::new(); let mut s = 0;
    while let Some(i) = xml[s..].find("<BaseURL>") {
        let a = s + i + 9;
        if let Some(e) = xml[a..].find("</BaseURL>") { v.push(xml[a..a + e].trim().into()); s = a + e; } else { break; }
    }
    v
}
fn mpd_utc(xml: &str) -> Vec<UtcTiming> {
    let mut v = Vec::new(); let mut s = 0;
    while let Some(i) = xml[s..].find("<UTCTiming") {
        let a = s + i;
        if let Some(e) = xml[a..].find("/>").or_else(|| xml[a..].find('>')) {
            let t = &xml[a..a + e + 2];
            v.push(UtcTiming { scheme_id_uri: mpd_tag_attr(t, "schemeIdUri").unwrap_or_default(), value: mpd_tag_attr(t, "value").unwrap_or_default() });
            s = a + e + 2;
        } else { break; }
    }
    v
}
fn mpd_close(xml: &str, tag: &str) -> Option<usize> { xml.find(&format!("</{tag}>")).map(|p| p + tag.len() + 3) }

fn mpd_periods(xml: &str) -> Vec<MpdPeriod> {
    let mut v = Vec::new(); let mut s = 0;
    while let Some(i) = xml[s..].find("<Period") {
        let a = s + i;
        let e = mpd_close(&xml[a..], "Period").map(|x| a + x).unwrap_or(xml.len());
        let px = &xml[a..e];
        v.push(MpdPeriod {
            id: mpd_tag_attr(px, "id"), start: mpd_tag_attr(px, "start").map(|v| parse_iso8601_duration(&v)),
            duration: mpd_tag_attr(px, "duration").map(|v| parse_iso8601_duration(&v)),
            base_urls: mpd_base_urls(px), adaptation_sets: mpd_asets(px),
            xlink_href: mpd_tag_attr(px, "xlink:href"),
        });
        s = e;
    }
    v
}

fn mpd_asets(px: &str) -> Vec<MpdAdaptationSet> {
    let mut v = Vec::new(); let mut s = 0;
    while let Some(i) = px[s..].find("<AdaptationSet") {
        let a = s + i;
        let e = mpd_close(&px[a..], "AdaptationSet").map(|x| a + x).unwrap_or(px.len());
        let ax = &px[a..e];
        let ct = mpd_tag_attr(ax, "contentType");
        let mt = mpd_tag_attr(ax, "mimeType");
        let codecs = mpd_tag_attr(ax, "codecs");
        let st = mpd_seg_template(ax); let sb = mpd_seg_base(ax); let sl = mpd_seg_list(ax);
        let mut reps = mpd_reps(ax);
        reps.sort_by_key(|r| r.bandwidth);
        for (i, r) in reps.iter_mut().enumerate() {
            r.absolute_index = i;
            if r.codecs.is_none() { r.codecs = codecs.clone(); }
            if r.mime_type.is_none() { r.mime_type = mt.clone(); }
            if r.segment_template.is_none() { r.segment_template = st.clone(); }
            if r.segment_base.is_none() { r.segment_base = sb.clone(); }
            if r.segment_list.is_none() { r.segment_list = sl.clone(); }
        }
        let ect = ct.clone().or_else(|| mt.as_deref().map(|m| {
            if m.starts_with("video") { "video".into() }
            else if m.starts_with("audio") { "audio".into() }
            else if m.starts_with("text") { "text".into() }
            else { m.to_string() }
        }));
        v.push(MpdAdaptationSet {
            id: mpd_tag_attr(ax, "id"), content_type: ect, mime_type: mt, codecs,
            lang: mpd_tag_attr(ax, "lang"),
            segment_alignment: mpd_tag_attr(ax, "segmentAlignment").as_deref() == Some("true"),
            subsegment_alignment: mpd_tag_attr(ax, "subsegmentAlignment").as_deref() == Some("true"),
            bitstream_switching: mpd_tag_attr(ax, "bitstreamSwitching").as_deref() == Some("true"),
            roles: mpd_descriptors(ax, "Role"), accessibility: mpd_descriptors(ax, "Accessibility"),
            labels: mpd_labels(ax), content_protection: mpd_cp(ax),
            supplemental_properties: mpd_descriptors(ax, "SupplementalProperty"),
            essential_properties: mpd_descriptors(ax, "EssentialProperty"),
            segment_template: st, segment_base: sb, segment_list: sl,
            base_urls: mpd_base_urls(ax), representations: reps,
        });
        s = e;
    }
    v
}

fn mpd_reps(ax: &str) -> Vec<MpdRepresentation> {
    let mut v = Vec::new(); let mut s = 0;
    while let Some(i) = ax[s..].find("<Representation") {
        let a = s + i;
        let e = mpd_close(&ax[a..], "Representation")
            .map(|x| a + x).unwrap_or_else(|| ax[a..].find("/>").map(|x| a + x + 2).unwrap_or(ax.len()));
        let rx = &ax[a..e];
        v.push(MpdRepresentation {
            id: mpd_tag_attr(rx, "id"),
            bandwidth: mpd_tag_attr(rx, "bandwidth").and_then(|s| s.parse().ok()).unwrap_or(0),
            width: mpd_tag_attr(rx, "width").and_then(|s| s.parse().ok()),
            height: mpd_tag_attr(rx, "height").and_then(|s| s.parse().ok()),
            codecs: mpd_tag_attr(rx, "codecs"), mime_type: mpd_tag_attr(rx, "mimeType"),
            frame_rate: mpd_tag_attr(rx, "frameRate"), sar: mpd_tag_attr(rx, "sar"),
            audio_sampling_rate: mpd_tag_attr(rx, "audioSamplingRate").and_then(|s| s.parse().ok()),
            segment_template: mpd_seg_template(rx), segment_base: mpd_seg_base(rx),
            segment_list: mpd_seg_list(rx), base_urls: mpd_base_urls(rx),
            content_protection: mpd_cp(rx), absolute_index: 0,
        });
        s = e;
    }
    v
}

fn mpd_seg_template(xml: &str) -> Option<MpdSegmentTemplate> {
    let i = xml.find("<SegmentTemplate")?;
    let ce = xml[i..].find('>')?;
    let e = mpd_close(&xml[i..], "SegmentTemplate").map(|x| i + x).unwrap_or(i + ce + 1);
    let inner = &xml[i..e];
    Some(MpdSegmentTemplate {
        initialization: mpd_tag_attr(inner, "initialization"), media: mpd_tag_attr(inner, "media"),
        start_number: mpd_tag_attr(inner, "startNumber").and_then(|s| s.parse().ok()).unwrap_or(0),
        timescale: mpd_tag_attr(inner, "timescale").and_then(|s| s.parse().ok()).unwrap_or(1000.0),
        duration: mpd_tag_attr(inner, "duration").and_then(|s| s.parse().ok()),
        presentation_time_offset: mpd_tag_attr(inner, "presentationTimeOffset").and_then(|s| s.parse().ok()),
        timeline: mpd_timeline(inner),
    })
}
fn mpd_timeline(xml: &str) -> Vec<MpdTimelineEntry> {
    let mut v = Vec::new(); let mut s = 0;
    while let Some(i) = xml[s..].find("<S ") {
        let a = s + i;
        if let Some(e) = xml[a..].find("/>") {
            let t = &xml[a..a + e + 2];
            v.push(MpdTimelineEntry {
                t: mpd_tag_attr(t, "t").and_then(|s| s.parse().ok()),
                d: mpd_tag_attr(t, "d").and_then(|s| s.parse().ok()).unwrap_or(0),
                r: mpd_tag_attr(t, "r").and_then(|s| s.parse().ok()).unwrap_or(0),
            });
            s = a + e + 2;
        } else { break; }
    }
    v
}
fn mpd_seg_base(xml: &str) -> Option<MpdSegmentBase> {
    let i = xml.find("<SegmentBase")?;
    let e = xml[i..].find("/>").or_else(|| xml[i..].find('>'))?;
    let t = &xml[i..i + e + 2];
    Some(MpdSegmentBase {
        index_range: mpd_tag_attr(t, "indexRange"),
        initialization_range: mpd_tag_attr(t, "Initialization")
            .or_else(|| xml[i..].find("<Initialization").and_then(|x| mpd_tag_attr(&xml[i + x..], "range"))),
        timescale: mpd_tag_attr(t, "timescale").and_then(|s| s.parse().ok()).unwrap_or(1000.0),
        presentation_time_offset: mpd_tag_attr(t, "presentationTimeOffset").and_then(|s| s.parse().ok()),
    })
}
fn mpd_seg_list(xml: &str) -> Option<MpdSegmentList> {
    let i = xml.find("<SegmentList")?;
    let e = mpd_close(&xml[i..], "SegmentList").map(|x| i + x).unwrap_or(xml.len());
    let inner = &xml[i..e];
    let init = inner.find("<Initialization").and_then(|x| mpd_tag_attr(&inner[x..], "sourceURL"));
    let mut urls = Vec::new(); let mut s2 = 0;
    while let Some(su) = inner[s2..].find("<SegmentURL") {
        let a2 = s2 + su;
        if let Some(e2) = inner[a2..].find("/>") {
            let t = &inner[a2..a2 + e2 + 2];
            urls.push(MpdSegmentUrl { media: mpd_tag_attr(t, "media").unwrap_or_default(), media_range: mpd_tag_attr(t, "mediaRange") });
            s2 = a2 + e2 + 2;
        } else { break; }
    }
    Some(MpdSegmentList {
        initialization: init, timescale: mpd_tag_attr(inner, "timescale").and_then(|s| s.parse().ok()).unwrap_or(1000.0),
        duration: mpd_tag_attr(inner, "duration").and_then(|s| s.parse().ok()),
        start_number: mpd_tag_attr(inner, "startNumber").and_then(|s| s.parse().ok()).unwrap_or(0),
        segment_urls: urls,
    })
}
fn mpd_descriptors(xml: &str, tag: &str) -> Vec<MpdDescriptor> {
    let mut v = Vec::new(); let st = format!("<{tag}"); let mut s = 0;
    while let Some(i) = xml[s..].find(&st) {
        let a = s + i;
        if let Some(e) = xml[a..].find("/>").or_else(|| xml[a..].find('>')) {
            let t = &xml[a..a + e + 2];
            v.push(MpdDescriptor { scheme_id_uri: mpd_tag_attr(t, "schemeIdUri").unwrap_or_default(), value: mpd_tag_attr(t, "value") });
            s = a + e + 2;
        } else { break; }
    }
    v
}
fn mpd_labels(xml: &str) -> Vec<String> {
    let mut v = Vec::new(); let mut s = 0;
    while let Some(i) = xml[s..].find("<Label>") {
        let a = s + i + 7;
        if let Some(e) = xml[a..].find("</Label>") { v.push(xml[a..a + e].trim().into()); s = a + e; } else { break; }
    }
    v
}
fn mpd_cp(xml: &str) -> Vec<MpdContentProtection> {
    let mut v = Vec::new(); let mut s = 0;
    while let Some(i) = xml[s..].find("<ContentProtection") {
        let a = s + i;
        let e = mpd_close(&xml[a..], "ContentProtection").unwrap_or_else(|| xml[a..].find("/>").map(|x| x + 2).unwrap_or(xml.len() - a));
        let t = &xml[a..a + e];
        let pssh = t.find("<cenc:pssh>").and_then(|p| { let r = &t[p + 11..]; r.find("</cenc:pssh>").map(|e| r[..e].trim().into()) });
        v.push(MpdContentProtection {
            scheme_id_uri: mpd_tag_attr(t, "schemeIdUri").unwrap_or_default(), value: mpd_tag_attr(t, "value"),
            default_kid: mpd_tag_attr(t, "cenc:default_KID").or_else(|| mpd_tag_attr(t, "default_KID")), cenc_pssh: pssh,
        });
        s = a + e;
    }
    v
}

fn mpd_resolve_tmpl(tmpl: &str, num: Option<usize>, rep_id: Option<&str>, time: Option<u64>, bw: Option<u64>) -> String {
    let mut r = tmpl.to_string();
    if let Some(n) = num {
        if r.contains("$Number%") {
            if let Some(s) = r.find("$Number%") {
                if let Some(e) = r[s + 8..].find('$') {
                    let fmt = &r[s + 8..s + 8 + e];
                    let pw: usize = fmt.trim_end_matches('d').parse().unwrap_or(1);
                    let f = format!("{:0>pw$}", n, pw = pw);
                    r = r.replace(&format!("$Number%{fmt}$"), &f);
                }
            }
        }
        r = r.replace("$Number$", &n.to_string());
    }
    if let Some(id) = rep_id { r = r.replace("$RepresentationID$", id); }
    if let Some(t) = time { r = r.replace("$Time$", &t.to_string()); }
    if let Some(b) = bw { r = r.replace("$Bandwidth$", &b.to_string()); }
    r
}
fn mpd_resolve_base(base: &str, rel: &str) -> String {
    if rel.starts_with("http://") || rel.starts_with("https://") || rel.starts_with("//") { return rel.into(); }
    if base.is_empty() { return rel.into(); }
    if let Some(ls) = base.rfind('/') { format!("{}/{rel}", &base[..ls]) } else { rel.into() }
}

/// Build segment list from parsed MPD for backward-compatible integration.
fn mpd_build_segments(mpd: &Mpd, pi: usize, ai: usize, ri: usize, base: &str) -> (String, f64, Vec<SegmentInfo>) {
    let p = &mpd.periods[pi]; let a = &p.adaptation_sets[ai]; let rep = &a.representations[ri];
    let b = if !rep.base_urls.is_empty() { rep.base_urls[0].clone() }
        else if !a.base_urls.is_empty() { a.base_urls[0].clone() }
        else if !p.base_urls.is_empty() { p.base_urls[0].clone() }
        else if !mpd.base_urls.is_empty() { mpd.base_urls[0].clone() }
        else if let Some(sl) = base.rfind('/') { base[..sl + 1].into() }
        else { String::new() };
    if let Some(t) = rep.segment_template.as_ref().or(a.segment_template.as_ref()) {
        return mpd_build_tmpl(mpd, t, rep, &b);
    }
    if let Some(l) = rep.segment_list.as_ref().or(a.segment_list.as_ref()) {
        return mpd_build_list(l, &b);
    }
    if let Some(sb) = rep.segment_base.as_ref().or(a.segment_base.as_ref()) {
        return mpd_build_base(sb, rep, &b);
    }
    (String::new(), mpd.media_presentation_duration, Vec::new())
}

fn mpd_build_tmpl(mpd: &Mpd, t: &MpdSegmentTemplate, rep: &MpdRepresentation, base: &str) -> (String, f64, Vec<SegmentInfo>) {
    let rid = rep.id.as_deref().unwrap_or("");
    let init = t.initialization.as_ref().map(|i| mpd_resolve_base(base, &mpd_resolve_tmpl(i, None, Some(rid), None, Some(rep.bandwidth)))).unwrap_or_default();
    let media = t.media.as_deref().unwrap_or("");
    let mut segs = Vec::new();
    if !t.timeline.is_empty() {
        let mut num = t.start_number; let mut ct: u64 = 0;
        for entry in &t.timeline {
            if let Some(tv) = entry.t { ct = tv; }
            let rc = if entry.r >= 0 { entry.r as usize } else { 0 };
            for _ in 0..=rc {
                let url = mpd_resolve_tmpl(media, Some(num), Some(rid), Some(ct), Some(rep.bandwidth));
                segs.push(SegmentInfo { url: mpd_resolve_base(base, &url), duration: entry.d as f64 / t.timescale });
                ct += entry.d; num += 1;
            }
        }
    } else if let Some(dur) = t.duration {
        let sd = dur / t.timescale;
        let n = if sd > 0.0 { (mpd.media_presentation_duration / sd).ceil() as usize } else { 0 };
        for i in 0..n {
            let url = mpd_resolve_tmpl(media, Some(t.start_number + i), Some(rid), None, Some(rep.bandwidth));
            segs.push(SegmentInfo { url: mpd_resolve_base(base, &url), duration: sd });
        }
    }
    (init, mpd.media_presentation_duration, segs)
}
fn mpd_build_list(l: &MpdSegmentList, base: &str) -> (String, f64, Vec<SegmentInfo>) {
    let init = l.initialization.as_ref().map(|u| mpd_resolve_base(base, u)).unwrap_or_default();
    let sd = l.duration.map(|d| d / l.timescale).unwrap_or_else(|| { log::warn!("SegmentList missing duration, using default"); SEGMENT_DURATION_F });
    let segs: Vec<_> = l.segment_urls.iter().map(|su| SegmentInfo { url: mpd_resolve_base(base, &su.media), duration: sd }).collect();
    (init, sd * segs.len() as f64, segs)
}
fn mpd_build_base(_sb: &MpdSegmentBase, rep: &MpdRepresentation, base: &str) -> (String, f64, Vec<SegmentInfo>) {
    let url = if !rep.base_urls.is_empty() { mpd_resolve_base(base, &rep.base_urls[0]) } else { base.into() };
    (url.clone(), 0.0, vec![SegmentInfo { url, duration: 0.0 }])
}

// ── §5 ABR Controller — AbrController.js ─────────────────────────────────────

#[derive(Debug, Clone, Copy, PartialEq)]
pub enum SwitchPriority { Weak = 0, Default = 1, Strong = 2 }

#[derive(Debug, Clone)]
pub struct SwitchRequest {
    pub representation_index: Option<usize>, pub priority: SwitchPriority,
    pub reason: String, pub rule: String,
}
impl SwitchRequest {
    fn no_change(rule: &str) -> Self { Self { representation_index: None, priority: SwitchPriority::Default, reason: "no change".into(), rule: rule.into() } }
    fn with_index(idx: usize, p: SwitchPriority, reason: String, rule: &str) -> Self { Self { representation_index: Some(idx), priority: p, reason, rule: rule.into() } }
}

#[derive(Debug, Clone)]
struct BolaState { state: BolaPhase, utilities: Vec<f64>, vp: f64, gp: f64, placeholder_buffer: f64, last_quality: usize }
#[derive(Debug, Clone, Copy, PartialEq)]
enum BolaPhase { OneBitrate, Startup, Steady }

pub struct AbrController {
    pub auto_switch_bitrate: bool,
    pub use_bola: HashMap<MediaType, bool>,
    bola_states: HashMap<MediaType, Option<BolaState>>,
    manual_quality: HashMap<MediaType, Option<usize>>,
}
impl AbrController {
    pub fn new() -> Self {
        let mut ub = HashMap::new(); ub.insert(MediaType::Video, false); ub.insert(MediaType::Audio, false);
        Self { auto_switch_bitrate: true, use_bola: ub, bola_states: HashMap::new(), manual_quality: HashMap::new() }
    }
    pub fn set_manual_quality(&mut self, mt: MediaType, idx: Option<usize>) { self.manual_quality.insert(mt, idx); }
    pub fn get_optimal_rep(reps: &[MpdRepresentation], kbps: f64) -> usize {
        let mut best = 0;
        for (i, r) in reps.iter().enumerate() { if r.bitrate_kbps() <= kbps { best = i; } }
        best
    }
    pub fn get_quality(&mut self, mt: MediaType, reps: &[MpdRepresentation], tc: &ThroughputController, metrics: &DashMetrics, buf: f64, is_dyn: bool) -> usize {
        if !self.auto_switch_bitrate { if let Some(Some(m)) = self.manual_quality.get(&mt) { return (*m).min(reps.len().saturating_sub(1)); } }
        if reps.len() <= 1 { return 0; }
        let mut reqs = vec![
            Self::throughput_rule(mt, reps, tc, metrics, is_dyn),
            Self::insufficient_buffer_rule(mt, reps, tc, buf),
            Self::switch_history_rule(reps, metrics),
            Self::dropped_frames_rule(reps, metrics),
        ];
        if *self.use_bola.get(&mt).unwrap_or(&false) { reqs.push(self.bola_rule(mt, reps, tc, buf)); }
        Self::arbitrate(&reqs, reps)
    }
    pub fn should_abandon(_mt: MediaType, reps: &[MpdRepresentation], cur: usize, loaded: usize, total: usize, elapsed_ms: f64, seg_dur: f64, buf: f64) -> Option<SwitchRequest> {
        if reps.len() <= 1 || cur == 0 || buf >= STABLE_BUFFER_TIME_S || elapsed_ms < 500.0 || loaded < 1000 || loaded >= total { return None; }
        let tp = (loaded as f64 * 8.0) / elapsed_ms;
        let est = (total as f64 * 8.0) / tp / 1000.0;
        if est < seg_dur * ABANDON_DURATION_MULTIPLIER { return None; }
        let opt = Self::get_optimal_rep(reps, tp);
        if opt >= cur { return None; }
        Some(SwitchRequest::with_index(opt, SwitchPriority::Strong, format!("abandon: est {est:.1}s"), "AbandonRequestsRule"))
    }
    fn throughput_rule(mt: MediaType, reps: &[MpdRepresentation], tc: &ThroughputController, metrics: &DashMetrics, is_dyn: bool) -> SwitchRequest {
        if metrics.get_buffer_state(mt) != BufferState::Loaded && !is_dyn { return SwitchRequest::no_change("ThroughputRule"); }
        if tc.get_sample_count(mt) < ABR_MIN_SAMPLES { return SwitchRequest::no_change("ThroughputRule"); }
        let safe = tc.get_safe_average_throughput(mt);
        if safe.is_nan() || safe <= 0.0 { return SwitchRequest::no_change("ThroughputRule"); }
        SwitchRequest::with_index(Self::get_optimal_rep(reps, safe), SwitchPriority::Default, format!("tp {safe:.0}kbps"), "ThroughputRule")
    }
    fn bola_rule(&mut self, mt: MediaType, reps: &[MpdRepresentation], tc: &ThroughputController, buf: f64) -> SwitchRequest {
        let bola = self.bola_states.entry(mt).or_insert_with(|| Self::init_bola(reps));
        let b = match bola { Some(b) => b, None => return SwitchRequest::no_change("BolaRule") };
        match b.state {
            BolaPhase::OneBitrate => SwitchRequest::no_change("BolaRule"),
            BolaPhase::Startup => {
                let safe = tc.get_safe_average_throughput(mt);
                let idx = if safe.is_nan() || safe <= 0.0 { 0 } else { Self::get_optimal_rep(reps, safe) };
                b.last_quality = idx; if buf >= SEGMENT_DURATION_F { b.state = BolaPhase::Steady; }
                SwitchRequest::with_index(idx, SwitchPriority::Default, "BOLA startup".into(), "BolaRule")
            }
            BolaPhase::Steady => {
                b.placeholder_buffer *= BOLA_PLACEHOLDER_DECAY;
                let eb = buf + b.placeholder_buffer;
                let mut bi = 0; let mut bs = f64::NEG_INFINITY;
                for (i, rep) in reps.iter().enumerate() {
                    if i < b.utilities.len() {
                        let sc = (b.vp * (b.utilities[i] - 1.0 + b.gp) - eb) / rep.bandwidth as f64;
                        if sc > bs { bs = sc; bi = i; }
                    }
                }
                let safe = tc.get_safe_average_throughput(mt);
                if !safe.is_nan() && safe > 0.0 {
                    let ti = Self::get_optimal_rep(reps, safe);
                    if bi > ti && bi > b.last_quality { bi = b.last_quality.max(ti); }
                }
                b.last_quality = bi;
                SwitchRequest::with_index(bi, SwitchPriority::Default, format!("BOLA buf={eb:.1}s"), "BolaRule")
            }
        }
    }
    fn init_bola(reps: &[MpdRepresentation]) -> Option<BolaState> {
        if reps.len() <= 1 { return Some(BolaState { state: BolaPhase::OneBitrate, utilities: vec![1.0], vp: 0.0, gp: 0.0, placeholder_buffer: 0.0, last_quality: 0 }); }
        let brs: Vec<f64> = reps.iter().map(|r| r.bandwidth as f64).collect();
        let mut utils: Vec<f64> = brs.iter().map(|b| b.ln()).collect();
        let u0 = utils[0]; for u in utils.iter_mut() { *u -= u0 - 1.0; }
        let hi = utils.iter().enumerate().max_by(|a, b| a.1.partial_cmp(b.1).unwrap_or(std::cmp::Ordering::Equal)).map(|(i, _)| i).unwrap_or(0);
        if hi == 0 { return None; }
        let bt = BUFFER_TIME_DEFAULT_S.max(BOLA_MINIMUM_BUFFER_S + BOLA_MINIMUM_BUFFER_PER_LEVEL_S * reps.len() as f64);
        let gp = (utils[hi] - 1.0) / (bt / BOLA_MINIMUM_BUFFER_S - 1.0);
        let vp = BOLA_MINIMUM_BUFFER_S / gp;
        Some(BolaState { state: BolaPhase::Startup, utilities: utils, vp, gp, placeholder_buffer: 0.0, last_quality: 0 })
    }
    fn insufficient_buffer_rule(mt: MediaType, reps: &[MpdRepresentation], tc: &ThroughputController, buf: f64) -> SwitchRequest {
        if buf >= STABLE_BUFFER_TIME_S { return SwitchRequest::no_change("InsufficientBufferRule"); }
        let tp = tc.get_average_throughput(mt);
        if tp.is_nan() || tp <= 0.0 { return SwitchRequest::no_change("InsufficientBufferRule"); }
        let avail = tp * ABR_BANDWIDTH_SAFETY_FACTOR * buf / SEGMENT_DURATION_F;
        SwitchRequest::with_index(Self::get_optimal_rep(reps, avail), SwitchPriority::Default, format!("low buf {buf:.1}s"), "InsufficientBufferRule")
    }
    fn switch_history_rule(reps: &[MpdRepresentation], metrics: &DashMetrics) -> SwitchRequest {
        for (i, _) in reps.iter().enumerate().rev() {
            if let Some(e) = metrics.switch_history.get(&i) {
                let total = e.drops + e.no_drops;
                if total >= SWITCH_HISTORY_SAMPLE_SIZE && e.no_drops > 0 && (e.drops as f64 / e.no_drops as f64) > SWITCH_HISTORY_THRESHOLD {
                    return SwitchRequest::with_index(i.saturating_sub(1), SwitchPriority::Default, "history".into(), "SwitchHistoryRule");
                }
            }
        }
        SwitchRequest::no_change("SwitchHistoryRule")
    }
    fn dropped_frames_rule(reps: &[MpdRepresentation], metrics: &DashMetrics) -> SwitchRequest {
        let df = &metrics.dropped_frames;
        if df.total_frames < DROPPED_FRAMES_MIN_SAMPLE { return SwitchRequest::no_change("DroppedFramesRule"); }
        if df.dropped_frames as f64 / df.total_frames as f64 > DROPPED_FRAMES_THRESHOLD && reps.len() > 1 {
            return SwitchRequest::with_index(reps.len().saturating_sub(2), SwitchPriority::Default, "drops".into(), "DroppedFramesRule");
        }
        SwitchRequest::no_change("DroppedFramesRule")
    }
    fn arbitrate(reqs: &[SwitchRequest], reps: &[MpdRepresentation]) -> usize {
        let (mut strong, mut default, mut weak): (Option<usize>, Option<usize>, Option<usize>) = (None, None, None);
        for req in reqs {
            if let Some(idx) = req.representation_index {
                let slot = match req.priority { SwitchPriority::Strong => &mut strong, SwitchPriority::Default => &mut default, SwitchPriority::Weak => &mut weak };
                let cur_bw = slot.and_then(|i| reps.get(i).map(|r| r.bandwidth));
                let new_bw = reps.get(idx).map(|r| r.bandwidth).unwrap_or(u64::MAX);
                if cur_bw.is_none() || new_bw < cur_bw.unwrap() { *slot = Some(idx); }
            }
        }
        strong.or(default).or(weak).unwrap_or(0)
    }
}

// ── §6 Schedule Controller ───────────────────────────────────────────────────

pub struct ScheduleController;
impl ScheduleController {
    pub fn should_schedule(buf: f64, seg_dur: f64, is_startup: bool) -> bool {
        let target = if is_startup { BUFFER_TIME_DEFAULT_S } else { STABLE_BUFFER_TIME_S };
        buf + seg_dur < target.max(MSE_TARGET_BUFFER_S)
    }
}

// ── §7 Live Stream Support ───────────────────────────────────────────────────

#[derive(Debug, Clone)]
pub struct LiveStreamController {
    pub clock_offset_s: f64, pub target_delay_s: f64,
    pub availability_start_time_epoch: Option<f64>,
    pub time_shift_buffer_depth: Option<f64>, pub minimum_update_period: Option<f64>,
    pub catchup_active: bool,
}
impl LiveStreamController {
    pub fn new() -> Self {
        Self { clock_offset_s: 0.0, target_delay_s: LIVE_DEFAULT_PRESENTATION_DELAY_S,
            availability_start_time_epoch: None, time_shift_buffer_depth: None,
            minimum_update_period: None, catchup_active: false }
    }
    pub fn configure_from_mpd(&mut self, mpd: &Mpd) {
        self.target_delay_s = mpd.suggested_presentation_delay.unwrap_or(LIVE_DEFAULT_PRESENTATION_DELAY_S);
        self.time_shift_buffer_depth = mpd.time_shift_buffer_depth;
        self.minimum_update_period = mpd.minimum_update_period;
        if let Some(ast) = &mpd.availability_start_time {
            let d = js_sys::Date::new(&JsValue::from_str(ast));
            let ms = d.get_time();
            if !ms.is_nan() { self.availability_start_time_epoch = Some(ms / 1000.0); }
        }
    }
    pub fn get_live_edge_time(&self) -> f64 {
        let now = js_sys::Date::now() / 1000.0;
        (now + self.clock_offset_s) - self.availability_start_time_epoch.unwrap_or(now)
    }
    pub fn get_live_start_position(&self) -> f64 { (self.get_live_edge_time() - self.target_delay_s).max(0.0) }
    pub fn is_segment_available(&self, time: f64) -> bool {
        let le = self.get_live_edge_time();
        let ws = self.time_shift_buffer_depth.map(|d| (le - d).max(0.0)).unwrap_or(0.0);
        time >= ws && time <= le
    }
    pub fn dvr_window_start(&self) -> f64 {
        self.time_shift_buffer_depth.map(|d| (self.get_live_edge_time() - d).max(0.0)).unwrap_or(0.0)
    }
    pub fn dvr_window_end(&self) -> f64 { self.get_live_edge_time() }
    pub fn get_refresh_interval_ms(&self) -> u32 {
        self.minimum_update_period.map(|p| (p * 1000.0) as u32).unwrap_or(LIVE_MPD_REFRESH_INTERVAL_MS)
    }
    pub fn calculate_catchup_rate(&self, latency: f64, buf: f64) -> f64 {
        let delta = latency - self.target_delay_s;
        let cpr = if delta < 0.0 { LIVE_CATCHUP_RATE_MIN.abs() } else { LIVE_CATCHUP_RATE_MAX };
        let sigmoid = (cpr * 2.0) / (1.0 + (-delta * 5.0).exp());
        let mut rate = (1.0 - cpr) + sigmoid;
        if buf <= self.target_delay_s / 2.0 && delta > 0.0 { rate = 1.0; }
        rate.clamp(1.0 + LIVE_CATCHUP_RATE_MIN, 1.0 + LIVE_CATCHUP_RATE_MAX)
    }
    pub fn sync_clock_from_response(&mut self, server_time_ms: f64) {
        self.clock_offset_s = (server_time_ms - js_sys::Date::now()) / 1000.0;
    }
}

// ── §8 Error Recovery ────────────────────────────────────────────────────────

#[derive(Debug, Clone)]
pub struct ErrorRecovery {
    pub consecutive_errors: u32, pub max_retries: u32, pub fatal_error: bool,
}
impl ErrorRecovery {
    pub fn new() -> Self { Self { consecutive_errors: 0, max_retries: 3, fatal_error: false } }
    pub fn on_success(&mut self) { self.consecutive_errors = 0; }
    pub fn on_error(&mut self) -> bool { self.consecutive_errors += 1; self.consecutive_errors < self.max_retries }
    pub fn is_quota_exceeded(err: &JsValue) -> bool {
        if let Some(de) = err.dyn_ref::<web_sys::DomException>() { return de.code() == QUOTA_EXCEEDED_ERR_CODE; }
        err.as_string().is_some_and(|s| s.contains("QuotaExceeded"))
    }
    pub fn signal_eos_error(ms: &web_sys::MediaSource, err_type: &str) {
        match err_type {
            "decode" | "network" => {
                let v: &JsValue = ms.as_ref();
                if let Ok(f) = js_sys::Reflect::get(v, &JsValue::from_str("endOfStream")).and_then(|m| m.dyn_into::<js_sys::Function>()) {
                    let _ = f.call1(v, &JsValue::from_str(err_type));
                }
            }
            _ => { let _ = ms.end_of_stream(); }
        }
    }
    pub fn reset(&mut self) { self.consecutive_errors = 0; self.fatal_error = false; }
}

/// Extended DASH state attached alongside MseState.
struct DashExtState {
    mpd: Mpd,
    abr: AbrController,
    throughput: ThroughputController,
    metrics: DashMetrics,
    event_bus: EventBus,
    error_recovery: ErrorRecovery,
    live: Option<LiveStreamController>,
}

// ── End of new DASH infrastructure ───────────────────────────────────────────

// ── Helpers ──────────────────────────────────────────────────────────────────

fn format_time(seconds: f64) -> String {
    if !seconds.is_finite() || seconds < 0.0 {
        return "0:00".to_string();
    }
    let total_secs = seconds.round() as u64;
    let hours = total_secs / 3600;
    let mins = (total_secs % 3600) / 60;
    let secs = total_secs % 60;
    if hours > 0 {
        format!("{hours}:{mins:02}:{secs:02}")
    } else {
        format!("{mins}:{secs:02}")
    }
}

/// Return the end of the buffered range that contains `time`.
/// If `time` is not inside any buffered range, returns 0.0.
fn buffered_end_at(video: &HtmlVideoElement, time: f64) -> f64 {
    let buffered = video.buffered();
    for i in 0..buffered.length() {
        if let (Ok(start), Ok(end)) = (buffered.start(i), buffered.end(i)) {
            if time >= start && time <= end {
                return end;
            }
        }
    }
    0.0
}

/// Check whether `time` falls inside any buffered range of the video element.
fn is_time_buffered(video: &HtmlVideoElement, time: f64) -> bool {
    let buffered = video.buffered();
    for i in 0..buffered.length() {
        if let (Ok(s), Ok(e)) = (buffered.start(i), buffered.end(i)) {
            if time >= s && time <= e {
                return true;
            }
        }
    }
    false
}

/// Gap-jumping helper modelled after dash.js `GapController._jumpGap()`.
///
/// When the playhead is stalled just before a small gap between buffered
/// ranges, this function nudges `currentTime` past the gap so playback
/// resumes without a visible stutter.
///
/// dash.js `GapController._jumpGap()`:
///   1. Finds the first buffered range whose start is ahead of `currentTime`.
///   2. If the gap (range.start − currentTime) is ≤ `smallGapLimit`, seeks
///      past it.
///   3. If `jumpLargeGaps` is enabled and no small gap was found, jump to
///      the start of the next buffered range regardless of gap size.
///
/// We enable the large-gap behaviour unconditionally because remuxed fMP4
/// segments can have gaps larger than 0.8 s when keyframes don't align with
/// segment boundaries (e.g. keyframes every 8 s but segments every 6 s).
/// Ref: dash.js `settings.streaming.gaps.jumpLargeGaps`
///
/// Returns `true` if a gap was jumped, `false` otherwise.
fn try_jump_gap(video: &HtmlVideoElement) -> bool {
    let current = video.current_time();
    let buffered = video.buffered();
    let len = buffered.length();
    let mut nearest_ahead: Option<f64> = None;
    for i in 0..len {
        if let (Ok(start), Ok(_end)) = (buffered.start(i), buffered.end(i)) {
            // Ignore ranges that start before/at the current position and
            // gaps smaller than 1 ms (floating-point rounding noise).
            let gap = start - current;
            if gap > 0.001 {
                if gap <= SMALL_GAP_LIMIT_S {
                    // Small gap — jump immediately (dash.js default).
                    let target = start + 0.001;
                    log::info!(
                        "GapController: jumping {gap:.3}s gap at {current:.3}s → {target:.3}s"
                    );
                    video.set_current_time(target);
                    return true;
                }
                // Track the closest buffered-range start ahead of the
                // current position (minimum of all candidates) so the
                // large-gap path below can jump to it.
                if nearest_ahead.map_or(true, |n| start < n) {
                    nearest_ahead = Some(start);
                }
            }
        }
    }
    // Large-gap jump (dash.js `jumpLargeGaps`): when the playhead is stalled
    // between buffered ranges and no small gap was found, jump to the start
    // of the nearest buffered range ahead.
    if let Some(start) = nearest_ahead {
        let target = start + 0.001;
        log::info!(
            "GapController: large gap jump at {current:.3}s → {target:.3}s (gap {:.3}s)",
            start - current
        );
        video.set_current_time(target);
        return true;
    }
    false
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

struct MseState {
    media_source: web_sys::MediaSource,
    source_buffer: web_sys::SourceBuffer,
    /// Blob URL created for this MediaSource; revoked on cleanup.
    object_url: String,
    /// Parsed segment list from the DASH MPD manifest.
    segments: Vec<SegmentInfo>,
    /// Index of the next segment the pump should consider fetching.
    next_seg: usize,
    /// Generation counter — incremented on every seek or restart.
    /// Any running `pump_loop` whose id doesn't match exits immediately.
    pump_gen: u32,
    /// True while a `pump_loop` future is alive.
    pump_running: bool,
    /// The highest segment index that has been successfully appended to the
    /// SourceBuffer.  Used by the segment-index-based buffer gate to decide
    /// whether enough data has been buffered ahead of the playhead.
    /// Set to `None` until the first media segment is appended.
    last_appended_seg: Option<usize>,
    /// Extended DASH infrastructure (ABR, metrics, throughput, events, etc.)
    dash_ext: Option<DashExtState>,
}

/// Parse a DASH MPD manifest and return the list of segment URLs with durations.
///
/// This is a minimal parser that handles the specific MPD format our server
/// emits: a single Period with a single AdaptationSet containing a
/// SegmentTemplate with a SegmentTimeline.
///
/// Returns `(init_url, total_duration_secs, segments)`.
fn parse_mpd(text: &str) -> (String, f64, Vec<SegmentInfo>) {
    // Use the full MPD parser and extract backward-compatible output.
    let mpd = parse_mpd_full(text);
    if mpd.periods.is_empty() { return (String::new(), 0.0, Vec::new()); }
    let period = &mpd.periods[0];
    if period.adaptation_sets.is_empty() { return (String::new(), 0.0, Vec::new()); }
    // Find the video (or muxed) adaptation set.
    let aset_idx = period.adaptation_sets.iter().position(|a| {
        a.content_type.as_deref() == Some("video")
            || a.mime_type.as_deref().is_some_and(|m| m.starts_with("video"))
            || a.content_type.is_none()
    }).unwrap_or(0);
    let aset = &period.adaptation_sets[aset_idx];
    if aset.representations.is_empty() { return (String::new(), 0.0, Vec::new()); }
    // Use first representation (lowest bandwidth after sorting).
    mpd_build_segments(&mpd, 0, aset_idx, 0, "")
}

/// Parse an ISO 8601 duration like "PT1H23M45S" or "PT0H0M30S" into seconds.
fn parse_iso8601_duration(s: &str) -> f64 {
    let s = s.strip_prefix("PT").unwrap_or(s);
    let mut total = 0.0_f64;
    let mut num_buf = String::new();
    for ch in s.chars() {
        match ch {
            'H' | 'h' => {
                total += num_buf.parse::<f64>().unwrap_or(0.0) * 3600.0;
                num_buf.clear();
            }
            'M' | 'm' => {
                total += num_buf.parse::<f64>().unwrap_or(0.0) * 60.0;
                num_buf.clear();
            }
            'S' | 's' => {
                total += num_buf.parse::<f64>().unwrap_or(0.0);
                num_buf.clear();
            }
            _ => num_buf.push(ch),
        }
    }
    total
}

/// Strip ftyp and moov boxes from an fMP4 segment, keeping only moof+mdat.
///
/// Each fMP4 segment produced by `ffmpeg -movflags empty_moov+frag_keyframe+default_base_moof`
/// contains [ftyp][moov][moof][mdat].  The init segment (ftyp+moov) has already
/// been appended to the SourceBuffer, so media segments must only contain
/// moof+mdat to avoid confusing the browser's MSE implementation.
///
/// This is the same pattern used by all major DASH clients — dash.js, Shaka
/// Player, and hls.js all separate init segments from media segments before
/// appending to the SourceBuffer.  Per ISO BMFF (ISO 14496-12), the moov box
/// must only appear once in the SourceBuffer initialization.
fn strip_init_boxes(data: &[u8]) -> Vec<u8> {
    let mut result = Vec::new();
    let mut pos = 0usize;

    while pos + 8 <= data.len() {
        let size = u32::from_be_bytes([data[pos], data[pos + 1], data[pos + 2], data[pos + 3]]) as usize;
        if size < 8 || pos + size > data.len() {
            // Copy remaining bytes as-is (shouldn't happen with well-formed fMP4).
            result.extend_from_slice(&data[pos..]);
            break;
        }
        let box_type = &data[pos + 4..pos + 8];
        // Keep moof, mdat, and any other non-init boxes.
        // Skip ftyp and moov (already sent as init segment).
        if box_type != b"ftyp" && box_type != b"moov" {
            result.extend_from_slice(&data[pos..pos + size]);
        }
        pos += size;
    }

    if result.is_empty() && pos == 0 {
        // No valid box header was found at all (data too short or starts
        // with a malformed box) — return the raw bytes unchanged so the
        // caller can decide what to do.
        data.to_vec()
    } else {
        // `result` may be empty when every box was ftyp/moov (init-only
        // segment with no media data).  The caller's `media_bytes.is_empty()`
        // check will catch this and skip the segment correctly.
        result
    }
}

/// Check whether `pump_gen` inside `MseState` still matches the expected
/// generation.  Returns `false` (= caller should exit) when the state has
/// been dropped or a newer pump has been started (e.g. after a seek).
fn is_pump_current(state: &Rc<RefCell<Option<MseState>>>, pump_id: u32) -> bool {
    let borrow = state.borrow();
    matches!(borrow.as_ref(), Some(s) if s.pump_gen == pump_id)
}

/// Wait for the SourceBuffer to finish any in-progress operation.
/// Returns `false` if the pump generation changed during the wait.
///
/// Uses `TimeoutFuture::new(0)` (equivalent to `setTimeout(0)` in JS)
/// to yield to the browser's event loop between checks.  This matches
/// dash.js `SourceBufferSink._onUpdateEnd` → `setTimeout(executeNext, 0)`
/// pattern, ensuring the browser can process audio/video decode work
/// between SourceBuffer operations.
///
/// A 0 ms timeout doesn't actually wait — it just yields the current
/// microtask, letting the browser's event loop run one tick (process
/// pending updateend events, decode audio frames, render, etc.) before
/// we re-check `sb.updating()`.
async fn wait_for_sb(
    sb: &web_sys::SourceBuffer,
    state: &Rc<RefCell<Option<MseState>>>,
    pump_id: u32,
) -> bool {
    while sb.updating() {
        TimeoutFuture::new(0).await;
        if !is_pump_current(state, pump_id) {
            return false;
        }
    }
    true
}

/// Evict played data behind the playhead to bound memory usage.
///
/// All major DASH clients implement back-buffer eviction:
///   • Shaka Player: `bufferBehind` (default 30 s) — evicts via
///     `MediaSourceEngine.remove()` in `StreamingEngine.evict_()`.
///   • dash.js: `bufferToKeep` (default 20 s) — evicts in
///     `BufferController.pruneBuffer()`.
///   • hls.js: `backBufferLength` (default 30 s) — evicts in
///     `BufferController.onBufferFlushing()`.
///   • DASH-IF IOP v4.3 §3.2.8 recommends proactive buffer management.
///
/// **Critical design constraint:** `SourceBuffer.remove()` sets
/// `updating = true`, blocking the next `appendBuffer()`.  During this
/// time the browser cannot decode new audio/video frames from appended
/// segments.  dash.js mitigates this by only calling `pruneBuffer()`
/// when the total buffer approaches the browser's SourceBuffer quota —
/// NOT on every append based on back-buffer time.
///
/// We follow the same approach: only evict when `total_buffered` exceeds
/// `MSE_BACK_BUFFER_S + MSE_TARGET_BUFFER_S`.  When eviction is needed,
/// trim enough to bring the total one segment below the cap, giving
/// headroom for the next append without re-triggering eviction.
async fn evict_back_buffer(
    sb: &web_sys::SourceBuffer,
    video: &HtmlVideoElement,
    state: &Rc<RefCell<Option<MseState>>>,
    pump_id: u32,
) {
    let current = video.current_time();

    // Check if there's actually buffered data.
    let ranges = match sb.buffered() {
        Ok(r) => r,
        Err(_) => return,
    };
    let len = ranges.length();
    if len == 0 {
        return;
    }

    // Calculate total buffered as the SUM of actual range durations,
    // NOT (buf_end − buf_start) which over-counts when ranges are
    // disjoint (e.g. after a backward seek: [6–12] + [60–102] has
    // 48 s of data, not 96 s).  Matches dash.js BufferController
    // getBufferLength() which sums individual ranges.
    let mut total_buffered = 0.0_f64;
    for i in 0..len {
        if let (Ok(s), Ok(e)) = (ranges.start(i), ranges.end(i)) {
            total_buffered += e - s;
        }
    }

    let max_total = MSE_BACK_BUFFER_S + MSE_TARGET_BUFFER_S;

    // Only evict when total buffer exceeds the cap.
    // This prevents unnecessary sb.remove() calls that block the append
    // pipeline.  dash.js only prunes when approaching the quota, not on
    // every append based on back-buffer time.
    if total_buffered <= max_total {
        return;
    }

    // Find the buffered range(s) BEHIND the playhead and trim from the
    // earliest one.  Only remove data behind (current − MSE_BACK_BUFFER_S)
    // so we never evict data near or ahead of the playhead.
    //
    // dash.js BufferController.getClearRanges() computes:
    //   startRangeToKeep = max(0, currentTime − bufferToKeep)
    // and only clears ranges with start < startRangeToKeep.
    let keep_start = (current - MSE_BACK_BUFFER_S).max(0.0);

    // Walk ranges and find data strictly behind keep_start.
    let mut evict_start = f64::MAX;
    let mut evict_end = 0.0_f64;
    for i in 0..len {
        if let (Ok(s), Ok(e)) = (ranges.start(i), ranges.end(i)) {
            if s < keep_start {
                if s < evict_start {
                    evict_start = s;
                }
                // Only remove the portion behind keep_start.
                evict_end = evict_end.max(e.min(keep_start));
            }
        }
    }

    if evict_start >= evict_end || (evict_end - evict_start) < MIN_EVICT_S {
        return; // nothing worth evicting behind the playhead
    }

    // Wait for SourceBuffer to be idle before removing.
    if !wait_for_sb(sb, state, pump_id).await {
        return;
    }

    log::info!(
        "evict: removing [{evict_start:.3}–{evict_end:.3}] (total was {total_buffered:.1}s, current={current:.3})"
    );
    let _ = sb.remove(evict_start, evict_end);

    // Wait for the remove operation to complete.
    let _ = wait_for_sb(sb, state, pump_id).await;
}

/// Shared segment cache used by the deep prefetch pipeline.
///
/// Background fetch tasks (spawned via `spawn_local`) populate this cache
/// ahead of the append position.  The sequential append loop pulls from
/// it, so there's always data ready to append without waiting on HTTP.
///
/// The cache is keyed by segment index.  Entries are removed after the
/// append loop consumes them (to bound memory).  The `in_flight` set
/// prevents duplicate fetch tasks for the same segment.
type SegmentCache = Rc<RefCell<HashMap<usize, Vec<u8>>>>;
type InFlightSet = Rc<RefCell<std::collections::HashSet<usize>>>;

/// Kick off background fetch tasks for segments in the lookahead window.
///
/// For each segment in `[from_seg, from_seg + LOOKAHEAD_SEGMENTS)` that
/// is within range, not already cached, and not already being fetched,
/// spawns an async task to fetch it and store the bytes in `cache`.
///
/// This runs on every loop iteration of the pump to keep the pipeline
/// full.  Tasks self-cancel if the pump generation changes (seek/restart).
fn kick_prefetch(
    cache: &SegmentCache,
    in_flight: &InFlightSet,
    state: &Rc<RefCell<Option<MseState>>>,
    pump_id: u32,
    from_seg: usize,
) {
    let borrow = state.borrow();
    let mse = match borrow.as_ref() {
        Some(s) if s.pump_gen == pump_id => s,
        _ => return,
    };
    let total = mse.segments.len();

    let end_seg = (from_seg + LOOKAHEAD_SEGMENTS).min(total);
    for idx in from_seg..end_seg {
        // Atomically check-and-insert to prevent duplicate fetch tasks.
        // WASM is single-threaded, so a single borrow_mut scope is sufficient.
        {
            let cached = cache.borrow().contains_key(&idx);
            if cached {
                continue;
            }
            let mut flight = in_flight.borrow_mut();
            if flight.contains(&idx) {
                continue;
            }
            flight.insert(idx);
        }
        let url = mse.segments[idx].url.clone();

        let cache = Rc::clone(cache);
        let in_flight = Rc::clone(in_flight);
        let state = Rc::clone(state);
        spawn_local(async move {
            // Check generation before starting the fetch.
            // If stale, leave idx in in_flight to prevent re-spawning
            // tasks for a cancelled pump.
            if !is_pump_current(&state, pump_id) {
                return;
            }
            match Request::get(&url).send().await {
                Ok(r) => match r.binary().await {
                    Ok(bytes) => {
                        // Only store if pump is still current.
                        if is_pump_current(&state, pump_id) {
                            cache.borrow_mut().insert(idx, bytes);
                            log::info!("prefetch[{pump_id}]: cached segment {idx}");
                        }
                    }
                    Err(e) => log::warn!("prefetch seg {idx}: body error: {e:?}"),
                },
                Err(e) => log::warn!("prefetch seg {idx}: fetch error: {e:?}"),
            }
            in_flight.borrow_mut().remove(&idx);
        });
    }
}

/// Start (or restart) the async segment-pump loop.
///
/// Bumps `pump_gen` so any previously-running `pump_loop` detects the
/// mismatch on its next generation check and exits cleanly.  This is
/// analogous to dash.js aborting in-flight XHR requests on seek, and
/// Shaka Player's `StreamingEngine.seeked()` which resets the update cycle.
///
/// Call sites:
/// - MSE initialisation (after the init segment is appended)
/// - `seeking` event handler (repoints the pump at the seek target)
/// - 150 ms timer safety-net (only when `pump_running` is false)
fn start_pump(state: &Rc<RefCell<Option<MseState>>>, video: &HtmlVideoElement) {
    let pump_id = {
        let mut borrow = state.borrow_mut();
        match borrow.as_mut() {
            Some(s) => {
                // If a pump is already running for this generation, don't
                // start another one — that would cause duplicate requests.
                if s.pump_running {
                    return;
                }
                s.pump_gen = s.pump_gen.wrapping_add(1);
                s.pump_running = true;
                s.pump_gen
            }
            None => return,
        }
    };
    let state_c = state.clone();
    let video_c = video.clone();
    spawn_local(async move {
        pump_loop(state_c.clone(), video_c, pump_id).await;
        if let Some(s) = state_c.borrow_mut().as_mut() {
            if s.pump_gen == pump_id {
                s.pump_running = false;
            }
        }
    });
}

/// Force-start a new pump loop, cancelling any currently-running one.
///
/// Unlike `start_pump()`, this always bumps the generation counter and
/// spawns a new loop regardless of whether a pump is already running.
/// Used by the seek handler which must immediately repoint the pump.
fn force_start_pump(state: &Rc<RefCell<Option<MseState>>>, video: &HtmlVideoElement) {
    let pump_id = {
        let mut borrow = state.borrow_mut();
        match borrow.as_mut() {
            Some(s) => {
                s.pump_gen = s.pump_gen.wrapping_add(1);
                s.pump_running = true;
                s.pump_gen
            }
            None => return,
        }
    };
    let state_c = state.clone();
    let video_c = video.clone();
    spawn_local(async move {
        pump_loop(state_c.clone(), video_c, pump_id).await;
        if let Some(s) = state_c.borrow_mut().as_mut() {
            if s.pump_gen == pump_id {
                s.pump_running = false;
            }
        }
    });
}

/// Sequential async loop that fetches and appends DASH fMP4 segments.
///
/// This is the core scheduling loop, analogous to:
///   • dash.js `ScheduleController.schedule()` — decides when to fetch the
///     next segment based on buffer level vs. target.
///   • Shaka Player `StreamingEngine.update_()` — periodically checks buffer
///     and fetches segments to keep `bufferingGoal` seconds ahead.
///   • hls.js `StreamController.doTick()` — main loop that drives segment
///     fetching and appending.
///
/// **Deep prefetch pipeline:** Instead of fetching one segment at a time,
/// background tasks continuously pre-fetch segments into a shared cache
/// (`SegmentCache`) up to `LOOKAHEAD_SEGMENTS` ahead of the current
/// append position.  The sequential append loop pulls from this cache,
/// so it never blocks on HTTP latency.  This is critical when the backend
/// generates segments on-demand (each first fetch triggers server-side
/// muxing which can take hundreds of milliseconds).
///
/// Steps per iteration (per DASH-IF IOP v4.3 §3.2):
///   1. Kick off background prefetch tasks for the lookahead window.
///   2. Determine the next segment needed.
///   3. Enforce a forward buffer limit — sleep when enough data is buffered.
///   4. Evict old played data behind the playhead to bound memory.
///   5. Pull the segment from the prefetch cache (or fetch inline as fallback).
///   6. Strip redundant init boxes (ftyp/moov) from the fMP4 data.
///   7. Wait for the SourceBuffer to be idle, then `appendBuffer`.
///   8. Wait for `updateend`, advance to the next segment, repeat.
///
/// The loop exits when all segments are appended, the generation counter
/// no longer matches (a seek started a newer pump), or the MseState has
/// been dropped (component unmount / quality change).
async fn pump_loop(
    state: Rc<RefCell<Option<MseState>>>,
    video: HtmlVideoElement,
    pump_id: u32,
) {
    // ── Deep prefetch pipeline state ─────────────────────────────────
    // Background fetch tasks populate `segment_cache` ahead of the
    // append position.  `in_flight` prevents duplicate fetch tasks.
    let segment_cache: SegmentCache = Rc::new(RefCell::new(HashMap::new()));
    let in_flight: InFlightSet = Rc::new(RefCell::new(Default::default()));

    loop {
        // ── 1. Generation check ──────────────────────────────────────
        if !is_pump_current(&state, pump_id) {
            return;
        }

        // ── 2. Determine the next segment to fetch ───────────────────
        let (seg_url, seg_idx, sb, last_appended) = {
            let borrow = state.borrow();
            let mse = match borrow.as_ref() {
                Some(s) if s.pump_gen == pump_id => s,
                _ => return,
            };

            // All segments appended — signal EOS.
            if mse.next_seg >= mse.segments.len() {
                log::info!("pump[{pump_id}]: all {} segments appended, signalling EOS", mse.segments.len());
                let _ = mse.media_source.end_of_stream();
                return;
            }

            (
                mse.segments[mse.next_seg].url.clone(),
                mse.next_seg,
                mse.source_buffer.clone(),
                mse.last_appended_seg,
            )
        };

        // ── 3. Kick off deep prefetch for the lookahead window ───────
        // Ensures background fetches are running for upcoming segments.
        // Each call is cheap — it only spawns tasks for segments not
        // already cached or in-flight.
        kick_prefetch(&segment_cache, &in_flight, &state, pump_id, seg_idx);

        // ── 4. Buffer-ahead gate ─────────────────────────────────────
        // Check how much data is buffered ahead of the playhead using the
        // actual SourceBuffer.buffered() ranges.  If we have enough data,
        // sleep instead of appending more — this prevents over-buffering
        // which causes the browser to hit its SourceBuffer quota and
        // emergency-evict data near the playhead.
        //
        // dash.js uses `bufferLevel` (actual buffered time ahead of
        // playhead) compared against `bufferTimeAtTopQuality` (12 s).
        // We use 30 s for VOD.
        //
        // Falls back to segment-index estimation when buffered() is
        // unavailable (e.g. immediately after seek+flush).
        {
            let current = video.current_time();
            let buf_ahead = if let Ok(ranges) = sb.buffered() {
                // Find the buffered range containing the playhead.
                let mut ahead = 0.0_f64;
                for i in 0..ranges.length() {
                    if let (Ok(s), Ok(e)) = (ranges.start(i), ranges.end(i)) {
                        if current >= s - PLAYHEAD_RANGE_TOLERANCE_S && current <= e + PLAYHEAD_RANGE_TOLERANCE_S {
                            ahead = (e - current).max(0.0);
                            break;
                        }
                    }
                }
                ahead
            } else if let Some(last_seg) = last_appended {
                // Fallback: estimate from segment indices.
                let buffered_to = (last_seg as f64 + 1.0) * SEGMENT_DURATION_F;
                (buffered_to - current).max(0.0)
            } else {
                0.0
            };

            if buf_ahead >= MSE_TARGET_BUFFER_S {
                // Enough data buffered ahead — sleep and re-check.
                // Keep prefetching in the background while we wait.
                // Use 200 ms (not 500 ms) for responsive scheduling —
                // matches dash.js ScheduleController which reschedules
                // quickly when buffer level changes.
                TimeoutFuture::new(200).await;
                continue;
            }
        }

        // ── 5. Evict old data behind the playhead ────────────────────
        // Proactively evict BEFORE appending to prevent the browser from
        // hitting its SourceBuffer quota.  Browser emergency eviction
        // removes data unpredictably (sometimes near the playhead),
        // causing audio dropout at segment transitions.
        //
        // Ref: dash.js BufferController.pruneBuffer() — runs before
        //      each append, not after.
        evict_back_buffer(&sb, &video, &state, pump_id).await;
        if !is_pump_current(&state, pump_id) {
            return;
        }

        // ── 6. Pull segment from prefetch cache (or fetch inline) ────
        // The deep prefetch pipeline should have this segment ready.
        // Poll the cache with yields to give background fetch tasks
        // time to complete.  Fall back to inline fetch if the cache
        // doesn't fill within a reasonable time.
        let fetch_start_ms = js_sys::Date::now();
        let bytes = {
            let mut data = None;
            // Give the background prefetch up to ~3 s to deliver.
            // 50 ms intervals × 60 iterations = 3 s max wait.
            for _ in 0..60 {
                if let Some(cached) = segment_cache.borrow_mut().remove(&seg_idx) {
                    data = Some(cached);
                    break;
                }
                // Yield to let background fetch tasks run.
                TimeoutFuture::new(50).await;
                if !is_pump_current(&state, pump_id) {
                    return;
                }
            }
            match data {
                Some(b) => {
                    log::info!("pump[{pump_id}]: using cached segment {seg_idx}");
                    b
                }
                None => {
                    // Fallback: fetch inline.  This defeats the purpose of
                    // prefetching and may cause playback stutter.
                    log::warn!("pump[{pump_id}]: cache miss for segment {seg_idx} — prefetch did not complete in time, fetching inline");
                    match Request::get(&seg_url).send().await {
                        Ok(r) => match r.binary().await {
                            Ok(b) => b,
                            Err(e) => {
                                log::error!("segment {seg_idx}: body read error: {e:?}");
                                TimeoutFuture::new(1000).await;
                                continue;
                            }
                        },
                        Err(e) => {
                            log::error!("segment {seg_idx}: fetch error: {e:?}");
                            TimeoutFuture::new(1000).await;
                            continue;
                        }
                    }
                }
            }
        };

        // Re-check generation after potentially waiting for cache.
        if !is_pump_current(&state, pump_id) {
            return;
        }

        // Record throughput measurement for ABR.
        {
            let fetch_end_ms = js_sys::Date::now();
            let elapsed_ms = (fetch_end_ms - fetch_start_ms).max(1.0);
            let throughput_kbps = (bytes.len() as f64 * 8.0) / elapsed_ms;
            let mut borrow = state.borrow_mut();
            if let Some(mse) = borrow.as_mut() {
                if let Some(ext) = mse.dash_ext.as_mut() {
                    ext.throughput.add_measurement(MediaType::Video, throughput_kbps, elapsed_ms);
                    ext.metrics.add_throughput_sample(ThroughputSample {
                        timestamp_ms: fetch_end_ms,
                        throughput_kbps,
                        latency_ms: elapsed_ms,
                        bytes: bytes.len(),
                        duration_ms: elapsed_ms,
                        media_type: MediaType::Video,
                    });
                    ext.event_bus.emit_simple(PlayerEvent::FragmentLoadingCompleted);
                }
            }
        }

        // ── 7. Strip init boxes & append ─────────────────────────────
        let media_bytes = strip_init_boxes(&bytes);
        if media_bytes.is_empty() {
            log::warn!("segment {seg_idx}: no media data after stripping init boxes (original size: {} bytes)", bytes.len());
            // Advance past this empty segment and try the next one.
            if let Some(s) = state.borrow_mut().as_mut() {
                if s.pump_gen == pump_id {
                    s.next_seg = seg_idx + 1;
                }
            }
            continue;
        }

        if !wait_for_sb(&sb, &state, pump_id).await {
            return;
        }

        // ── 7b. Append without per-segment appendWindow ──────────────
        //
        // dash.js (SourceBufferSink.js) only uses appendWindow for
        // multi-period transitions.  For single-period VOD content the
        // window covers the entire presentation [0, duration], so no
        // per-segment clipping is performed.
        //
        // Ref: dash.js SourceBufferSink.js — updateAppendWindow() sets
        //      window to [periodStart − 0.1, periodEnd + 0.01], NOT to
        //      individual segment boundaries.
        // Ref: DASH-IF IOP v4.3 §3.2 — segments start at SAPs; the
        //      browser de-duplicates overlapping data via baseMediaDecodeTime.

        let uint8_array = js_sys::Uint8Array::from(media_bytes.as_slice());
        let array_buffer = uint8_array.buffer();
        if sb.append_buffer_with_array_buffer(&array_buffer).is_err() {
            log::error!("segment {seg_idx}: appendBuffer failed, evicting and retrying");
            // Likely QuotaExceededError — evict aggressively and retry.
            evict_back_buffer(&sb, &video, &state, pump_id).await;
            TimeoutFuture::new(500).await;
            continue;
        }

        // ── 8. Wait for updateend ────────────────────────────────────
        // Background prefetch tasks continue running in parallel — the
        // browser handles HTTP requests on its I/O thread even while
        // we're waiting for the SourceBuffer.
        if !wait_for_sb(&sb, &state, pump_id).await {
            return;
        }

        // ── 9. Advance to next segment ───────────────────────────────
        {
            // Log buffered ranges after append for diagnostics.
            if let Ok(buffered) = sb.buffered() {
                let len = buffered.length();
                let mut ranges = String::new();
                for i in 0..len {
                    if let (Ok(s), Ok(e)) = (buffered.start(i), buffered.end(i)) {
                        if !ranges.is_empty() {
                            ranges.push_str(", ");
                        }
                        ranges.push_str(&format!("[{s:.3}–{e:.3}]"));
                    }
                }
                log::info!(
                    "pump[{pump_id}]: seg {seg_idx} appended → buffered: {ranges}"
                );
            }

            let mut borrow = state.borrow_mut();
            if let Some(s) = borrow.as_mut() {
                if s.pump_gen != pump_id {
                    return;
                }
                s.next_seg = seg_idx + 1;
                s.last_appended_seg = Some(seg_idx);
                log::info!(
                    "pump[{pump_id}]: appended segment {seg_idx}, next_seg={}",
                    s.next_seg
                );
            }
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
    // re-initialisation of the DASH stream.
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

                // Embed the selected quality in the manifest URL so the server
                // returns segment URLs for the correct quality level.
                let manifest_url = format!(
                    "/api/videos/{}/manifest.mpd?quality={}",
                    video_id, quality
                );

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
                let manifest_url_for_open = manifest_url.clone();
                let video_for_open = video.clone();
                let status_for_open = status_clone.clone();
                let error_for_open = error_clone.clone();
                let mse_state_for_open = mse_state_clone.clone();
                let media_source_for_open = media_source.clone();
                let object_url_for_open = object_url.clone();

                let sourceopen_cb = Closure::once(Box::new(move || {
                    let manifest_url = manifest_url_for_open;
                    let video = video_for_open;
                    let status = status_for_open;
                    let error = error_for_open;
                    let mse_state = mse_state_for_open;
                    let media_source = media_source_for_open;
                    let object_url = object_url_for_open;

                    spawn_local(async move {
                        // Fetch the DASH MPD manifest.
                        let resp = match Request::get(&manifest_url).send().await {
                            Ok(r) => r,
                            Err(e) => {
                                error.set(Some(format!("Failed to fetch manifest: {e:?}")));
                                return;
                            }
                        };
                        let text = match resp.text().await {
                            Ok(t) => t,
                            Err(e) => {
                                error.set(Some(format!("Failed to read manifest: {e:?}")));
                                return;
                            }
                        };

                        // Parse segment list from MPD.
                        let (init_url, total_duration, segments) = parse_mpd(&text);
                        if segments.is_empty() {
                            error.set(Some("Manifest contains no segments.".to_string()));
                            return;
                        }
                        if init_url.is_empty() {
                            error.set(Some("Manifest missing init segment URL.".to_string()));
                            return;
                        }

                        // fMP4 segments use video/mp4 MIME type with codecs.
                        // dash.js and Shaka Player read the codec string from
                        // the MPD's Representation@codecs attribute.  Since our
                        // MPD doesn't include codecs, we probe the browser with
                        // isTypeSupported() — similar to how hls.js probes
                        // codec support in BufferController before creating
                        // SourceBuffers.
                        //
                        // Ref: https://github.com/video-dev/hls.js/blob/master/src/controller/buffer-controller.ts
                        // Ref: https://github.com/Dash-Industry-Forum/dash.js (TextController codec probing)
                        let mime_candidates = [
                            "video/mp4; codecs=\"avc1.640029,mp4a.40.2\"",  // H.264 High L4.1, AAC-LC
                            "video/mp4; codecs=\"avc1.64001F,mp4a.40.2\"",  // H.264 High L3.1, AAC-LC
                            "video/mp4; codecs=\"avc1.4D4028,mp4a.40.2\"",  // H.264 Main L4.0, AAC-LC
                            "video/mp4; codecs=\"avc1.42E01E,mp4a.40.2\"",  // H.264 Baseline L3.0, AAC-LC
                            "video/mp4; codecs=\"avc1.640029,mp4a.40.5\"",  // H.264 High, HE-AAC
                            "video/mp4; codecs=\"avc1.640029,mp3\"",        // H.264 High, MP3
                            "video/mp4",                                     // Generic fallback
                        ];
                        let mime = mime_candidates.iter()
                            .find(|m| web_sys::MediaSource::is_type_supported(m))
                            .or(mime_candidates.last())
                            .unwrap();
                        log::info!("MSE: using MIME type: {mime}");

                        // Create the SourceBuffer.
                        let source_buffer = match media_source.add_source_buffer(mime) {
                            Ok(sb) => sb,
                            Err(e) => {
                                error.set(Some(format!(
                                    "Unsupported stream format. Try a different quality level. ({e:?})"
                                )));
                                return;
                            }
                        };

                        // Use the default "segments" mode — the browser
                        // positions each appended fragment on the timeline
                        // using the baseMediaDecodeTime from the moof/tfdt
                        // boxes, with no automatic timestampOffset adjustment.
                        // This is the mode used by dash.js's SourceBufferSink
                        // (it never calls set_mode()) and by Shaka Player.
                        //
                        // The backend rebases PTS so segment N starts at
                        // N × SEGMENT_DURATION_F (6 s), which means Segments
                        // mode places each fragment at the correct absolute
                        // time without any client-side offset management.
                        //
                        // Ref: dash.js SourceBufferSink.initializeForFirstUse()
                        //      — never calls sourceBuffer.mode = 'sequence'
                        // Ref: DASH-IF IOP v4.3 §3.2
                        log::info!("MSE: SourceBuffer created in Segments mode (default)");

                        // Set the total presentation duration from the MPD so
                        // the browser knows the full video length.
                        if total_duration > 0.0 {
                            media_source.set_duration(total_duration);
                        }

                        // Fetch and append the init segment (ftyp+moov) first.
                        let init_bytes = match Request::get(&init_url).send().await {
                            Ok(r) => match r.binary().await {
                                Ok(b) => b,
                                Err(e) => {
                                    error.set(Some(format!("Failed to read init segment: {e:?}")));
                                    return;
                                }
                            },
                            Err(e) => {
                                error.set(Some(format!("Failed to fetch init segment: {e:?}")));
                                return;
                            }
                        };

                        // Append the init segment to the SourceBuffer.
                        let uint8_array = js_sys::Uint8Array::from(init_bytes.as_slice());
                        let array_buffer = uint8_array.buffer();
                        if source_buffer
                            .append_buffer_with_array_buffer(&array_buffer)
                            .is_err()
                        {
                            error.set(Some("Failed to append init segment.".to_string()));
                            return;
                        }

                        // Wait for the init segment append to complete
                        // (simple polling — no leaked event listeners).
                        for _ in 0..200 {
                            if !source_buffer.updating() {
                                break;
                            }
                            TimeoutFuture::new(5).await;
                        }

                        // Calculate which segment to start from when resuming.
                        let start_seg = if start_pos > 0.0 {
                            segment_for_time(start_pos)
                        } else {
                            0
                        };

                        // Store MSE state.
                        let full_mpd = parse_mpd_full(&text);
                        let live_ctrl = if full_mpd.mpd_type == MpdType::Dynamic {
                            let mut lc = LiveStreamController::new();
                            lc.configure_from_mpd(&full_mpd);
                            Some(lc)
                        } else {
                            None
                        };
                        *mse_state.borrow_mut() = Some(MseState {
                            media_source,
                            source_buffer: source_buffer.clone(),
                            object_url,
                            segments,
                            next_seg: start_seg,
                            pump_gen: 0,
                            pump_running: false,
                            last_appended_seg: None,
                            dash_ext: Some(DashExtState {
                                mpd: full_mpd,
                                abr: AbrController::new(),
                                throughput: ThroughputController::new(),
                                metrics: DashMetrics::new(),
                                event_bus: EventBus::new(),
                                error_recovery: ErrorRecovery::new(),
                                live: live_ctrl,
                            }),
                        });

                        status.set(String::new());
                        if start_pos > 0.0 {
                            video.set_current_time(start_pos);
                        }

                        // Start the sequential async pump loop (per DASH-IF
                        // IOP v4.3 §3.2: fetch → append → wait → repeat).
                        start_pump(&mse_state, &video);
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
                if let Some(state) = mse_state_for_cleanup.borrow_mut().take() {
                    let _ = state.media_source.end_of_stream();
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

    // Update time/duration periodically.  Also acts as a safety-net to
    // restart the pump loop if it exited unexpectedly (e.g. a transient
    // network error exhausted its retries).
    {
        let video_ref = video_ref.clone();
        let current_time = current_time.clone();
        let duration = duration.clone();
        let buffered_end = buffered_end.clone();
        let is_playing = is_playing.clone();
        let is_dragging = is_dragging.clone();
        let video_ended = video_ended.clone();
        let mse_state = mse_state.clone();

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
                    buffered_end.set(buffered_end_at(&video, video.current_time()));
                    is_playing.set(!video.paused());

                    // Check if video ended
                    video_ended.set(video.ended());

                    // Periodic gap-jump safety net (dash.js GapController
                    // runs on a similar periodic interval).  Catches gaps
                    // that don't always trigger the `waiting` event.
                    //
                    // Guard: skip while seeking — dash.js GapController
                    // _shouldCheckForGaps() returns false when
                    // playbackController.isSeeking() is true.  Without
                    // this, backward seeks to unbuffered positions get
                    // defeated by forward gap-jumps.
                    if !video.paused() && !video.ended() && !video.seeking() && video.ready_state() <= 2 {
                        try_jump_gap(&video);
                    }

                    // Safety-net: restart the pump when it has exited
                    // (pump_running == false) but there are still segments
                    // to fetch and the buffer is below target.
                    //
                    // Uses actual buffered() ranges when available, with
                    // segment-index fallback.  This mirrors dash.js
                    // ScheduleController's periodic buffer check.
                    let needs_restart = {
                        let borrow = mse_state.borrow();
                        if let Some(s) = borrow.as_ref() {
                            if s.pump_running || s.next_seg >= s.segments.len() {
                                false
                            } else {
                                let current = video.current_time();
                                let buf_ahead = if let Ok(ranges) = s.source_buffer.buffered() {
                                    let mut ahead = 0.0_f64;
                                    for i in 0..ranges.length() {
                                        if let (Ok(rs), Ok(re)) = (ranges.start(i), ranges.end(i)) {
                                            if current >= rs - PLAYHEAD_RANGE_TOLERANCE_S && current <= re + PLAYHEAD_RANGE_TOLERANCE_S {
                                                ahead = (re - current).max(0.0);
                                                break;
                                            }
                                        }
                                    }
                                    ahead
                                } else {
                                    match s.last_appended_seg {
                                        Some(last) => {
                                            let buffered_to = (last as f64 + 1.0) * SEGMENT_DURATION_F;
                                            (buffered_to - current).max(0.0)
                                        }
                                        None => 0.0,
                                    }
                                };
                                buf_ahead < MSE_TARGET_BUFFER_S
                            }
                        } else {
                            false
                        }
                    };
                    if needs_restart {
                        start_pump(&mse_state, &video);
                    }
                }
            });
            move || drop(interval)
        });
    }

    // Detect buffering via waiting/playing events (NOT readyState polling,
    // which gives false positives during appendBuffer operations).
    //
    // The `waiting` handler also implements gap-jumping — modelled after
    // dash.js `GapController`.  When playback stalls at a small gap between
    // segments, we nudge the playhead past it.
    {
        let video_ref = video_ref.clone();
        let is_buffering = is_buffering.clone();

        use_effect_with(video_ref.clone(), move |video_ref| {
            let video_opt = video_ref.cast::<HtmlVideoElement>();

            let waiting_cb = video_opt.as_ref().map(|video| {
                let is_buffering = is_buffering.clone();
                let video_for_gap = video.clone();
                let cb = Closure::<dyn Fn()>::new(move || {
                    // Try to jump over a small gap first (dash.js
                    // GapController pattern).  Only show buffering spinner
                    // if there's no jumpable gap.
                    //
                    // Guard: skip while seeking — matches dash.js
                    // GapController._shouldCheckForGaps().
                    if !video_for_gap.seeking() && !try_jump_gap(&video_for_gap) {
                        is_buffering.set(true);
                    }
                });
                video
                    .add_event_listener_with_callback("waiting", cb.as_ref().unchecked_ref())
                    .ok();
                cb
            });

            let playing_cb = video_opt.as_ref().map(|video| {
                let is_buffering = is_buffering.clone();
                let cb = Closure::<dyn Fn()>::new(move || {
                    is_buffering.set(false);
                });
                video
                    .add_event_listener_with_callback("playing", cb.as_ref().unchecked_ref())
                    .ok();
                cb
            });

            let video_opt_cleanup = video_opt.clone();
            move || {
                if let Some(video) = video_opt_cleanup {
                    if let Some(cb) = waiting_cb {
                        video
                            .remove_event_listener_with_callback(
                                "waiting",
                                cb.as_ref().unchecked_ref(),
                            )
                            .ok();
                    }
                    if let Some(cb) = playing_cb {
                        video
                            .remove_event_listener_with_callback(
                                "playing",
                                cb.as_ref().unchecked_ref(),
                            )
                            .ok();
                    }
                }
            }
        });
    }

    // Handle seeks — modelled after how major DASH clients react to seeks:
    //
    //  • dash.js: PlaybackController listens for the `seeking` event, aborts
    //    any in-flight segment requests, resets BufferController, and
    //    reschedules downloads from the new position.
    //    Source: dash.js/src/streaming/controllers/PlaybackController.js
    //
    //  • Shaka Player: `Player.onSeeking_()` cancels outstanding segment
    //    requests and calls `StreamingEngine.seeked()` which clears its
    //    internal state and restarts the update cycle from the new position.
    //    Source: shaka-player/lib/player.js
    //
    //  • DASH-IF IOP v4.3 §3.2.4 — the client should react on the `seeking`
    //    event (not `seeked` — reacting earlier avoids fetching intermediate
    //    segments).  If the target is already buffered, continue; otherwise
    //    cancel the current download and start from the target segment.
    //
    // In Segments mode, the browser uses baseMediaDecodeTime from each
    // segment's moof/tfdt to place fragments on the timeline.  No
    // timestampOffset adjustment or buffer flush is needed — just cancel
    // the pump and restart from the target segment.
    //
    // Firefox fires `seeking` up to 7 times for a single user seek, so
    // we only bump the pump generation when `next_seg` actually changes.
    {
        let video_ref = video_ref.clone();
        let mse_state = mse_state.clone();

        use_effect_with(video_ref.clone(), move |video_ref| {
            let video_opt = video_ref.cast::<HtmlVideoElement>();

            let seeking_cb = video_opt.as_ref().map(|video| {
                let mse_state_for_seek = mse_state.clone();
                let video_for_seek = video.clone();

                let cb = Closure::<dyn Fn()>::new(move || {
                    let seek_time = video_for_seek.current_time();
                    let target_seg = segment_for_time(seek_time);

                    if is_time_buffered(&video_for_seek, seek_time) {
                        // Seek target is already in a buffered range —
                        // the browser handles this natively.  Just ensure
                        // the pump will keep filling ahead.
                        let need_pump = {
                            let borrow = mse_state_for_seek.borrow();
                            if let Some(mse) = borrow.as_ref() {
                                !mse.pump_running
                            } else {
                                false
                            }
                        };
                        if need_pump {
                            start_pump(&mse_state_for_seek, &video_for_seek);
                        }
                    } else {
                        // Seek target is NOT buffered.  In Segments mode the
                        // browser places fragments via baseMediaDecodeTime, so
                        // we just cancel the pump and restart from the target
                        // segment — no buffer flush or timestampOffset needed.
                        //
                        // Modelled after dash.js PlaybackController.onPlaybackSeeking():
                        //   → clearScheduleTimer() + fragmentModel.abortRequests()
                        //   → setExplicitBufferingTime(targetTime)
                        //   → scheduleController.startScheduleTimer()
                        log::info!("seek: target {seek_time:.1}s not buffered, restarting from segment {target_seg}");

                        {
                            let mut borrow = mse_state_for_seek.borrow_mut();
                            if let Some(mse) = borrow.as_mut() {
                                mse.pump_gen = mse.pump_gen.wrapping_add(1);
                                mse.pump_running = false;
                                mse.next_seg = target_seg;
                                mse.last_appended_seg = None;
                            } else {
                                return;
                            }
                        }

                        // Flush stale buffered data ahead of the seek target.
                        //
                        // Without this, old SourceBuffer ranges ahead of the new seek
                        // position survive as disjoint ranges (e.g. [60–66] + [114–120]).
                        // Once seeking completes (video.seeking() == false), the 150ms
                        // gap-controller timer sees the [66s–114s] gap and jumps the
                        // playhead to 114s — defeating the backward seek.
                        //
                        // This mirrors dash.js BufferController.onPlaybackSeeking():
                        //   this.sourceBuffer.remove(seekTime, this.mediaSource.duration)
                        //
                        // The pump's wait_for_sb() at the top of pump_loop handles
                        // the SB being busy after a remove() — safe to fire-and-forget.
                        {
                            let borrow = mse_state_for_seek.borrow();
                            if let Some(mse) = borrow.as_ref() {
                                let sb = &mse.source_buffer;
                                // Remove everything from the end of the seek-target segment
                                // to infinity, clearing all stale ranges ahead.
                                let flush_start = (target_seg as f64 + 1.0) * SEGMENT_DURATION_F;
                                if !sb.updating() {
                                    match sb.remove(flush_start, f64::INFINITY) {
                                        Ok(_) => log::info!(
                                            "seek: flushing stale SB data [{flush_start:.1}s, ∞) ahead of seek to {seek_time:.1}s"
                                        ),
                                        Err(e) => log::warn!(
                                            "seek: sb.remove({flush_start:.1}, ∞) failed: {e:?}"
                                        ),
                                    }
                                }
                            }
                        }

                        force_start_pump(&mse_state_for_seek, &video_for_seek);
                    }
                });

                video
                    .add_event_listener_with_callback("seeking", cb.as_ref().unchecked_ref())
                    .ok();
                cb
            });

            move || {
                if let (Some(cb), Some(video)) = (seeking_cb, video_opt) {
                    video
                        .remove_event_listener_with_callback(
                            "seeking",
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

    // Settings toggle removed - gear icon had no functional purpose

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
    // the DASH stream at the new quality level, resuming from the current playback position.
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
                        let current = video.current_time();
                        video.set_current_time((current - 10.0).max(0.0));
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
