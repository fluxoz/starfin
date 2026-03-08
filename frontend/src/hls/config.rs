//! HLS configuration options

/// Quality switching mode for adaptive streaming
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum QualitySwitchMode {
    /// Immediate quality switch at current video position
    Instant,
    /// Quality switch for next loaded fragment
    Smooth,
    /// Quality switch change for next loaded fragment without flushing the buffer
    BandwidthConservative,
}

impl Default for QualitySwitchMode {
    fn default() -> Self {
        Self::Smooth
    }
}

/// Level capping configuration
#[derive(Clone, Debug)]
pub struct LevelCapConfig {
    /// Maximum width for level capping (0 = disabled)
    pub max_width: u32,
    /// Maximum height for level capping (0 = disabled)
    pub max_height: u32,
    /// Maximum bitrate in bits per second (0 = disabled)
    pub max_bitrate: u64,
    /// Maximum dropped frames ratio before quality reduction (0.0-1.0)
    pub max_dropped_frames_ratio: f64,
    /// HDCP level requirement (None = no requirement)
    pub hdcp_level: Option<String>,
}

impl Default for LevelCapConfig {
    fn default() -> Self {
        Self {
            max_width: 0,
            max_height: 0,
            max_bitrate: 0,
            max_dropped_frames_ratio: 0.3,
            hdcp_level: None,
        }
    }
}

/// Configuration for the HLS player
#[derive(Clone, Debug)]
pub struct HlsConfig {
    /// Whether to start playback automatically
    pub auto_start_load: bool,
    /// Start position in seconds (-1 for live edge)
    pub start_position: f64,
    /// Default audio codec preference
    pub default_audio_codec: Option<String>,
    
    // Buffer configuration
    /// Maximum buffer length in seconds
    pub max_buffer_length: f64,
    /// Maximum buffer size in bytes (0 = unlimited)
    pub max_buffer_size: u64,
    /// Minimum buffer to start playback (seconds)
    pub min_buffer_length: f64,
    /// Target buffer ahead of current position (seconds)
    pub buffer_ahead: f64,
    /// Maximum buffer behind current position before cleanup (seconds)
    pub back_buffer_length: f64,
    
    // ABR configuration
    /// Enable adaptive bitrate streaming
    pub abr_enabled: bool,
    /// Starting quality level (-1 for auto)
    pub start_level: i32,
    /// ABR bandwidth estimation window in ms
    pub abr_ema_slow_time: u32,
    /// ABR bandwidth estimation fast window in ms
    pub abr_ema_fast_time: u32,
    /// ABR bandwidth safety factor (0.0-1.0)
    pub abr_bandwidth_factor: f64,
    /// Quality switch mode
    pub quality_switch_mode: QualitySwitchMode,
    /// Emergency switch down bandwidth threshold (ratio)
    pub emergency_switch_threshold: f64,
    
    // Level capping
    /// Level capping configuration
    pub level_cap: LevelCapConfig,
    /// Cap level to player size
    pub cap_level_to_player_size: bool,
    
    // Retry configuration
    /// Maximum retries for manifest load
    pub manifest_load_max_retry: u32,
    /// Maximum retries for level/playlist load
    pub level_load_max_retry: u32,
    /// Maximum retries for fragment load
    pub frag_load_max_retry: u32,
    /// Retry delay in milliseconds
    pub retry_delay: u32,
    /// Maximum retry delay in milliseconds
    pub max_retry_delay: u32,
    /// Retry delay exponential backoff factor
    pub retry_backoff_factor: f64,
    
    // Timeout configuration
    /// Manifest load timeout in milliseconds
    pub manifest_load_timeout: u32,
    /// Level/playlist load timeout in milliseconds
    pub level_load_timeout: u32,
    /// Fragment load timeout in milliseconds
    pub frag_load_timeout: u32,
    
    // Low latency configuration
    /// Enable low latency mode for LL-HLS
    pub low_latency_mode: bool,
    /// Target latency in seconds for live streams
    pub live_sync_duration: f64,
    /// Maximum latency before seeking forward
    pub live_max_latency_duration: f64,
    
    // Debug options
    /// Enable debug logging
    pub debug: bool,
}

impl Default for HlsConfig {
    fn default() -> Self {
        Self {
            auto_start_load: true,
            start_position: -1.0,
            default_audio_codec: None,
            
            max_buffer_length: 60.0,
            max_buffer_size: 0,
            min_buffer_length: 1.0,
            buffer_ahead: 30.0,
            back_buffer_length: 30.0,
            
            abr_enabled: true,
            start_level: -1,
            abr_ema_slow_time: 5000,
            abr_ema_fast_time: 2000,
            abr_bandwidth_factor: 0.8,
            quality_switch_mode: QualitySwitchMode::default(),
            emergency_switch_threshold: 0.5,
            
            level_cap: LevelCapConfig::default(),
            cap_level_to_player_size: true,
            
            manifest_load_max_retry: 3,
            level_load_max_retry: 3,
            frag_load_max_retry: 6,
            retry_delay: 1000,
            max_retry_delay: 8000,
            retry_backoff_factor: 2.0,
            
            manifest_load_timeout: 10000,
            level_load_timeout: 10000,
            frag_load_timeout: 20000,
            
            low_latency_mode: false,
            live_sync_duration: 3.0,
            live_max_latency_duration: 10.0,
            
            debug: false,
        }
    }
}

impl HlsConfig {
    /// Create a new HlsConfig with default values
    pub fn new() -> Self {
        Self::default()
    }
    
    /// Create a config optimized for low latency streaming
    pub fn low_latency() -> Self {
        Self {
            low_latency_mode: true,
            live_sync_duration: 1.5,
            live_max_latency_duration: 5.0,
            buffer_ahead: 10.0,
            back_buffer_length: 10.0,
            ..Default::default()
        }
    }
    
    /// Create a config optimized for bandwidth conservation
    pub fn bandwidth_conservative() -> Self {
        Self {
            quality_switch_mode: QualitySwitchMode::BandwidthConservative,
            abr_bandwidth_factor: 0.7,
            buffer_ahead: 60.0,
            ..Default::default()
        }
    }
}
