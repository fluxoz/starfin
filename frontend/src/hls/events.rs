//! HLS events system

use crate::hls::playlist::{Level, Segment, MediaPlaylist};
use crate::hls::error::HlsError;

/// HLS events that can be emitted during playback
#[derive(Clone, Debug)]
pub enum HlsEvent {
    // Manifest events
    /// Manifest loaded successfully
    ManifestLoaded {
        levels: Vec<Level>,
        audio_tracks: Vec<AudioTrack>,
        subtitle_tracks: Vec<SubtitleTrack>,
    },
    /// Manifest parsing error
    ManifestParsingError { error: HlsError },
    
    // Level events
    /// Level loading started
    LevelLoading { level: usize },
    /// Level loaded successfully
    LevelLoaded { level: usize, playlist: MediaPlaylist },
    /// Level switching started
    LevelSwitching { level: usize },
    /// Level switched successfully
    LevelSwitched { level: usize },
    /// Level update (for live streams)
    LevelUpdated { level: usize, playlist: MediaPlaylist },
    
    // Fragment events
    /// Fragment loading started
    FragLoading { level: usize, frag: FragmentInfo },
    /// Fragment loaded successfully
    FragLoaded { level: usize, frag: FragmentInfo, stats: FragmentStats },
    /// Fragment changed (new fragment playing)
    FragChanged { frag: FragmentInfo },
    /// Fragment parsing error
    FragParsingError { frag: FragmentInfo, error: HlsError },
    
    // Buffer events
    /// Buffer appended
    BufferAppended { segment_type: SegmentType, time_range: (f64, f64) },
    /// Buffer flushed
    BufferFlushed { segment_type: SegmentType },
    /// Buffer stalled (playback waiting for data)
    BufferStalled,
    /// Buffer recovered from stall
    BufferRecovered,
    /// Buffer end of stream
    BufferEos,
    
    // Media events
    /// Media attached to video element
    MediaAttached,
    /// Media detached from video element
    MediaDetached,
    /// Media ended
    MediaEnded,
    /// Seeking started
    Seeking { target: f64 },
    /// Seeking completed
    Seeked { target: f64 },
    
    // ABR events
    /// ABR bandwidth estimate updated
    BandwidthEstimate { bandwidth: u64, estimate_time: f64 },
    /// Quality level auto-switched by ABR
    AbrLevelSwitch { from: usize, to: usize, reason: AbrSwitchReason },
    
    // Timed metadata events
    /// ID3 metadata received
    Id3Metadata { samples: Vec<Id3Sample> },
    /// EMSG metadata received  
    EmsgMetadata { event: EmsgEvent },
    /// DATERANGE tag parsed
    DateRangeMetadata { date_range: DateRange },
    
    // Subtitle/caption events
    /// Subtitle track switch
    SubtitleTrackSwitch { track: Option<usize> },
    /// Subtitle track loaded
    SubtitleTrackLoaded { track: usize },
    /// Subtitle cue parsed
    SubtitleCue { cue: SubtitleCue },
    
    // Error events
    /// Error occurred
    Error { error: HlsError },
    /// Recovery attempt started
    RecoveryAttempt { action: String },
    /// Recovery succeeded
    RecoverySuccess,
    
    // Lifecycle events
    /// HLS controller destroyed
    Destroying,
}

/// Audio track information
#[derive(Clone, Debug)]
pub struct AudioTrack {
    pub id: usize,
    pub name: String,
    pub language: Option<String>,
    pub default: bool,
    pub autoselect: bool,
    pub group_id: String,
    pub channels: Option<String>,
}

/// Subtitle track information
#[derive(Clone, Debug)]
pub struct SubtitleTrack {
    pub id: usize,
    pub name: String,
    pub language: Option<String>,
    pub default: bool,
    pub autoselect: bool,
    pub forced: bool,
    pub group_id: String,
}

/// Fragment/segment information
#[derive(Clone, Debug)]
pub struct FragmentInfo {
    pub sn: u64,
    pub level: usize,
    pub start: f64,
    pub duration: f64,
    pub url: String,
    pub byte_range: Option<(u64, u64)>,
}

