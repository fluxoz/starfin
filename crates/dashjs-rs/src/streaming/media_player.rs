//! Port of `dash.js/src/streaming/MediaPlayer.js`.
//!
//! The `MediaPlayer` is the primary facade for controlling MPEG-DASH playback.
//! It owns all internal controllers, models, and metrics, and exposes a
//! public API that mirrors the dash.js `MediaPlayer` class.

use crate::core::event_bus::{EventBus, EventData, HandlerId};
use crate::core::events::Event;
use crate::core::logger::Logger;
use crate::core::settings::Settings;
use crate::dash::dash_metrics::DashMetrics;
use crate::streaming::controllers::buffer_controller::BufferController;
use crate::streaming::controllers::catchup_controller::CatchupController;
use crate::streaming::controllers::gap_controller::GapController;
use crate::streaming::controllers::media_controller::{MediaController, TrackInfo};
use crate::streaming::controllers::playback_controller::PlaybackController;
use crate::streaming::controllers::schedule_controller::ScheduleController;
use crate::streaming::controllers::stream_controller::StreamController;
use crate::streaming::controllers::throughput_controller::ThroughputController;
use crate::streaming::controllers::base_url_controller::BaseUrlController;
use crate::streaming::controllers::event_controller::EventController;
use crate::streaming::controllers::fragment_controller::FragmentController;
use crate::streaming::controllers::media_source_controller::MediaSourceController;
use crate::streaming::controllers::time_sync_controller::TimeSyncController;
use crate::streaming::models::media_player_model::MediaPlayerModel;
use crate::streaming::models::metrics_model::MetricsModel;
use crate::streaming::models::fragment_model::FragmentModel;
use crate::streaming::models::throughput_model::ThroughputModel;
use crate::streaming::protection::ProtectionController;
use crate::streaming::text::TextController;
use crate::streaming::thumbnail::ThumbnailController;
use crate::streaming::vo::BitrateInfo;

/// Library version string, mirroring `MediaPlayer.VERSION` in dash.js.
pub const VERSION: &str = env!("CARGO_PKG_VERSION");

// ─── Player lifecycle state ──────────────────────────────────────────────────

/// Lifecycle state of the `MediaPlayer`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PlayerState {
    Created,
    Initialized,
    Playing,
    Paused,
    Stopped,
    Error,
    Ended,
}

impl Default for PlayerState {
    fn default() -> Self {
        Self::Created
    }
}

// ─── MediaPlayer ─────────────────────────────────────────────────────────────

/// The main player facade — owns all controllers and coordinates them.
///
/// Modelled after `dash.js/src/streaming/MediaPlayer.js`.
pub struct MediaPlayer {
    // Lifecycle
    state: PlayerState,

    // Source
    source: Option<String>,
    auto_play: bool,

    // Core infrastructure
    event_bus: EventBus,
    settings: Settings,
    logger: Logger,

    // Audio / video state
    volume: f64,
    muted: bool,

    // Quality per media type (indexed by "video", "audio", …)
    quality: std::collections::HashMap<String, usize>,

    // Controllers
    playback_controller: PlaybackController,
    stream_controller: StreamController,
    buffer_controller: BufferController,
    schedule_controller: ScheduleController,
    abr_controller: crate::streaming::controllers::abr_controller::AbrController,
    gap_controller: GapController,
    throughput_controller: ThroughputController,
    catchup_controller: CatchupController,
    media_source_controller: MediaSourceController,
    media_controller: MediaController,
    event_controller: EventController,
    base_url_controller: BaseUrlController,
    time_sync_controller: TimeSyncController,
    fragment_controller: FragmentController,
    protection_controller: ProtectionController,
    text_controller: TextController,
    thumbnail_controller: ThumbnailController,

    // Models
    media_player_model: MediaPlayerModel,
    metrics_model: MetricsModel,
    fragment_model: FragmentModel,
    throughput_model: ThroughputModel,

    // Metrics
    dash_metrics: DashMetrics,
}

impl std::fmt::Debug for MediaPlayer {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("MediaPlayer")
            .field("state", &self.state)
            .field("source", &self.source)
            .field("auto_play", &self.auto_play)
            .finish()
    }
}

impl Default for MediaPlayer {
    fn default() -> Self {
        Self::new()
    }
}

impl MediaPlayer {
    // ── Factory / constructor ─────────────────────────────────────────────

