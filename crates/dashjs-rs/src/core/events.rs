// Port of dash.js/src/core/events/Events.js + CoreEvents.js + MediaPlayerEvents.js
//
// All event types used throughout the player, combining the public MediaPlayer
// events and internal Core events into a single strongly-typed enum.

/// Every event that can flow through the [`EventBus`](super::event_bus::EventBus).
///
/// Variants prefixed with nothing are public (MediaPlayer-level) events.
/// The enum merges `MediaPlayerEvents`, `CoreEvents`, and `EventsBase` from
/// the original dash.js source.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub enum Event {
    // ── Public MediaPlayer events (from MediaPlayerEvents.js) ────────────

    /// Streaming.vo.metrics.ManifestUpdate(...).catch(...) available
    AstInFuture,
    /// Base URLs have been updated
    BaseUrlsUpdated,
    /// Buffer has run out of data (stalled)
    BufferEmpty,
    /// Buffer has loaded enough data to resume
    BufferLoaded,
    /// Buffer level state (e.g. empty ↔ loaded) changed
    BufferLevelStateChanged,
    /// Buffer level was recalculated
    BufferLevelUpdated,
    /// A DVB font download was added
    DvbFontDownloadAdded,
    /// A DVB font download completed
    DvbFontDownloadComplete,
    /// A DVB font download failed
    DvbFontDownloadFailed,
    /// Stream switched from dynamic (live) to static (VOD)
    DynamicToStatic,
    /// A fatal or non-fatal error occurred
    Error,
    /// A media fragment has finished loading
    FragmentLoadingCompleted,
    /// Progress on a media fragment download
    FragmentLoadingProgress,
    /// A media fragment started loading
    FragmentLoadingStarted,
    /// A media fragment load was abandoned (ABR down-switch)
    FragmentLoadingAbandoned,
    /// Log message dispatched (when `debug.dispatch_event` is true)
    Log,
    /// Manifest loading has started
    ManifestLoadingStarted,
    /// Manifest loading has finished (success or failure)
    ManifestLoadingFinished,
    /// Manifest has been parsed and is available
    ManifestLoaded,
    /// Global metrics changed
    MetricsChanged,
    /// A specific metric changed
    MetricChanged,
    /// A new metric was added
    MetricAdded,
    /// A metric was updated
    MetricUpdated,
    /// Period switch has started
    PeriodSwitchStarted,
    /// Period switch has completed
    PeriodSwitchCompleted,
    /// ABR requested a quality change
    QualityChangeRequested,
    /// Quality change has been rendered on screen
    QualityChangeRendered,
    /// A new track was selected
    NewTrackSelected,
    /// Track change has been rendered on screen
    TrackChangeRendered,
    /// Stream is initializing
    StreamInitializing,
    /// Stream metadata was updated
    StreamUpdated,
    /// Stream was activated
    StreamActivated,
    /// Stream was deactivated
    StreamDeactivated,
    /// Stream has been fully initialized
    StreamInitialized,
    /// Stream teardown is complete
    StreamTeardownComplete,
    /// All text tracks have been added
    TextTracksAdded,
    /// A single text track was added
    TextTrackAdded,
    /// A subtitle cue entered the active region
    CueEnter,
    /// A subtitle cue exited the active region
    CueExit,
    /// A new throughput measurement was stored
    ThroughputMeasurementStored,
    /// TTML subtitle data has been parsed
    TtmlParsed,
    /// TTML subtitle data is ready to be parsed
    TtmlToParse,
    /// A caption was rendered
    CaptionRendered,
    /// Caption container was resized
    CaptionContainerResize,
    /// HTMLMediaElement `canplay` event
    CanPlay,
    /// HTMLMediaElement `canplaythrough` event
    CanPlayThrough,
    /// Playback has ended (reached the end of the stream)
    PlaybackEnded,
    /// HTMLMediaElement error during playback
    PlaybackError,
    /// Playback controller has been initialized
    PlaybackInitialized,
    /// Playback was blocked by autoplay policy
    PlaybackNotAllowed,
    /// HTMLMediaElement `loadedmetadata` event
    PlaybackMetadataLoaded,
    /// HTMLMediaElement `loadeddata` event
    PlaybackLoadedData,
    /// Playback was paused
    PlaybackPaused,
    /// Playback is now playing
    PlaybackPlaying,
    /// Download progress on the HTMLMediaElement
    PlaybackProgress,
    /// Playback rate changed
    PlaybackRateChanged,
    /// A seek operation completed
    PlaybackSeeked,
    /// A seek operation started
    PlaybackSeeking,
    /// Playback stalled (rebuffering)
    PlaybackStalled,
    /// Playback started for the first time
    PlaybackStarted,
    /// Playback time was updated (fires frequently)
    PlaybackTimeUpdated,
    /// Playback volume was changed
    PlaybackVolumeChanged,
    /// Playback is waiting for data
    PlaybackWaiting,
    /// Manifest validity window changed
    ManifestValidityChanged,
    /// Event dispatch mode — on start
    EventModeOnStart,
    /// Event dispatch mode — on receive
    EventModeOnReceive,
    /// A DASH-IF conformance violation was detected
    ConformanceViolation,
    /// Representation (quality level) was switched
    RepresentationSwitch,
    /// An AdaptationSet was removed because the device lacks capabilities
    AdaptationSetRemovedNoCapabilities,
    /// Content steering request completed
    ContentSteeringRequestCompleted,
    /// In-band ProducerReferenceTime box received
    InbandPrft,
    /// ManagedMediaSource start streaming signal
    ManagedMediaSourceStartStreaming,
    /// ManagedMediaSource end streaming signal
    ManagedMediaSourceEndStreaming,

    // ── Internal Core events (from CoreEvents.js) ────────────────────────

    /// Background sync attempt requested
    AttemptBackgroundSync,
    /// Buffering completed for a representation
    BufferingCompleted,
    /// Buffer was cleared (pruned)
    BufferCleared,
    /// Buffer replacement started (fast-switch)
    BufferReplacementStarted,
    /// All bytes of the last fragment have been appended
    BytesAppendedEndFragment,
    /// Existence check for a resource completed
    CheckForExistenceCompleted,
    /// CMSD static header received
    CmsdStaticHeader,
    /// Current track changed internally
    CurrentTrackChanged,
    /// Internal data update completed after manifest reload
    DataUpdateCompleted,
    /// In-band events were parsed
    InbandEvents,
    /// Initial stream switch happened
    InitialStreamSwitch,
    /// An init (initialization) fragment has been loaded
    InitFragmentLoaded,
    /// An init fragment is needed by the buffer controller
    InitFragmentNeeded,
    /// Internal manifest loaded (before public event)
    InternalManifestLoaded,
    /// A segment load was abandoned
    LoadingAbandoned,
    /// A segment load completed
    LoadingCompleted,
    /// Segment loading data progress
    LoadingDataProgress,
    /// Segment loading progress
    LoadingProgress,
    /// Manifest was updated (internal notification)
    ManifestUpdated,
    /// MediaInfo was updated
    MediaInfoUpdated,
    /// A media fragment has been loaded
    MediaFragmentLoaded,
    /// A media fragment is needed by the buffer controller
    MediaFragmentNeeded,
    /// The original (unprocessed) manifest was loaded
    OriginalManifestLoaded,
    /// Source buffer quota was exceeded
    QuotaExceeded,
    /// Seek target was set
    SeekTarget,
    /// Segment location added to blacklist
    SegmentLocationBlacklistAdd,
    /// Segment location blacklist changed
    SegmentLocationBlacklistChanged,
    /// Service location (BaseURL) added to blacklist
    ServiceLocationBaseUrlBlacklistAdd,
    /// Service location (BaseURL) blacklist changed
    ServiceLocationBaseUrlBlacklistChanged,
    /// Service location (Location element) added to blacklist
    ServiceLocationLocationBlacklistAdd,
    /// Service location (Location element) blacklist changed
    ServiceLocationLocationBlacklistChanged,
    /// ABR active rules setting updated
    SettingUpdatedAbrActiveRules,
    /// Catchup enabled setting updated
    SettingUpdatedCatchupEnabled,
    /// Live delay setting updated
    SettingUpdatedLiveDelay,
    /// Live delay fragment count setting updated
    SettingUpdatedLiveDelayFragmentCount,
    /// Max bitrate setting updated
    SettingUpdatedMaxBitrate,
    /// Min bitrate setting updated
    SettingUpdatedMinBitrate,
    /// Playback rate max setting updated
    SettingUpdatedPlaybackRateMax,
    /// Playback rate min setting updated
    SettingUpdatedPlaybackRateMin,
    /// Fragmented text set after being disabled
    SetFragmentedTextAfterDisabled,
    /// Non-fragmented text set
    SetNonFragmentedText,
    /// An error on the MSE SourceBuffer
    SourceBufferError,
    /// Streams have been composed
    StreamsComposed,
    /// Buffering completed for the entire stream
    StreamBufferingCompleted,
    /// Stream requesting completed
    StreamRequestingCompleted,
    /// Text tracks queue initialized
    TextTracksQueueInitialized,
    /// Time synchronization completed
    TimeSynchronizationCompleted,
    /// Time sync offset should be updated
    UpdateTimeSyncOffset,
    /// URL resolution failed
    UrlResolutionFailed,
    /// A video chunk was received (low-latency)
    VideoChunkReceived,
    /// Video element was resized
    VideoElementResized,
    /// Wallclock time was updated (internal timer tick)
    WallclockTimeUpdated,
    /// An XLink element was loaded
    XlinkElementLoaded,
    /// XLink resolution is ready
    XlinkReady,

    // ── Protection / DRM events ──────────────────────────────────────────

    /// A key system was selected
    KeySystemSelected,
    /// A key session was created
    KeySessionCreated,
    /// A key session was closed
    KeySessionClosed,
    /// Key statuses changed
    KeyStatusesChanged,
    /// License request completed
    LicenseRequestComplete,
    /// Protection subsystem was created
    ProtectionCreated,
    /// Protection subsystem was destroyed
    ProtectionDestroyed,
    /// Browser `needkey` / `encrypted` event
    NeedKey,

    // ── Catch-all for forward compatibility ──────────────────────────────

    /// An event type not yet modelled. Carries the raw string identifier.
    Other(String),
}