impl From<&Segment> for FragmentInfo {
    fn from(seg: &Segment) -> Self {
        Self {
            sn: seg.sequence_number,
            level: 0, // Set by caller
            start: seg.start_time,
            duration: seg.duration,
            url: seg.uri.clone(),
            byte_range: seg.byte_range,
        }
    }
}

/// Fragment load statistics
#[derive(Clone, Debug)]
pub struct FragmentStats {
    /// Total load time in milliseconds
    pub load_time: f64,
    /// Size of loaded data in bytes
    pub loaded_bytes: u64,
    /// Effective bandwidth (bytes per second)
    pub bandwidth: f64,
    /// Retry count
    pub retry_count: u32,
}

/// Segment type (audio/video/muxed)
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SegmentType {
    Audio,
    Video,
    Combined,
}

/// ABR switch reason
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum AbrSwitchReason {
    /// Bandwidth estimate increased
    BandwidthIncrease,
    /// Bandwidth estimate decreased
    BandwidthDecrease,
    /// Emergency switch due to buffer starvation
    EmergencySwitch,
    /// Buffer level exceeded threshold
    BufferFull,
    /// Dropped frames threshold exceeded
    DroppedFrames,
    /// Level capping applied
    LevelCap,
}

/// ID3 metadata sample
#[derive(Clone, Debug)]
pub struct Id3Sample {
    pub pts: f64,
    pub dts: f64,
    pub data: Vec<u8>,
    pub frames: Vec<Id3Frame>,
}

/// ID3 frame data
#[derive(Clone, Debug)]
pub struct Id3Frame {
    pub id: String,
    pub data: Vec<u8>,
}

/// EMSG (Event Message) metadata
#[derive(Clone, Debug)]
pub struct EmsgEvent {
    pub scheme_id_uri: String,
    pub value: String,
    pub timescale: u32,
    pub presentation_time_delta: u32,
    pub event_duration: u32,
    pub id: u32,
    pub message_data: Vec<u8>,
}

/// DATERANGE tag information
#[derive(Clone, Debug)]
pub struct DateRange {
    pub id: String,
    pub class: Option<String>,
    pub start_date: String,
    pub end_date: Option<String>,
    pub duration: Option<f64>,
    pub planned_duration: Option<f64>,
    pub scte35_cmd: Option<Vec<u8>>,
    pub scte35_out: Option<Vec<u8>>,
    pub scte35_in: Option<Vec<u8>>,
    pub end_on_next: bool,
    pub client_attributes: Vec<(String, String)>,
}

/// Subtitle cue
#[derive(Clone, Debug)]
pub struct SubtitleCue {
    pub start_time: f64,
    pub end_time: f64,
    pub text: String,
    pub track_id: usize,
}

/// Event handler trait for HLS events
pub trait HlsEventHandler {
    fn on_event(&self, event: HlsEvent);
}

/// Callback-based event handler
pub struct CallbackEventHandler {
    callback: Box<dyn Fn(HlsEvent) + 'static>,
}

impl CallbackEventHandler {
    pub fn new<F>(callback: F) -> Self
    where
        F: Fn(HlsEvent) + 'static,
    {
        Self {
            callback: Box::new(callback),
        }
    }
}

impl HlsEventHandler for CallbackEventHandler {
    fn on_event(&self, event: HlsEvent) {
        (self.callback)(event);
    }
}

/// Multi-handler event dispatcher
pub struct EventDispatcher {
    handlers: Vec<Box<dyn HlsEventHandler>>,
}

impl EventDispatcher {
    pub fn new() -> Self {
        Self { handlers: Vec::new() }
    }
    
    pub fn add_handler(&mut self, handler: Box<dyn HlsEventHandler>) {
        self.handlers.push(handler);
    }
    
    pub fn dispatch(&self, event: HlsEvent) {
        for handler in &self.handlers {
            handler.on_event(event.clone());
        }
    }
    
    pub fn clear(&mut self) {
        self.handlers.clear();
    }
}

impl Default for EventDispatcher {
    fn default() -> Self {
        Self::new()
    }
}