    /// Create a new `MediaPlayer` in the `Created` state.
    ///
    /// Equivalent to `dashjs.MediaPlayer().create()`.
    pub fn create() -> Self {
        Self::new()
    }

    /// Create a new `MediaPlayer` in the `Created` state.
    pub fn new() -> Self {
        let settings = Settings::default();
        Self {
            state: PlayerState::Created,
            source: None,
            auto_play: false,
            event_bus: EventBus::new(),
            logger: Logger::new("MediaPlayer"),
            volume: 1.0,
            muted: false,
            quality: std::collections::HashMap::new(),
            playback_controller: PlaybackController::new(),
            stream_controller: StreamController::new(),
            buffer_controller: BufferController::new(),
            schedule_controller: ScheduleController::new(),
            abr_controller:
                crate::streaming::controllers::abr_controller::AbrController::new(),
            gap_controller: GapController::new(),
            throughput_controller: ThroughputController::new(),
            catchup_controller: CatchupController::new(),
            media_source_controller: MediaSourceController::new(),
            media_controller: MediaController::new(),
            event_controller: EventController::new(),
            base_url_controller: BaseUrlController::new(),
            time_sync_controller: TimeSyncController::new(),
            fragment_controller: FragmentController::new(),
            protection_controller: ProtectionController::new(),
            text_controller: TextController::new(),
            thumbnail_controller: ThumbnailController::new(),
            media_player_model: MediaPlayerModel::new(settings.clone()),
            metrics_model: MetricsModel::new(),
            fragment_model: FragmentModel::new(),
            throughput_model: ThroughputModel::new(),
            dash_metrics: DashMetrics::new(),
            settings,
        }
    }

    // ── Initialisation ───────────────────────────────────────────────────

    /// Initialise the player, optionally attaching a source and enabling
    /// auto-play.
    ///
    /// Mirrors `MediaPlayer.initialize(source, autoPlay)`.
    pub fn initialize(&mut self, source: Option<&str>, auto_play: bool) {
        if self.state != PlayerState::Created {
            self.logger.warn("initialize() called on an already-initialised player");
            return;
        }

        self.auto_play = auto_play;
        self.stream_controller.initialize(auto_play);
        self.gap_controller.initialize();

        if let Some(url) = source {
            self.source = Some(url.to_owned());
        }

        self.state = if auto_play {
            PlayerState::Playing
        } else {
            PlayerState::Initialized
        };

        self.event_bus.trigger(
            Event::PlaybackInitialized,
            EventData::default(),
        );

        self.logger.info("Player initialised");
    }

    // ── Ready / dynamic ──────────────────────────────────────────────────

    /// Returns `true` when the player has been initialised and has a source
    /// attached.
    pub fn is_ready(&self) -> bool {
        self.state != PlayerState::Created && self.source.is_some()
    }

    /// Returns `true` when the loaded manifest is dynamic (live).
    pub fn is_dynamic(&self) -> bool {
        self.playback_controller.is_dynamic()
    }

    // ── Source ────────────────────────────────────────────────────────────

    /// Attach a manifest URL to the player. If the player is already
    /// initialised, this triggers a source switch.
    pub fn attach_source(&mut self, url: &str) {
        self.source = Some(url.to_owned());
        self.logger.info(&format!("Source attached: {url}"));
        self.event_bus.trigger(
            Event::ManifestLoadingStarted,
            EventData::default(),
        );
    }

    /// Returns the currently attached source URL, if any.
    pub fn get_source(&self) -> Option<&str> {
        self.source.as_deref()
    }

    // ── Playback control ─────────────────────────────────────────────────

    /// Start or resume playback.
    pub fn play(&mut self) {
        if self.state == PlayerState::Created {
            self.logger.warn("play() called before initialize()");
            return;
        }
        self.playback_controller.play();
        self.state = PlayerState::Playing;
        self.event_bus.trigger(Event::PlaybackStarted, EventData::default());
    }

    /// Pause playback.
    pub fn pause(&mut self) {
        if self.state == PlayerState::Created {
            self.logger.warn("pause() called before initialize()");
            return;
        }
        self.playback_controller.pause();
        self.state = PlayerState::Paused;
        self.event_bus.trigger(Event::PlaybackPaused, EventData::default());
    }

