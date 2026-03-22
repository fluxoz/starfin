// Port of dash.js/src/core/Settings.js
//
// Comprehensive settings with serde support and Default impls matching
// the dash.js default values.

use serde::{Deserialize, Serialize};

// ─── Top-level settings ──────────────────────────────────────────────────────

/// Root player settings, mirroring the dash.js `PlayerSettings` object.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Settings {
    pub debug: DebugSettings,
    pub streaming: StreamingSettings,
    pub errors: ErrorSettings,
}

impl Default for Settings {
    fn default() -> Self {
        Self {
            debug: DebugSettings::default(),
            streaming: StreamingSettings::default(),
            errors: ErrorSettings::default(),
        }
    }
}

// ─── Debug ───────────────────────────────────────────────────────────────────

/// `debug` section — controls log output.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct DebugSettings {
    /// Log level (0 = none … 5 = debug).  Default: 3 (warning).
    pub log_level: u8,
    /// When `true`, a `Log` event is dispatched for every log message.
    pub dispatch_event: bool,
}

impl Default for DebugSettings {
    fn default() -> Self {
        Self {
            log_level: 3, // LOG_LEVEL_WARNING
            dispatch_event: false,
        }
    }
}

// ─── Error settings ──────────────────────────────────────────────────────────

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct ErrorSettings {
    pub recover_attempts: RecoverAttempts,
}

