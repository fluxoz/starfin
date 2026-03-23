//! Port of `dash.js/src/streaming/MediaPlayerEvents.js`.
//!
//! All public-facing MediaPlayer events, ported as Rust string constants and as
//! re-exports of the strongly-typed [`Event`] enum.  Each constant has the same
//! name and wire-string value as its JavaScript counterpart so that consumers
//! can do either of:
//!
//! ```rust
//! use dashjs_rs::streaming::media_player_events::{BUFFER_EMPTY, Event};
//! assert_eq!(BUFFER_EMPTY, "bufferStalled");
//! let _ = Event::BufferEmpty;
//! ```
//!
//! The 73 public constants below correspond 1-to-1 with the properties set in
//! `dash.js/src/streaming/MediaPlayerEvents.js`.

// ── Re-export the strongly-typed enum so callers can pattern-match ────────────
pub use crate::core::events::Event;

// ── String constants — identical to dash.js MediaPlayerEvents property values ─

/// `astInFuture` — MPD availabilityStartTime is in the future; check `delay`.
pub const AST_IN_FUTURE: &str = "astInFuture";
/// `baseUrlsUpdated` — Base URLs were updated.
pub const BASE_URLS_UPDATED: &str = "baseUrlsUpdated";
/// `bufferStalled` — Buffer ran out of data (stalled).
pub const BUFFER_EMPTY: &str = "bufferStalled";
/// `bufferLoaded` — Buffer has loaded enough data to resume.
pub const BUFFER_LOADED: &str = "bufferLoaded";
/// `bufferStateChanged` — Buffer state (empty ↔ loaded) changed.
pub const BUFFER_LEVEL_STATE_CHANGED: &str = "bufferStateChanged";
/// `bufferLevelUpdated` — Buffer level was recalculated.
pub const BUFFER_LEVEL_UPDATED: &str = "bufferLevelUpdated";
/// `dvbFontDownloadAdded` — DVB font download was added to the FontFaceSet.
pub const DVB_FONT_DOWNLOAD_ADDED: &str = "dvbFontDownloadAdded";
/// `dvbFontDownloadComplete` — DVB font downloaded successfully.
pub const DVB_FONT_DOWNLOAD_COMPLETE: &str = "dvbFontDownloadComplete";
/// `dvbFontDownloadFailed` — DVB font download failed.
pub const DVB_FONT_DOWNLOAD_FAILED: &str = "dvbFontDownloadFailed";
/// `dynamicToStatic` — Stream switched from live (dynamic) to VOD (static).
pub const DYNAMIC_TO_STATIC: &str = "dynamicToStatic";
/// `error` — A fatal or non-fatal error occurred.
pub const ERROR: &str = "error";
/// `fragmentLoadingCompleted` — A media fragment finished loading.
pub const FRAGMENT_LOADING_COMPLETED: &str = "fragmentLoadingCompleted";
/// `fragmentLoadingProgress` — Progress on a media fragment download.
pub const FRAGMENT_LOADING_PROGRESS: &str = "fragmentLoadingProgress";
/// `fragmentLoadingStarted` — A media fragment started loading.
pub const FRAGMENT_LOADING_STARTED: &str = "fragmentLoadingStarted";
/// `fragmentLoadingAbandoned` — A media fragment load was abandoned (ABR down-switch).
pub const FRAGMENT_LOADING_ABANDONED: &str = "fragmentLoadingAbandoned";
/// `log` — Log message dispatched (when `debug.dispatchEvent` is true).
pub const LOG: &str = "log";
/// `manifestLoadingStarted` — Manifest loading has started.
pub const MANIFEST_LOADING_STARTED: &str = "manifestLoadingStarted";
/// `manifestLoadingFinished` — Manifest loading has finished (success or failure).
pub const MANIFEST_LOADING_FINISHED: &str = "manifestLoadingFinished";
/// `manifestLoaded` — Manifest has been parsed and is available.
pub const MANIFEST_LOADED: &str = "manifestLoaded";
/// `manifestValidityChanged` — Manifest validity window changed.
pub const MANIFEST_VALIDITY_CHANGED: &str = "manifestValidityChanged";
/// `metricsChanged` — Global metrics changed.
pub const METRICS_CHANGED: &str = "metricsChanged";
/// `metricChanged` — A specific metric changed.
pub const METRIC_CHANGED: &str = "metricChanged";
/// `metricAdded` — A new metric was added.
pub const METRIC_ADDED: &str = "metricAdded";
/// `metricUpdated` — A metric was updated.
pub const METRIC_UPDATED: &str = "metricUpdated";
/// `periodSwitchStarted` — Period switch has started.
pub const PERIOD_SWITCH_STARTED: &str = "periodSwitchStarted";
/// `periodSwitchCompleted` — Period switch has completed.
pub const PERIOD_SWITCH_COMPLETED: &str = "periodSwitchCompleted";
/// `qualityChangeRequested` — ABR requested a quality change.
pub const QUALITY_CHANGE_REQUESTED: &str = "qualityChangeRequested";
/// `qualityChangeRendered` — Quality change has been rendered on screen.
pub const QUALITY_CHANGE_RENDERED: &str = "qualityChangeRendered";
/// `newTrackSelected` — A new track was selected.
pub const NEW_TRACK_SELECTED: &str = "newTrackSelected";
/// `trackChangeRendered` — Track change has been rendered.
pub const TRACK_CHANGE_RENDERED: &str = "trackChangeRendered";
/// `streamInitializing` — Stream is initializing.
pub const STREAM_INITIALIZING: &str = "streamInitializing";
/// `streamUpdated` — Stream metadata was updated.
pub const STREAM_UPDATED: &str = "streamUpdated";
/// `streamActivated` — Stream was activated.
pub const STREAM_ACTIVATED: &str = "streamActivated";
/// `streamDeactivated` — Stream was deactivated.
pub const STREAM_DEACTIVATED: &str = "streamDeactivated";
/// `streamInitialized` — Stream has been fully initialized.
pub const STREAM_INITIALIZED: &str = "streamInitialized";
/// `streamTeardownComplete` — Stream teardown is complete.
pub const STREAM_TEARDOWN_COMPLETE: &str = "streamTeardownComplete";
/// `textTracksAdded` — All text tracks have been added.
pub const TEXT_TRACKS_ADDED: &str = "textTracksAdded";
/// `textTrackAdded` — A single text track was added.
pub const TEXT_TRACK_ADDED: &str = "textTrackAdded";
/// `cueEnter` — A subtitle cue entered the active region.
pub const CUE_ENTER: &str = "cueEnter";
/// `cueExit` — A subtitle cue exited the active region.
pub const CUE_EXIT: &str = "cueExit";
/// `throughputMeasurementStored` — A new throughput measurement was stored.
pub const THROUGHPUT_MEASUREMENT_STORED: &str = "throughputMeasurementStored";
/// `ttmlParsed` — TTML subtitle data has been parsed.
pub const TTML_PARSED: &str = "ttmlParsed";
/// `ttmlToParse` — TTML subtitle data is ready to be parsed.
pub const TTML_TO_PARSE: &str = "ttmlToParse";
/// `captionRendered` — A caption was rendered.
pub const CAPTION_RENDERED: &str = "captionRendered";
/// `captionContainerResize` — Caption container was resized.
pub const CAPTION_CONTAINER_RESIZE: &str = "captionContainerResize";
/// `canPlay` — HTMLMediaElement `canplay` event.
pub const CAN_PLAY: &str = "canPlay";
/// `canPlayThrough` — HTMLMediaElement `canplaythrough` event.
pub const CAN_PLAY_THROUGH: &str = "canPlayThrough";
/// `playbackEnded` — Playback reached the end of the stream.
pub const PLAYBACK_ENDED: &str = "playbackEnded";
/// `playbackError` — HTMLMediaElement error during playback.
pub const PLAYBACK_ERROR: &str = "playbackError";
/// `playbackInitialized` — Playback controller has been initialized.
pub const PLAYBACK_INITIALIZED: &str = "playbackInitialized";
/// `playbackNotAllowed` — Playback was blocked by the autoplay policy.
pub const PLAYBACK_NOT_ALLOWED: &str = "playbackNotAllowed";
/// `playbackMetaDataLoaded` — HTMLMediaElement `loadedmetadata` event.
pub const PLAYBACK_METADATA_LOADED: &str = "playbackMetaDataLoaded";
/// `playbackLoadedData` — HTMLMediaElement `loadeddata` event.
pub const PLAYBACK_LOADED_DATA: &str = "playbackLoadedData";
/// `playbackPaused` — Playback was paused.
pub const PLAYBACK_PAUSED: &str = "playbackPaused";
/// `playbackPlaying` — Playback is now playing.
pub const PLAYBACK_PLAYING: &str = "playbackPlaying";
/// `playbackProgress` — Download progress on the HTMLMediaElement.
pub const PLAYBACK_PROGRESS: &str = "playbackProgress";
/// `playbackRateChanged` — Playback rate changed.
pub const PLAYBACK_RATE_CHANGED: &str = "playbackRateChanged";
/// `playbackSeeked` — A seek operation completed.
pub const PLAYBACK_SEEKED: &str = "playbackSeeked";
/// `playbackSeeking` — A seek operation started.
pub const PLAYBACK_SEEKING: &str = "playbackSeeking";
/// `playbackStalled` — Playback stalled (rebuffering).
pub const PLAYBACK_STALLED: &str = "playbackStalled";
/// `playbackStarted` — Playback started for the first time.
pub const PLAYBACK_STARTED: &str = "playbackStarted";
/// `playbackTimeUpdated` — Playback time was updated.
pub const PLAYBACK_TIME_UPDATED: &str = "playbackTimeUpdated";
/// `playbackVolumeChanged` — Playback volume was changed.
pub const PLAYBACK_VOLUME_CHANGED: &str = "playbackVolumeChanged";
/// `playbackWaiting` — Playback is waiting for data.
pub const PLAYBACK_WAITING: &str = "playbackWaiting";
/// `representationSwitch` — Representation (quality level) was switched.
pub const REPRESENTATION_SWITCH: &str = "representationSwitch";
/// `adaptationSetRemovedNoCapabilities` — AdaptationSet removed (device lacks capabilities).
pub const ADAPTATION_SET_REMOVED_NO_CAPABILITIES: &str = "adaptationSetRemovedNoCapabilities";
/// `contentSteeringRequestCompleted` — Content steering request completed.
pub const CONTENT_STEERING_REQUEST_COMPLETED: &str = "contentSteeringRequestCompleted";
/// `inbandPrft` — In-band ProducerReferenceTime box received.
pub const INBAND_PRFT: &str = "inbandPrft";
/// `managedMediaSourceStartStreaming` — ManagedMediaSource start streaming signal.
pub const MANAGED_MEDIA_SOURCE_START_STREAMING: &str = "managedMediaSourceStartStreaming";
/// `managedMediaSourceEndStreaming` — ManagedMediaSource end streaming signal.
pub const MANAGED_MEDIA_SOURCE_END_STREAMING: &str = "managedMediaSourceEndStreaming";
/// `conformanceViolation` — A DASH-IF conformance violation was detected.
pub const CONFORMANCE_VIOLATION: &str = "conformanceViolation";
/// `eventModeOnStart` — Event dispatch mode: on start.
pub const EVENT_MODE_ON_START: &str = "eventModeOnStart";
/// `eventModeOnReceive` — Event dispatch mode: on receive.
pub const EVENT_MODE_ON_RECEIVE: &str = "eventModeOnReceive";