    /// Seek to `time` (seconds).
    pub fn seek(&mut self, time: f64) {
        if self.state == PlayerState::Created {
            self.logger.warn("seek() called before initialize()");
            return;
        }
        self.playback_controller.seek(time);
        self.event_bus.trigger(Event::PlaybackSeeking, EventData::default());
    }

    /// Returns `true` when the player is paused.
    pub fn is_paused(&self) -> bool {
        self.playback_controller.is_paused()
    }

    /// Returns `true` when the player is currently seeking.
    pub fn is_seeking(&self) -> bool {
        self.playback_controller.is_seeking()
    }

    // ── Time / duration ──────────────────────────────────────────────────

    /// Current playback position in seconds.
    pub fn time(&self) -> f64 {
        self.playback_controller.get_time()
    }

    /// Total presentation duration in seconds.
    pub fn duration(&self) -> f64 {
        self.playback_controller.get_duration()
    }

    /// Returns the buffer length for the given media type (seconds).
    pub fn get_buffer_length(&self, media_type: &str) -> f64 {
        self.dash_metrics.get_current_buffer_level(media_type)
    }

    // ── Playback rate ────────────────────────────────────────────────────

    /// Get the current playback rate (1.0 = normal).
    pub fn get_playback_rate(&self) -> f64 {
        self.playback_controller.get_playback_rate()
    }

    /// Set the playback rate.
    pub fn set_playback_rate(&mut self, rate: f64) {
        self.playback_controller.set_playback_rate(rate);
        self.event_bus.trigger(Event::PlaybackRateChanged, EventData::default());
    }

    // ── Volume / mute ────────────────────────────────────────────────────

    /// Get the current volume (0.0 – 1.0).
    pub fn get_volume(&self) -> f64 {
        self.volume
    }

    /// Set volume (clamped to 0.0 – 1.0).
    pub fn set_volume(&mut self, vol: f64) {
        self.volume = vol.clamp(0.0, 1.0);
        self.event_bus.trigger(Event::PlaybackVolumeChanged, EventData::default());
    }

    /// Mute or un-mute the player.
    pub fn set_mute(&mut self, muted: bool) {
        self.muted = muted;
        self.event_bus.trigger(Event::PlaybackVolumeChanged, EventData::default());
    }

    /// Returns `true` when the player is muted.
    pub fn is_muted(&self) -> bool {
        self.muted
    }

    // ── Auto-play ────────────────────────────────────────────────────────

    /// Set the auto-play flag. Must be called before `initialize()` or
    /// `attach_source()` for the flag to take effect.
    pub fn set_auto_play(&mut self, auto_play: bool) {
        self.auto_play = auto_play;
    }

    /// Get the current auto-play flag.
    pub fn get_auto_play(&self) -> bool {
        self.auto_play
    }

    // ── Settings ─────────────────────────────────────────────────────────

    /// Returns a reference to the current settings.
    pub fn get_settings(&self) -> &Settings {
        &self.settings
    }

    /// Replace the settings wholesale.
    pub fn update_settings(&mut self, settings: Settings) {
        self.settings = settings.clone();
        self.media_player_model = MediaPlayerModel::new(settings);
    }

    // ── Quality / ABR ────────────────────────────────────────────────────

    /// Get the current quality index for a media type.
    pub fn get_quality_for(&self, media_type: &str) -> usize {
        self.quality.get(media_type).copied().unwrap_or(0)
    }

    /// Set the quality index for a media type.
    pub fn set_quality_for(&mut self, media_type: &str, quality: usize) {
        self.quality.insert(media_type.to_owned(), quality);
        self.event_bus.trigger(Event::QualityChangeRequested, EventData::default());
    }

    /// Returns a list of `BitrateInfo` entries for the given media type.
    ///
    /// Until a manifest is parsed and tracks are wired, this returns an
    /// empty list.
    pub fn get_bitrate_info_list_for(&self, media_type: &str) -> Vec<BitrateInfo> {
        let tracks = self.media_controller.get_tracks_for_type(media_type);
        tracks
            .iter()
            .enumerate()
            .map(|(i, t)| BitrateInfo {
                media_type: media_type.to_owned(),
                bitrate: t.bitrate.unwrap_or(0),
                width: None,
                height: None,
                quality_index: i,
            })
            .collect()
    }

    // ── Track selection ──────────────────────────────────────────────────