impl Default for ErrorSettings {
    fn default() -> Self {
        Self {
            recover_attempts: RecoverAttempts::default(),
        }
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct RecoverAttempts {
    pub media_error_decode: u32,
}

impl Default for RecoverAttempts {
    fn default() -> Self {
        Self {
            media_error_decode: 5,
        }
    }
}

// ─── Streaming settings ─────────────────────────────────────────────────────

/// `streaming` section.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct StreamingSettings {
    pub abandon_load_timeout: u32,
    pub wallclock_time_update_interval: u32,
    pub manifest_update_retry_interval: u32,
    pub live_update_time_threshold_in_milliseconds: u32,
    pub cache_init_segments: bool,
    pub cache_init_segments_limit: u32,
    pub apply_service_description: bool,
    pub apply_producer_reference_time: bool,
    pub apply_content_steering: bool,
    pub enable_manifest_duration_mismatch_fix: bool,
    pub parse_inband_prft: bool,
    pub enable_manifest_timescale_mismatch_fix: bool,

    pub capabilities: CapabilitiesSettings,
    pub events: EventControllerSettings,
    pub time_shift_buffer: TimeShiftBufferSettings,
    pub metrics: MetricsSettings,
    pub delay: DelaySettings,
    pub protection: ProtectionSettings,
    pub buffer: BufferSettings,
    pub gaps: GapSettings,
    pub utc_synchronization: UtcSynchronizationSettings,
    pub scheduling: SchedulingSettings,
    pub text: TextSettings,
    pub live_catchup: LiveCatchupSettings,
    pub last_bitrate_caching_info: CachingInfoSettings,
    pub last_media_settings_caching_info: CachingInfoSettings,
    pub save_last_media_settings_for_current_streaming_session: bool,
    pub cache_load_thresholds: AudioVideoSetting<u32>,
    pub track_switch_mode: AudioVideoSetting<TrackSwitchMode>,
    pub include_preselections_in_mediainfo_array: bool,
    pub include_preselections_for_initial_track_selection: bool,
    pub ignore_selection_priority: bool,
    pub prioritize_role_main: bool,
    pub assume_default_role_as_main: bool,
    pub selection_mode_for_initial_track: TrackSelectionMode,
    pub fragment_request_timeout: u32,
    pub fragment_request_progress_timeout: i32,
    pub manifest_request_timeout: u32,
    pub retry_intervals: RetrySettings,
    pub retry_attempts: RetrySettings,
    pub abr: AbrSettings,
    pub cmcd: CmcdSettings,
    pub cmsd: CmsdSettings,
    pub enhancement: EnhancementSettings,
    pub default_scheme_id_uri: DefaultSchemeIdUri,
    pub dvb_reporting: DvbReportingSettings,
}

impl Default for StreamingSettings {
    fn default() -> Self {
        Self {
            abandon_load_timeout: 10_000,
            wallclock_time_update_interval: 100,
            manifest_update_retry_interval: 100,
            live_update_time_threshold_in_milliseconds: 0,
            cache_init_segments: false,
            cache_init_segments_limit: 50,
            apply_service_description: true,
            apply_producer_reference_time: true,
            apply_content_steering: true,
            enable_manifest_duration_mismatch_fix: true,
            parse_inband_prft: false,
            enable_manifest_timescale_mismatch_fix: false,
            capabilities: CapabilitiesSettings::default(),
            events: EventControllerSettings::default(),
            time_shift_buffer: TimeShiftBufferSettings::default(),
            metrics: MetricsSettings::default(),
            delay: DelaySettings::default(),
            protection: ProtectionSettings::default(),
            buffer: BufferSettings::default(),
            gaps: GapSettings::default(),
            utc_synchronization: UtcSynchronizationSettings::default(),
            scheduling: SchedulingSettings::default(),
            text: TextSettings::default(),
            live_catchup: LiveCatchupSettings::default(),
            last_bitrate_caching_info: CachingInfoSettings {
                enabled: true,
                ttl: 360_000,
            },
            last_media_settings_caching_info: CachingInfoSettings {
                enabled: true,
                ttl: 360_000,
            },
            save_last_media_settings_for_current_streaming_session: true,
            cache_load_thresholds: AudioVideoSetting {
                audio: 5,
                video: 10,
            },
            track_switch_mode: AudioVideoSetting {
                audio: TrackSwitchMode::AlwaysReplace,
                video: TrackSwitchMode::NeverReplace,
            },
            include_preselections_in_mediainfo_array: true,
            include_preselections_for_initial_track_selection: false,
            ignore_selection_priority: false,
            prioritize_role_main: true,
            assume_default_role_as_main: true,
            selection_mode_for_initial_track: TrackSelectionMode::LowestStartupDelay,
            fragment_request_timeout: 20_000,
            fragment_request_progress_timeout: -1,
            manifest_request_timeout: 10_000,
            retry_intervals: RetrySettings {
                mpd: 500,
                xlink_expansion: 500,
                media_segment: 1000,
                init_segment: 1000,
                bitstream_switching_segment: 1000,
                index_segment: 1000,
                mss_fragment_info_segment: 1000,
                license: 1000,
                license_certificate: 1000,
                other: 1000,
                low_latency_reduction_factor: 10,
                low_latency_multiply_factor: 1,
            },
            retry_attempts: RetrySettings {
                mpd: 3,
                xlink_expansion: 1,
                media_segment: 3,
                init_segment: 3,
                bitstream_switching_segment: 3,
                index_segment: 3,
                mss_fragment_info_segment: 3,
                license: 3,
                license_certificate: 3,
                other: 3,
                low_latency_reduction_factor: 1,
                low_latency_multiply_factor: 5,
            },
            abr: AbrSettings::default(),
            cmcd: CmcdSettings::default(),
            cmsd: CmsdSettings::default(),
            enhancement: EnhancementSettings::default(),
            default_scheme_id_uri: DefaultSchemeIdUri::default(),
            dvb_reporting: DvbReportingSettings::default(),
        }
    }
}

// ─── Capabilities ────────────────────────────────────────────────────────────

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct CapabilitiesSettings {
    pub filter_unsupported_essential_properties: bool,
    pub use_media_capabilities_api: bool,
    pub filter_video_colorimetry_essential_properties: bool,
    pub filter_hdr_metadata_format_essential_properties: bool,
    pub filter_audio_channel_configuration: bool,
}

impl Default for CapabilitiesSettings {
    fn default() -> Self {
        Self {
            filter_unsupported_essential_properties: true,
            use_media_capabilities_api: true,
            filter_video_colorimetry_essential_properties: true,
            filter_hdr_metadata_format_essential_properties: true,
            filter_audio_channel_configuration: false,
        }
    }
}

// ─── Event controller ────────────────────────────────────────────────────────

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct EventControllerSettings {
    pub event_controller_refresh_delay: u32,
    pub delete_event_message_data_timeout: i32,
}

impl Default for EventControllerSettings {
    fn default() -> Self {
        Self {
            event_controller_refresh_delay: 100,
            delete_event_message_data_timeout: 10_000,
        }
    }
}

// ─── TimeShiftBuffer ─────────────────────────────────────────────────────────

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct TimeShiftBufferSettings {
    pub calc_from_segment_timeline: bool,
    pub fallback_to_segment_timeline: bool,
}

impl Default for TimeShiftBufferSettings {
    fn default() -> Self {
        Self {
            calc_from_segment_timeline: false,
            fallback_to_segment_timeline: true,
        }
    }
}

// ─── Metrics ─────────────────────────────────────────────────────────────────

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct MetricsSettings {
    pub max_list_depth: u32,
}

impl Default for MetricsSettings {
    fn default() -> Self {
        Self {
            max_list_depth: 100,
        }
    }
}

// ─── Delay (live) ────────────────────────────────────────────────────────────

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct DelaySettings {
    /// Fragment-count-based live delay. `None` = auto.
    pub live_delay_fragment_count: Option<f64>,
    /// Explicit live delay in seconds. `None` = auto.
    pub live_delay: Option<f64>,
    pub use_suggested_presentation_delay: bool,
}

impl Default for DelaySettings {
    fn default() -> Self {
        Self {
            live_delay_fragment_count: None, // NaN in JS
            live_delay: None,                // NaN in JS
            use_suggested_presentation_delay: true,
        }
    }
}

// ─── Protection (DRM) ────────────────────────────────────────────────────────

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct ProtectionSettings {
    pub keep_protection_media_keys: bool,
    pub keep_protection_media_keys_maximum_open_sessions: i32,
    pub ignore_eme_encrypted_event: bool,
    pub detect_playready_message_format: bool,
    pub ignore_key_statuses: bool,
}

impl Default for ProtectionSettings {
    fn default() -> Self {
        Self {
            keep_protection_media_keys: false,
            keep_protection_media_keys_maximum_open_sessions: -1,
            ignore_eme_encrypted_event: false,
            detect_playready_message_format: true,
            ignore_key_statuses: false,
        }
    }
}

// ─── Buffer ──────────────────────────────────────────────────────────────────

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct BufferSettings {
    pub enable_seek_decorrelation_fix: bool,
    /// `None` = auto-detect (enabled for non low-latency).
    pub fast_switch_enabled: Option<bool>,
    pub flush_buffer_at_track_switch: bool,
    pub reuse_existing_source_buffers: bool,
    /// Seconds between buffer prune cycles.
    pub buffer_pruning_interval: f64,
    /// Seconds of buffer behind the playhead to keep.
    pub buffer_to_keep: f64,
    /// Default forward buffer target in seconds.
    pub buffer_time_default: f64,
    /// Forward buffer target at top quality (short form).
    pub buffer_time_at_top_quality: f64,
    /// Forward buffer target at top quality (long form).
    pub buffer_time_at_top_quality_long_form: f64,
    /// Duration threshold for "long form" content in seconds.
    pub long_form_content_duration_threshold: f64,
    /// Initial buffer level before playback starts. `None` = auto.
    pub initial_buffer_level: Option<f64>,
    pub stall_threshold: f64,
    pub low_latency_stall_threshold: f64,
    pub use_append_window: bool,
    pub set_stall_state: bool,
    pub avoid_current_time_range_pruning: bool,
    pub use_change_type: bool,
    pub media_source_duration_infinity: bool,
    pub reset_source_buffers_for_track_switch: bool,
    pub synthetic_stall_events: SyntheticStallSettings,
}

impl Default for BufferSettings {
    fn default() -> Self {
        Self {
            enable_seek_decorrelation_fix: false,
            fast_switch_enabled: None,
            flush_buffer_at_track_switch: false,
            reuse_existing_source_buffers: true,
            buffer_pruning_interval: 10.0,
            buffer_to_keep: 20.0,
            buffer_time_default: 18.0,
            buffer_time_at_top_quality: 30.0,
            buffer_time_at_top_quality_long_form: 60.0,
            long_form_content_duration_threshold: 600.0,
            initial_buffer_level: None,
            stall_threshold: 0.3,
            low_latency_stall_threshold: 0.3,
            use_append_window: true,
            set_stall_state: true,
            avoid_current_time_range_pruning: false,
            use_change_type: true,
            media_source_duration_infinity: true,
            reset_source_buffers_for_track_switch: false,
            synthetic_stall_events: SyntheticStallSettings::default(),
        }
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct SyntheticStallSettings {
    pub enabled: bool,
    pub ignore_ready_state: bool,
}

impl Default for SyntheticStallSettings {
    fn default() -> Self {
        Self {
            enabled: false,
            ignore_ready_state: false,
        }
    }
}

// ─── Gaps ────────────────────────────────────────────────────────────────────

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct GapSettings {
    pub jump_gaps: bool,
    pub jump_large_gaps: bool,
    pub small_gap_limit: f64,
    pub threshold: f64,
    pub enable_seek_fix: bool,
    pub enable_stall_fix: bool,
    pub stall_seek: f64,
    pub seek_offset: f64,
    pub check_interval: u32,
}

impl Default for GapSettings {
    fn default() -> Self {
        Self {
            jump_gaps: true,
            jump_large_gaps: true,
            small_gap_limit: 1.5,
            threshold: 0.3,
            enable_seek_fix: true,
            enable_stall_fix: false,
            stall_seek: 0.1,
            seek_offset: 0.0,
            check_interval: 250,
        }
    }
}

// ─── UTC Synchronization ─────────────────────────────────────────────────────

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct UtcSynchronizationSettings {
    pub enabled: bool,
    pub use_manifest_date_header_time_source: bool,
    pub background_attempts: u32,
    pub time_between_sync_attempts: u32,
    pub maximum_time_between_sync_attempts: u32,
    pub minimum_time_between_sync_attempts: u32,
    pub time_between_sync_attempts_adjustment_factor: u32,
    pub maximum_allowed_drift: u32,
    pub enable_background_sync_after_segment_download_error: bool,
    pub default_timing_source: TimingSource,
    pub artificial_time_offset_to_apply: i64,
}

impl Default for UtcSynchronizationSettings {
    fn default() -> Self {
        Self {
            enabled: true,
            use_manifest_date_header_time_source: true,
            background_attempts: 2,
            time_between_sync_attempts: 30,
            maximum_time_between_sync_attempts: 600,
            minimum_time_between_sync_attempts: 2,
            time_between_sync_attempts_adjustment_factor: 2,
            maximum_allowed_drift: 100,
            enable_background_sync_after_segment_download_error: true,
            default_timing_source: TimingSource {
                scheme: "urn:mpeg:dash:utc:http-xsdate:2014".into(),
                value: "https://time.akamai.com/?iso&ms".into(),
            },
            artificial_time_offset_to_apply: 0,
        }
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct TimingSource {
    pub scheme: String,
    pub value: String,
}

// ─── Scheduling ──────────────────────────────────────────────────────────────

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct SchedulingSettings {
    pub default_timeout: u32,
    pub low_latency_timeout: u32,
    pub schedule_while_paused: bool,
}

impl Default for SchedulingSettings {
    fn default() -> Self {
        Self {
            default_timeout: 500,
            low_latency_timeout: 0,
            schedule_while_paused: true,
        }
    }
}

// ─── Text / subtitles ────────────────────────────────────────────────────────

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct TextSettings {
    pub default_enabled: bool,
    pub dispatch_for_manual_rendering: bool,
    pub extend_segmented_cues: bool,
    pub imsc: ImscSettings,
    pub webvtt: WebVttSettings,
}

impl Default for TextSettings {
    fn default() -> Self {
        Self {
            default_enabled: true,
            dispatch_for_manual_rendering: false,
            extend_segmented_cues: true,
            imsc: ImscSettings::default(),
            webvtt: WebVttSettings::default(),
        }
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct ImscSettings {
    pub display_forced_only_mode: bool,
    pub enable_roll_up: bool,
}

impl Default for ImscSettings {
    fn default() -> Self {
        Self {
            display_forced_only_mode: false,
            enable_roll_up: true,
        }
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct WebVttSettings {
    pub custom_rendering_enabled: bool,
}

impl Default for WebVttSettings {
    fn default() -> Self {
        Self {
            custom_rendering_enabled: false,
        }
    }
}

// ─── Live catchup ────────────────────────────────────────────────────────────

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct LiveCatchupSettings {
    /// Maximum latency drift before seeking. `None` = auto.
    pub max_drift: Option<f64>,
    pub playback_rate: MinMax,
    pub step: LiveCatchupStep,
    /// Latency difference threshold for resetting to 1× rate. -1 = disabled.
    pub live_threshold: f64,
    pub playback_buffer_min: f64,
    /// `None` = auto-detect based on low-latency.
    pub enabled: Option<bool>,
    pub mode: LiveCatchupMode,
}

impl Default for LiveCatchupSettings {
    fn default() -> Self {
        Self {
            max_drift: None,
            playback_rate: MinMax {
                min: None,
                max: None,
            },
            step: LiveCatchupStep::default(),
            live_threshold: -1.0,
            playback_buffer_min: 0.5,
            enabled: None,
            mode: LiveCatchupMode::Default,
        }
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct LiveCatchupStep {
    pub start: MinMax,
    pub stop: MinMax,
}

impl Default for LiveCatchupStep {
    fn default() -> Self {
        Self {
            start: MinMax {
                min: None,
                max: None,
            },
            stop: MinMax {
                min: None,
                max: None,
            },
        }
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct MinMax {
    pub min: Option<f64>,
    pub max: Option<f64>,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub enum LiveCatchupMode {
    #[serde(rename = "liveCatchupModeDefault")]
    Default,
    #[serde(rename = "liveCatchupModeLOLP")]
    Lolp,
    #[serde(rename = "liveCatchupModeStep")]
    Step,
}

// ─── Caching info ────────────────────────────────────────────────────────────

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct CachingInfoSettings {
    pub enabled: bool,
    /// Time-to-live in milliseconds.
    pub ttl: u64,
}

// ─── Audio / Video generic pair ──────────────────────────────────────────────

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct AudioVideoSetting<T> {
    pub audio: T,
    pub video: T,
}

// ─── Track switch mode ───────────────────────────────────────────────────────

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub enum TrackSwitchMode {
    #[serde(rename = "alwaysReplace")]
    AlwaysReplace,
    #[serde(rename = "neverReplace")]
    NeverReplace,
}

// ─── Track selection mode ────────────────────────────────────────────────────

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub enum TrackSelectionMode {
    #[serde(rename = "highestBitrate")]
    HighestBitrate,
    #[serde(rename = "firstTrack")]
    FirstTrack,
    #[serde(rename = "highestEfficiency")]
    HighestEfficiency,
    #[serde(rename = "widestRange")]
    WidestRange,
    #[serde(rename = "lowestStartupDelay")]
    LowestStartupDelay,
}

// ─── Retry settings ──────────────────────────────────────────────────────────

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct RetrySettings {
    pub mpd: u32,
    pub xlink_expansion: u32,
    pub media_segment: u32,
    pub init_segment: u32,
    pub bitstream_switching_segment: u32,
    pub index_segment: u32,
    pub mss_fragment_info_segment: u32,
    pub license: u32,
    pub license_certificate: u32,
    pub other: u32,
    pub low_latency_reduction_factor: u32,
    pub low_latency_multiply_factor: u32,
}

// ─── ABR settings ────────────────────────────────────────────────────────────

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct AbrSettings {
    pub limit_bitrate_by_portal: bool,
    pub use_pixel_ratio_in_limit_bitrate_by_portal: bool,
    pub limit_bitrate_by_portal_minimum: u32,
    pub enable_supplemental_property_adaptation_set_switching: bool,
    pub rules: AbrRules,
    pub throughput: ThroughputSettings,
    pub max_bitrate: AudioVideoSetting<i64>,
    pub min_bitrate: AudioVideoSetting<i64>,
    pub initial_bitrate: AudioVideoSetting<i64>,
    pub auto_switch_bitrate: AudioVideoSetting<bool>,
}

impl Default for AbrSettings {
    fn default() -> Self {
        Self {
            limit_bitrate_by_portal: false,
            use_pixel_ratio_in_limit_bitrate_by_portal: false,
            limit_bitrate_by_portal_minimum: 0,
            enable_supplemental_property_adaptation_set_switching: true,
            rules: AbrRules::default(),
            throughput: ThroughputSettings::default(),
            max_bitrate: AudioVideoSetting {
                audio: -1,
                video: -1,
            },
            min_bitrate: AudioVideoSetting {
                audio: -1,
                video: -1,
            },
            initial_bitrate: AudioVideoSetting {
                audio: -1,
                video: -1,
            },
            auto_switch_bitrate: AudioVideoSetting {
                audio: true,
                video: true,
            },
        }
    }
}

// ─── ABR rules ───────────────────────────────────────────────────────────────

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct AbrRules {
    pub throughput_rule: AbrRuleConfig,
    pub bola_rule: AbrRuleConfig,
    pub insufficient_buffer_rule: AbrRuleWithParams<InsufficientBufferParams>,
    pub switch_history_rule: AbrRuleWithParams<SwitchHistoryParams>,
    pub dropped_frames_rule: AbrRuleWithParams<DroppedFramesParams>,
    pub abandon_requests_rule: AbrRuleWithParams<AbandonRequestsParams>,
    pub l2a_rule: AbrRuleConfig,
    pub lo_lp_rule: AbrRuleConfig,
}

impl Default for AbrRules {
    fn default() -> Self {
        Self {
            throughput_rule: AbrRuleConfig { active: true },
            bola_rule: AbrRuleConfig { active: true },
            insufficient_buffer_rule: AbrRuleWithParams {
                active: true,
                parameters: InsufficientBufferParams {
                    throughput_safety_factor: 0.7,
                    segment_ignore_count: 2,
                },
            },
            switch_history_rule: AbrRuleWithParams {
                active: true,
                parameters: SwitchHistoryParams {
                    sample_size: 8,
                    switch_percentage_threshold: 0.075,
                },
            },
            dropped_frames_rule: AbrRuleWithParams {
                active: false,
                parameters: DroppedFramesParams {
                    minimum_sample_size: 375,
                    dropped_frames_percentage_threshold: 0.15,
                },
            },
            abandon_requests_rule: AbrRuleWithParams {
                active: true,
                parameters: AbandonRequestsParams {
                    abandon_duration_multiplier: 1.8,
                    min_segment_download_time_threshold_in_ms: 500,
                    min_throughput_samples_threshold: 6,
                },
            },
            l2a_rule: AbrRuleConfig { active: false },
            lo_lp_rule: AbrRuleConfig { active: false },
        }
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct AbrRuleConfig {
    pub active: bool,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct AbrRuleWithParams<P> {
    pub active: bool,
    pub parameters: P,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct InsufficientBufferParams {
    pub throughput_safety_factor: f64,
    pub segment_ignore_count: u32,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct SwitchHistoryParams {
    pub sample_size: u32,
    pub switch_percentage_threshold: f64,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct DroppedFramesParams {
    pub minimum_sample_size: u32,
    pub dropped_frames_percentage_threshold: f64,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct AbandonRequestsParams {
    pub abandon_duration_multiplier: f64,
    pub min_segment_download_time_threshold_in_ms: u32,
    pub min_throughput_samples_threshold: u32,
}

// ─── Throughput settings ─────────────────────────────────────────────────────

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct ThroughputSettings {
    pub average_calculation_mode: ThroughputCalculationMode,
    pub low_latency_download_time_calculation_mode: LowLatencyDownloadTimeCalculationMode,
    pub use_resource_timing_api: bool,
    pub use_network_information_api: NetworkInformationApiSettings,
    pub use_dead_time_latency: bool,
    pub bandwidth_safety_factor: f64,
    pub sample_settings: ThroughputSampleSettings,
    pub ewma: EwmaSettings,
}

impl Default for ThroughputSettings {
    fn default() -> Self {
        Self {
            average_calculation_mode: ThroughputCalculationMode::Ewma,
            low_latency_download_time_calculation_mode:
                LowLatencyDownloadTimeCalculationMode::MoofParsing,
            use_resource_timing_api: true,
            use_network_information_api: NetworkInformationApiSettings::default(),
            use_dead_time_latency: true,
            bandwidth_safety_factor: 0.9,
            sample_settings: ThroughputSampleSettings::default(),
            ewma: EwmaSettings::default(),
        }
    }
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub enum ThroughputCalculationMode {
    #[serde(rename = "throughputCalculationModeEwma")]
    Ewma,
    #[serde(rename = "throughputCalculationModeArithmeticMean")]
    ArithmeticMean,
    #[serde(rename = "throughputCalculationModeHarmonicMean")]
    HarmonicMean,
    #[serde(rename = "throughputCalculationModeByte")]
    ByteSizeBased,
    #[serde(rename = "throughputCalculationModeDateHeader")]
    DateHeader,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub enum LowLatencyDownloadTimeCalculationMode {
    #[serde(rename = "lowLatencyDownloadTimeCalculationModeMoofParsing")]
    MoofParsing,
    #[serde(rename = "lowLatencyDownloadTimeCalculationModeAast")]
    Aast,
    #[serde(rename = "lowLatencyDownloadTimeCalculationModeDownloadedData")]
    DownloadedData,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct NetworkInformationApiSettings {
    pub xhr: bool,
    pub fetch: bool,
}

impl Default for NetworkInformationApiSettings {
    fn default() -> Self {
        Self {
            xhr: false,
            fetch: false,
        }
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct ThroughputSampleSettings {
    pub live: u32,
    pub vod: u32,
    pub enable_sample_size_adjustment: bool,
    pub decrease_scale: f64,
    pub increase_scale: f64,
    pub max_measurements_to_keep: u32,
    pub average_latency_sample_amount: u32,
}

impl Default for ThroughputSampleSettings {
    fn default() -> Self {
        Self {
            live: 3,
            vod: 4,
            enable_sample_size_adjustment: true,
            decrease_scale: 0.7,
            increase_scale: 1.3,
            max_measurements_to_keep: 20,
            average_latency_sample_amount: 4,
        }
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct EwmaSettings {
    pub throughput_slow_half_life_seconds: f64,
    pub throughput_fast_half_life_seconds: f64,
    pub latency_slow_half_life_count: f64,
    pub latency_fast_half_life_count: f64,
    pub weight_download_time_multiplication_factor: f64,
}

impl Default for EwmaSettings {
    fn default() -> Self {
        Self {
            throughput_slow_half_life_seconds: 8.0,
            throughput_fast_half_life_seconds: 3.0,
            latency_slow_half_life_count: 2.0,
            latency_fast_half_life_count: 1.0,
            weight_download_time_multiplication_factor: 0.0015,
        }
    }
}

// ─── CMCD ────────────────────────────────────────────────────────────────────

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct CmcdSettings {
    pub apply_parameters_from_mpd: bool,
    pub enabled: bool,
    pub sid: Option<String>,
    pub cid: Option<String>,
    pub rtp: Option<u32>,
    pub rtp_safety_factor: u32,
    pub mode: CmcdMode,
    pub enabled_keys: Vec<String>,
    pub include_in_requests: Vec<String>,
    pub version: u32,
}

impl Default for CmcdSettings {
    fn default() -> Self {
        Self {
            apply_parameters_from_mpd: true,
            enabled: false,
            sid: None,
            cid: None,
            rtp: None,
            rtp_safety_factor: 5,
            mode: CmcdMode::Query,
            enabled_keys: vec![
                "br".into(),
                "d".into(),
                "ot".into(),
                "tb".into(),
                "bl".into(),
                "dl".into(),
                "mtp".into(),
                "nor".into(),
                "nrr".into(),
                "su".into(),
                "bs".into(),
                "rtp".into(),
                "cid".into(),
                "pr".into(),
                "sf".into(),
                "sid".into(),
                "st".into(),
                "v".into(),
            ],
            include_in_requests: vec!["segment".into(), "mpd".into()],
            version: 1,
        }
    }
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub enum CmcdMode {
    #[serde(rename = "query")]
    Query,
    #[serde(rename = "header")]
    Header,
}

// ─── CMSD ────────────────────────────────────────────────────────────────────

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct CmsdSettings {
    pub enabled: bool,
    pub abr: CmsdAbrSettings,
}

impl Default for CmsdSettings {
    fn default() -> Self {
        Self {
            enabled: false,
            abr: CmsdAbrSettings::default(),
        }
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct CmsdAbrSettings {
    pub apply_mb: bool,
    pub etp_weight_ratio: f64,
}

impl Default for CmsdAbrSettings {
    fn default() -> Self {
        Self {
            apply_mb: false,
            etp_weight_ratio: 0.0,
        }
    }
}

// ─── Enhancement (LCEVC) ─────────────────────────────────────────────────────

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct EnhancementSettings {
    pub enabled: bool,
    pub codecs: Vec<String>,
}

impl Default for EnhancementSettings {
    fn default() -> Self {
        Self {
            enabled: false,
            codecs: vec!["lvc1".into()],
        }
    }
}

// ─── Default scheme ID URIs ──────────────────────────────────────────────────

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct DefaultSchemeIdUri {
    pub viewpoint: String,
    pub audio_channel_configuration: String,
    pub role: String,
    pub accessibility: String,
}

impl Default for DefaultSchemeIdUri {
    fn default() -> Self {
        Self {
            viewpoint: String::new(),
            audio_channel_configuration: "urn:mpeg:mpegB:cicp:ChannelConfiguration".into(),
            role: "urn:mpeg:dash:role:2011".into(),
            accessibility: "urn:mpeg:dash:role:2011".into(),
        }
    }
}

// ─── DVB Reporting ───────────────────────────────────────────────────────────

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct DvbReportingSettings {
    pub reporting_url: Option<String>,
}

impl Default for DvbReportingSettings {
    fn default() -> Self {
        Self {
            reporting_url: None,
        }
    }
}

// ─── Tests ───────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_buffer_values() {
        let s = Settings::default();
        let b = &s.streaming.buffer;
        assert!((b.buffer_time_at_top_quality - 30.0).abs() < f64::EPSILON);
        assert!((b.buffer_time_at_top_quality_long_form - 60.0).abs() < f64::EPSILON);
        assert!((b.long_form_content_duration_threshold - 600.0).abs() < f64::EPSILON);
        assert!((b.buffer_pruning_interval - 10.0).abs() < f64::EPSILON);
        assert!((b.buffer_to_keep - 20.0).abs() < f64::EPSILON);
    }

    #[test]
    fn default_abr_values() {
        let s = Settings::default();
        assert!(!s.streaming.abr.limit_bitrate_by_portal);
        assert!((s.streaming.abr.throughput.bandwidth_safety_factor - 0.9).abs() < f64::EPSILON);
    }

    #[test]
    fn serialization_round_trip() {
        let original = Settings::default();
        let json = serde_json::to_string(&original).unwrap();
        let deserialized: Settings = serde_json::from_str(&json).unwrap();
        assert_eq!(
            deserialized.streaming.buffer.buffer_time_at_top_quality,
            original.streaming.buffer.buffer_time_at_top_quality,
        );
    }
}