impl Event {
    /// Return the dash.js string identifier for this event.
    pub fn as_str(&self) -> &str {
        match self {
            // Public events
            Self::AstInFuture => "astInFuture",
            Self::BaseUrlsUpdated => "baseUrlsUpdated",
            Self::BufferEmpty => "bufferStalled",
            Self::BufferLoaded => "bufferLoaded",
            Self::BufferLevelStateChanged => "bufferStateChanged",
            Self::BufferLevelUpdated => "bufferLevelUpdated",
            Self::DvbFontDownloadAdded => "dvbFontDownloadAdded",
            Self::DvbFontDownloadComplete => "dvbFontDownloadComplete",
            Self::DvbFontDownloadFailed => "dvbFontDownloadFailed",
            Self::DynamicToStatic => "dynamicToStatic",
            Self::Error => "error",
            Self::FragmentLoadingCompleted => "fragmentLoadingCompleted",
            Self::FragmentLoadingProgress => "fragmentLoadingProgress",
            Self::FragmentLoadingStarted => "fragmentLoadingStarted",
            Self::FragmentLoadingAbandoned => "fragmentLoadingAbandoned",
            Self::Log => "log",
            Self::ManifestLoadingStarted => "manifestLoadingStarted",
            Self::ManifestLoadingFinished => "manifestLoadingFinished",
            Self::ManifestLoaded => "manifestLoaded",
            Self::MetricsChanged => "metricsChanged",
            Self::MetricChanged => "metricChanged",
            Self::MetricAdded => "metricAdded",
            Self::MetricUpdated => "metricUpdated",
            Self::PeriodSwitchStarted => "periodSwitchStarted",
            Self::PeriodSwitchCompleted => "periodSwitchCompleted",
            Self::QualityChangeRequested => "qualityChangeRequested",
            Self::QualityChangeRendered => "qualityChangeRendered",
            Self::NewTrackSelected => "newTrackSelected",
            Self::TrackChangeRendered => "trackChangeRendered",
            Self::StreamInitializing => "streamInitializing",
            Self::StreamUpdated => "streamUpdated",
            Self::StreamActivated => "streamActivated",
            Self::StreamDeactivated => "streamDeactivated",
            Self::StreamInitialized => "streamInitialized",
            Self::StreamTeardownComplete => "streamTeardownComplete",
            Self::TextTracksAdded => "allTextTracksAdded",
            Self::TextTrackAdded => "textTrackAdded",
            Self::CueEnter => "cueEnter",
            Self::CueExit => "cueExit",
            Self::ThroughputMeasurementStored => "throughputMeasurementStored",
            Self::TtmlParsed => "ttmlParsed",
            Self::TtmlToParse => "ttmlToParse",
            Self::CaptionRendered => "captionRendered",
            Self::CaptionContainerResize => "captionContainerResize",
            Self::CanPlay => "canPlay",
            Self::CanPlayThrough => "canPlayThrough",
            Self::PlaybackEnded => "playbackEnded",
            Self::PlaybackError => "playbackError",
            Self::PlaybackInitialized => "playbackInitialized",
            Self::PlaybackNotAllowed => "playbackNotAllowed",
            Self::PlaybackMetadataLoaded => "playbackMetaDataLoaded",
            Self::PlaybackLoadedData => "playbackLoadedData",
            Self::PlaybackPaused => "playbackPaused",
            Self::PlaybackPlaying => "playbackPlaying",
            Self::PlaybackProgress => "playbackProgress",
            Self::PlaybackRateChanged => "playbackRateChanged",
            Self::PlaybackSeeked => "playbackSeeked",
            Self::PlaybackSeeking => "playbackSeeking",
            Self::PlaybackStalled => "playbackStalled",
            Self::PlaybackStarted => "playbackStarted",
            Self::PlaybackTimeUpdated => "playbackTimeUpdated",
            Self::PlaybackVolumeChanged => "playbackVolumeChanged",
            Self::PlaybackWaiting => "playbackWaiting",
            Self::ManifestValidityChanged => "manifestValidityChanged",
            Self::EventModeOnStart => "eventModeOnStart",
            Self::EventModeOnReceive => "eventModeOnReceive",
            Self::ConformanceViolation => "conformanceViolation",
            Self::RepresentationSwitch => "representationSwitch",
            Self::AdaptationSetRemovedNoCapabilities => "adaptationSetRemovedNoCapabilities",
            Self::ContentSteeringRequestCompleted => "contentSteeringRequestCompleted",
            Self::InbandPrft => "inbandPrft",
            Self::ManagedMediaSourceStartStreaming => "managedMediaSourceStartStreaming",
            Self::ManagedMediaSourceEndStreaming => "managedMediaSourceEndStreaming",

            // Core events
            Self::AttemptBackgroundSync => "attemptBackgroundSync",
            Self::BufferingCompleted => "bufferingCompleted",
            Self::BufferCleared => "bufferCleared",
            Self::BufferReplacementStarted => "bufferReplacementStarted",
            Self::BytesAppendedEndFragment => "bytesAppendedEndFragment",
            Self::CheckForExistenceCompleted => "checkForExistenceCompleted",
            Self::CmsdStaticHeader => "cmsdStaticHeader",
            Self::CurrentTrackChanged => "currentTrackChanged",
            Self::DataUpdateCompleted => "dataUpdateCompleted",
            Self::InbandEvents => "inbandEvents",
            Self::InitialStreamSwitch => "initialStreamSwitch",
            Self::InitFragmentLoaded => "initFragmentLoaded",
            Self::InitFragmentNeeded => "initFragmentNeeded",
            Self::InternalManifestLoaded => "internalManifestLoaded",
            Self::LoadingAbandoned => "loadingAborted",
            Self::LoadingCompleted => "loadingCompleted",
            Self::LoadingDataProgress => "loadingDataProgress",
            Self::LoadingProgress => "loadingProgress",
            Self::ManifestUpdated => "manifestUpdated",
            Self::MediaInfoUpdated => "mediaInfoUpdated",
            Self::MediaFragmentLoaded => "mediaFragmentLoaded",
            Self::MediaFragmentNeeded => "mediaFragmentNeeded",
            Self::OriginalManifestLoaded => "originalManifestLoaded",
            Self::QuotaExceeded => "quotaExceeded",
            Self::SeekTarget => "seekTarget",
            Self::SegmentLocationBlacklistAdd => "segmentLocationBlacklistAdd",
            Self::SegmentLocationBlacklistChanged => "segmentLocationBlacklistChanged",
            Self::ServiceLocationBaseUrlBlacklistAdd => "serviceLocationBlacklistAdd",
            Self::ServiceLocationBaseUrlBlacklistChanged => "serviceLocationBlacklistChanged",
            Self::ServiceLocationLocationBlacklistAdd => "serviceLocationLocationBlacklistAdd",
            Self::ServiceLocationLocationBlacklistChanged => "serviceLocationLocationBlacklistChanged",
            Self::SettingUpdatedAbrActiveRules => "settingUpdatedAbrActiveRules",
            Self::SettingUpdatedCatchupEnabled => "settingUpdatedCatchupEnabled",
            Self::SettingUpdatedLiveDelay => "settingUpdatedLiveDelay",
            Self::SettingUpdatedLiveDelayFragmentCount => "settingUpdatedLiveDelayFragmentCount",
            Self::SettingUpdatedMaxBitrate => "settingUpdatedMaxBitrate",
            Self::SettingUpdatedMinBitrate => "settingUpdatedMinBitrate",
            Self::SettingUpdatedPlaybackRateMax => "settingUpdatedPlaybackRateMax",
            Self::SettingUpdatedPlaybackRateMin => "settingUpdatedPlaybackRateMin",
            Self::SetFragmentedTextAfterDisabled => "setFragmentedTextAfterDisabled",
            Self::SetNonFragmentedText => "setNonFragmentedText",
            Self::SourceBufferError => "sourceBufferError",
            Self::StreamsComposed => "streamsComposed",
            Self::StreamBufferingCompleted => "streamBufferingCompleted",
            Self::StreamRequestingCompleted => "streamRequestingCompleted",
            Self::TextTracksQueueInitialized => "textTracksQueueInitialized",
            Self::TimeSynchronizationCompleted => "timeSynchronizationComplete",
            Self::UpdateTimeSyncOffset => "updateTimeSyncOffset",
            Self::UrlResolutionFailed => "urlResolutionFailed",
            Self::VideoChunkReceived => "videoChunkReceived",
            Self::VideoElementResized => "videoElementResized",
            Self::WallclockTimeUpdated => "wallclockTimeUpdated",
            Self::XlinkElementLoaded => "xlinkElementLoaded",
            Self::XlinkReady => "xlinkReady",

            // Protection events
            Self::KeySystemSelected => "keySystemSelected",
            Self::KeySessionCreated => "keySessionCreated",
            Self::KeySessionClosed => "keySessionClosed",
            Self::KeyStatusesChanged => "keyStatusesChanged",
            Self::LicenseRequestComplete => "licenseRequestComplete",
            Self::ProtectionCreated => "protectionCreated",
            Self::ProtectionDestroyed => "protectionDestroyed",
            Self::NeedKey => "needKey",

            Self::Other(s) => s.as_str(),
        }
    }
}

impl std::fmt::Display for Event {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.as_str())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn event_string_round_trip() {
        assert_eq!(Event::BufferEmpty.as_str(), "bufferStalled");
        assert_eq!(Event::PlaybackStarted.as_str(), "playbackStarted");
        assert_eq!(Event::WallclockTimeUpdated.as_str(), "wallclockTimeUpdated");
    }

    #[test]
    fn event_equality() {
        assert_eq!(Event::Error, Event::Error);
        assert_ne!(Event::Error, Event::PlaybackError);
    }

    #[test]
    fn event_display() {
        assert_eq!(format!("{}", Event::ManifestLoaded), "manifestLoaded");
    }
}