    /// Get the currently active track for a media type.
    pub fn get_current_track_for(&self, media_type: &str) -> Option<&TrackInfo> {
        self.media_controller.get_active_track(media_type)
    }

    /// Switch the active track. The `track` must reference a valid track
    /// previously discovered from the manifest.
    pub fn set_current_track(&mut self, track: &TrackInfo) {
        self.media_controller
            .switch_track(&track.media_type, &track.id);
        self.event_bus.trigger(Event::CurrentTrackChanged, EventData::default());
    }

    /// Get all available tracks for a media type.
    pub fn get_tracks_for(&self, media_type: &str) -> &[TrackInfo] {
        self.media_controller.get_tracks_for_type(media_type)
    }

    // ── Event registration ───────────────────────────────────────────────

    /// Register an event handler. Returns a `HandlerId` that can be used
    /// with `off()`.
    pub fn on(
        &mut self,
        event: Event,
        callback: impl Fn(&EventData) + 'static,
    ) -> HandlerId {
        self.event_bus.on(event, callback, None)
    }

    /// Remove a previously registered handler.
    pub fn off(&mut self, event: &Event, id: HandlerId) {
        self.event_bus.off(event, id);
    }

    // ── Metrics / adapter accessors ──────────────────────────────────────

    /// Returns a reference to the `DashMetrics` collector.
    pub fn get_dash_metrics(&self) -> &DashMetrics {
        &self.dash_metrics
    }

    /// Returns a mutable reference to the `DashMetrics` collector.
    pub fn get_dash_metrics_mut(&mut self) -> &mut DashMetrics {
        &mut self.dash_metrics
    }

    // ── Live latency ─────────────────────────────────────────────────────

    /// Returns the current live latency in seconds.
    ///
    /// Requires `wall_clock_time` (milliseconds since epoch) — in a
    /// browser this would be `Date.now()`.
    pub fn get_current_live_latency(&self) -> f64 {
        // Without a real clock source we return 0.0; callers can use
        // `get_current_live_latency_with_clock` for a proper calculation.
        0.0
    }

    /// Calculate live latency given an explicit wall-clock time (ms).
    pub fn get_current_live_latency_with_clock(&self, wall_clock_ms: f64) -> f64 {
        self.playback_controller.get_current_live_latency(wall_clock_ms)
    }

    /// Returns the target live delay configured for low-latency playback.
    pub fn get_target_live_delay(&self) -> f64 {
        self.playback_controller.get_live_delay()
    }

    // ── Throughput ────────────────────────────────────────────────────────

    /// Returns the average measured throughput for the given media type.
    ///
    /// Currently returns the global throughput estimate; per-type tracking
    /// is planned.
    pub fn get_average_throughput(&self, _media_type: &str) -> f64 {
        self.throughput_controller.get_average_throughput()
    }

    // ── Version ──────────────────────────────────────────────────────────

    /// Returns the library version string.
    pub fn get_version(&self) -> &str {
        VERSION
    }

    // ── Player state ─────────────────────────────────────────────────────

    /// Returns the current `PlayerState`.
    pub fn get_state(&self) -> PlayerState {
        self.state
    }

    // ── Reset / destroy ──────────────────────────────────────────────────

    /// Reset the player to the `Created` state, releasing all resources.
    ///
    /// After `reset()` the player can be re-initialised with `initialize()`.
    pub fn reset(&mut self) {
        self.playback_controller.reset();
        self.stream_controller.reset();
        self.buffer_controller.reset();
        self.schedule_controller.reset();
        self.abr_controller.reset();
        self.gap_controller.reset();
        self.throughput_controller.reset();
        self.catchup_controller.reset();
        self.media_source_controller.reset();
        self.media_controller.reset();
        self.event_controller.reset();
        self.base_url_controller.reset();
        self.time_sync_controller.reset();
        self.fragment_controller.reset();
        self.protection_controller.reset();
        self.text_controller.reset();
        self.thumbnail_controller.reset();
        self.metrics_model.reset();
        self.fragment_model.reset();
        self.throughput_model.reset();
        self.dash_metrics.clear_all();
        self.event_bus.reset();
        self.quality.clear();

        self.source = None;
        self.auto_play = false;
        self.volume = 1.0;
        self.muted = false;
        self.state = PlayerState::Created;

        self.settings = Settings::default();
        self.media_player_model = MediaPlayerModel::new(self.settings.clone());

        self.logger.info("Player reset");
    }

    /// Destroy the player. Equivalent to `reset()` — provided for API
    /// compatibility with dash.js `MediaPlayer.destroy()`.
    pub fn destroy(&mut self) {
        self.reset();
    }

    // ── Internal controller accessors (for integration) ──────────────────

    /// Returns a reference to the internal `PlaybackController`.
    pub fn playback_controller(&self) -> &PlaybackController {
        &self.playback_controller
    }

    /// Returns a mutable reference to the internal `PlaybackController`.
    pub fn playback_controller_mut(&mut self) -> &mut PlaybackController {
        &mut self.playback_controller
    }

    /// Returns a reference to the internal `StreamController`.
    pub fn stream_controller(&self) -> &StreamController {
        &self.stream_controller
    }

    /// Returns a mutable reference to the internal `StreamController`.
    pub fn stream_controller_mut(&mut self) -> &mut StreamController {
        &mut self.stream_controller
    }

    /// Returns a reference to the internal `BufferController`.
    pub fn buffer_controller(&self) -> &BufferController {
        &self.buffer_controller
    }

    /// Returns a mutable reference to the internal `BufferController`.
    pub fn buffer_controller_mut(&mut self) -> &mut BufferController {
        &mut self.buffer_controller
    }

    /// Returns a reference to the internal `MediaController`.
    pub fn media_controller(&self) -> &MediaController {
        &self.media_controller
    }

    /// Returns a mutable reference to the internal `MediaController`.
    pub fn media_controller_mut(&mut self) -> &mut MediaController {
        &mut self.media_controller
    }

    /// Returns a reference to the internal `ThroughputController`.
    pub fn throughput_controller(&self) -> &ThroughputController {
        &self.throughput_controller
    }

    /// Returns a mutable reference to the internal `ThroughputController`.
    pub fn throughput_controller_mut(&mut self) -> &mut ThroughputController {
        &mut self.throughput_controller
    }

    /// Returns a reference to the internal `EventBus`.
    pub fn event_bus(&self) -> &EventBus {
        &self.event_bus
    }

    /// Returns a mutable reference to the internal `EventBus`.
    pub fn event_bus_mut(&mut self) -> &mut EventBus {
        &mut self.event_bus
    }
}

// ─── Tests ───────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use std::cell::Cell;
    use std::rc::Rc;

    // -- factory / lifecycle ------------------------------------------------

    #[test]
    fn create_returns_created_state() {
        let player = MediaPlayer::create();
        assert_eq!(player.get_state(), PlayerState::Created);
    }

    #[test]
    fn new_returns_created_state() {
        let player = MediaPlayer::new();
        assert_eq!(player.get_state(), PlayerState::Created);
        assert!(!player.is_ready());
    }

    #[test]
    fn initialize_transitions_to_initialized() {
        let mut player = MediaPlayer::new();
        player.initialize(Some("https://example.com/manifest.mpd"), false);
        assert_eq!(player.get_state(), PlayerState::Initialized);
        assert!(player.is_ready());
    }

    #[test]
    fn initialize_with_auto_play_transitions_to_playing() {
        let mut player = MediaPlayer::new();
        player.initialize(Some("https://example.com/manifest.mpd"), true);
        assert_eq!(player.get_state(), PlayerState::Playing);
    }

    #[test]
    fn double_initialize_is_ignored() {
        let mut player = MediaPlayer::new();
        player.initialize(Some("https://example.com/a.mpd"), false);
        player.initialize(Some("https://example.com/b.mpd"), false);
        // Source should still be the first one
        assert_eq!(player.get_source(), Some("https://example.com/a.mpd"));
    }

    #[test]
    fn reset_returns_to_created() {
        let mut player = MediaPlayer::new();
        player.initialize(Some("https://example.com/manifest.mpd"), true);
        player.reset();
        assert_eq!(player.get_state(), PlayerState::Created);
        assert!(player.get_source().is_none());
        assert!(!player.is_ready());
    }

    #[test]
    fn destroy_is_equivalent_to_reset() {
        let mut player = MediaPlayer::new();
        player.initialize(Some("https://example.com/manifest.mpd"), false);
        player.destroy();
        assert_eq!(player.get_state(), PlayerState::Created);
    }

    // -- play / pause -------------------------------------------------------

    #[test]
    fn play_transitions_to_playing() {
        let mut player = MediaPlayer::new();
        player.initialize(None, false);
        player.play();
        assert_eq!(player.get_state(), PlayerState::Playing);
        assert!(!player.is_paused());
    }

    #[test]
    fn pause_transitions_to_paused() {
        let mut player = MediaPlayer::new();
        player.initialize(None, false);
        player.play();
        player.pause();
        assert_eq!(player.get_state(), PlayerState::Paused);
        assert!(player.is_paused());
    }

    #[test]
    fn play_before_initialize_is_noop() {
        let mut player = MediaPlayer::new();
        player.play();
        assert_eq!(player.get_state(), PlayerState::Created);
    }

    #[test]
    fn pause_before_initialize_is_noop() {
        let mut player = MediaPlayer::new();
        player.pause();
        assert_eq!(player.get_state(), PlayerState::Created);
    }

    // -- seek ---------------------------------------------------------------

    #[test]
    fn seek_updates_time() {
        let mut player = MediaPlayer::new();
        player.initialize(None, false);
        player.seek(42.0);
        assert!((player.time() - 42.0).abs() < f64::EPSILON);
        assert!(player.is_seeking());
    }

    #[test]
    fn seek_before_initialize_is_noop() {
        let mut player = MediaPlayer::new();
        player.seek(10.0);
        assert!((player.time() - 0.0).abs() < f64::EPSILON);
    }

    // -- volume / mute ------------------------------------------------------

    #[test]
    fn default_volume_is_one() {
        let player = MediaPlayer::new();
        assert!((player.get_volume() - 1.0).abs() < f64::EPSILON);
    }

    #[test]
    fn set_volume_clamps() {
        let mut player = MediaPlayer::new();
        player.set_volume(1.5);
        assert!((player.get_volume() - 1.0).abs() < f64::EPSILON);
        player.set_volume(-0.5);
        assert!((player.get_volume() - 0.0).abs() < f64::EPSILON);
    }

    #[test]
    fn mute_and_unmute() {
        let mut player = MediaPlayer::new();
        assert!(!player.is_muted());
        player.set_mute(true);
        assert!(player.is_muted());
        player.set_mute(false);
        assert!(!player.is_muted());
    }

    // -- settings -----------------------------------------------------------

    #[test]
    fn get_default_settings() {
        let player = MediaPlayer::new();
        let s = player.get_settings();
        assert_eq!(s.debug.log_level, 3);
    }

    #[test]
    fn update_settings_applies() {
        let mut player = MediaPlayer::new();
        let mut settings = Settings::default();
        settings.debug.log_level = 5;
        player.update_settings(settings);
        assert_eq!(player.get_settings().debug.log_level, 5);
    }

    // -- quality ------------------------------------------------------------

    #[test]
    fn default_quality_is_zero() {
        let player = MediaPlayer::new();
        assert_eq!(player.get_quality_for("video"), 0);
    }

    #[test]
    fn set_and_get_quality() {
        let mut player = MediaPlayer::new();
        player.set_quality_for("video", 3);
        assert_eq!(player.get_quality_for("video"), 3);
        assert_eq!(player.get_quality_for("audio"), 0);
    }

    // -- event registration -------------------------------------------------

    #[test]
    fn on_registers_and_fires_callback() {
        let mut player = MediaPlayer::new();
        let called = Rc::new(Cell::new(false));
        let called_c = called.clone();
        player.on(Event::PlaybackStarted, move |_| called_c.set(true));
        player.initialize(None, false);
        player.play();
        assert!(called.get());
    }

    #[test]
    fn off_removes_callback() {
        let mut player = MediaPlayer::new();
        let called = Rc::new(Cell::new(false));
        let called_c = called.clone();
        let id = player.on(Event::PlaybackStarted, move |_| called_c.set(true));
        player.off(&Event::PlaybackStarted, id);
        player.initialize(None, false);
        player.play();
        assert!(!called.get());
    }

    // -- source attach ------------------------------------------------------

    #[test]
    fn attach_source_sets_url() {
        let mut player = MediaPlayer::new();
        player.initialize(None, false);
        player.attach_source("https://example.com/manifest.mpd");
        assert_eq!(player.get_source(), Some("https://example.com/manifest.mpd"));
        assert!(player.is_ready());
    }

    #[test]
    fn source_is_none_before_attach() {
        let player = MediaPlayer::new();
        assert!(player.get_source().is_none());
    }

    // -- is_ready / is_dynamic / version ------------------------------------

    #[test]
    fn is_ready_requires_init_and_source() {
        let mut player = MediaPlayer::new();
        assert!(!player.is_ready());
        player.initialize(None, false);
        assert!(!player.is_ready()); // no source yet
        player.attach_source("https://example.com/manifest.mpd");
        assert!(player.is_ready());
    }

    #[test]
    fn is_dynamic_defaults_to_false() {
        let player = MediaPlayer::new();
        assert!(!player.is_dynamic());
    }

    #[test]
    fn get_version_returns_cargo_version() {
        let player = MediaPlayer::new();
        assert!(!player.get_version().is_empty());
        assert_eq!(player.get_version(), env!("CARGO_PKG_VERSION"));
    }

    // -- playback rate ------------------------------------------------------

    #[test]
    fn playback_rate_default_is_one() {
        let player = MediaPlayer::new();
        assert!((player.get_playback_rate() - 1.0).abs() < f64::EPSILON);
    }

    #[test]
    fn set_playback_rate() {
        let mut player = MediaPlayer::new();
        player.set_playback_rate(2.0);
        assert!((player.get_playback_rate() - 2.0).abs() < f64::EPSILON);
    }

    // -- auto-play ----------------------------------------------------------

    #[test]
    fn auto_play_defaults_to_false() {
        let player = MediaPlayer::new();
        assert!(!player.get_auto_play());
    }

    #[test]
    fn set_auto_play() {
        let mut player = MediaPlayer::new();
        player.set_auto_play(true);
        assert!(player.get_auto_play());
    }

    // -- live latency / throughput -------------------------------------------

    #[test]
    fn live_latency_defaults_to_zero() {
        let player = MediaPlayer::new();
        assert!((player.get_current_live_latency() - 0.0).abs() < f64::EPSILON);
    }

    #[test]
    fn target_live_delay_defaults_to_zero() {
        let player = MediaPlayer::new();
        assert!((player.get_target_live_delay() - 0.0).abs() < f64::EPSILON);
    }

    #[test]
    fn average_throughput_defaults_to_zero() {
        let player = MediaPlayer::new();
        assert!((player.get_average_throughput("video") - 0.0).abs() < f64::EPSILON);
    }

    // -- duration / buffer length -------------------------------------------

    #[test]
    fn duration_defaults_to_zero() {
        let player = MediaPlayer::new();
        assert!((player.duration() - 0.0).abs() < f64::EPSILON);
    }

    #[test]
    fn buffer_length_defaults_to_zero() {
        let player = MediaPlayer::new();
        assert!((player.get_buffer_length("video") - 0.0).abs() < f64::EPSILON);
    }

    // -- tracks / bitrate info list -----------------------------------------

    #[test]
    fn get_tracks_empty_by_default() {
        let player = MediaPlayer::new();
        assert!(player.get_tracks_for("video").is_empty());
    }

    #[test]
    fn get_bitrate_info_list_empty_by_default() {
        let player = MediaPlayer::new();
        assert!(player.get_bitrate_info_list_for("video").is_empty());
    }

    #[test]
    fn current_track_is_none_by_default() {
        let player = MediaPlayer::new();
        assert!(player.get_current_track_for("video").is_none());
    }

    // -- reset clears quality map -------------------------------------------

    #[test]
    fn reset_clears_quality() {
        let mut player = MediaPlayer::new();
        player.set_quality_for("video", 5);
        player.reset();
        assert_eq!(player.get_quality_for("video"), 0);
    }

    // -- full lifecycle: create → init → play → pause → seek → reset --------

    #[test]
    fn full_lifecycle() {
        let mut player = MediaPlayer::create();
        assert_eq!(player.get_state(), PlayerState::Created);

        player.initialize(Some("https://example.com/stream.mpd"), false);
        assert_eq!(player.get_state(), PlayerState::Initialized);

        player.play();
        assert_eq!(player.get_state(), PlayerState::Playing);

        player.pause();
        assert_eq!(player.get_state(), PlayerState::Paused);

        player.seek(30.0);
        assert!((player.time() - 30.0).abs() < f64::EPSILON);

        player.play();
        assert_eq!(player.get_state(), PlayerState::Playing);

        player.reset();
        assert_eq!(player.get_state(), PlayerState::Created);
        assert!(player.get_source().is_none());
    }
}
