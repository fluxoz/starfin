//! Main HLS Controller
//!
//! Provides a simplified API for HLS playback that integrates with the existing
//! video player component. This module focuses on manifest parsing, quality
//! level management, and ABR decisions rather than full player lifecycle.

use crate::hls::abr::{AbrController, AbrMode, LevelSelection, PlayerSize, SwitchReason};
use crate::hls::config::{HlsConfig, QualitySwitchMode};
use crate::hls::error::{ErrorCode, HlsError, HlsResult};
use crate::hls::loader::ManifestLoader;
use crate::hls::metadata::MetadataParser;
use crate::hls::playlist::{is_master_playlist, Level, MasterPlaylist, MediaPlaylist};

/// Quality level info for UI display
#[derive(Clone, Debug)]
pub struct QualityLevel {
    pub index: usize,
    pub name: String,
    pub bitrate: u64,
    pub resolution: Option<(u32, u32)>,
    pub enabled: bool,
}

impl From<&Level> for QualityLevel {
    fn from(level: &Level) -> Self {
        Self {
            index: level.index,
            name: level.name.clone(),
            bitrate: level.bitrate,
            resolution: level.resolution.as_ref().map(|r| (r.width, r.height)),
            enabled: true,
        }
    }
}

/// HLS stream information loaded from manifest
#[derive(Clone, Debug)]
pub struct HlsStreamInfo {
    /// Whether this is a multi-bitrate stream
    pub is_master: bool,
    /// Available quality levels
    pub levels: Vec<QualityLevel>,
    /// Total duration (if known from playlist)
    pub duration: Option<f64>,
    /// Whether the stream is VOD or live
    pub is_vod: bool,
    /// Initial recommended level index
    pub initial_level: usize,
}

/// HLS Controller for managing adaptive streaming
/// 
/// This controller handles:
/// - Loading and parsing HLS manifests
/// - Managing quality levels and ABR decisions
/// - Tracking bandwidth estimates
/// - Level capping based on player size
pub struct HlsController {
    /// Configuration
    config: HlsConfig,
    /// ABR controller for quality decisions
    abr: AbrController,
    /// Manifest loader
    manifest_loader: ManifestLoader,
    /// Metadata parser
    metadata: MetadataParser,
    /// Master playlist (if multi-bitrate)
    master_playlist: Option<MasterPlaylist>,
    /// Current media playlist
    media_playlist: Option<MediaPlaylist>,
    /// Manifest URL
    manifest_url: String,
    /// Recovery attempt count
    recovery_attempts: u32,
}

impl HlsController {
    /// Create a new HLS controller with default config
    pub fn new() -> Self {
        Self::with_config(HlsConfig::default())
    }
    
    /// Create a new HLS controller with custom config
    pub fn with_config(config: HlsConfig) -> Self {
        Self {
            abr: AbrController::new(&config),
            manifest_loader: ManifestLoader::new(&config),
            config,
            metadata: MetadataParser::new(),
            master_playlist: None,
            media_playlist: None,
            manifest_url: String::new(),
            recovery_attempts: 0,
        }
    }
    
    /// Load an HLS manifest and return stream information
    pub async fn load_manifest(&mut self, url: &str) -> HlsResult<HlsStreamInfo> {
        self.manifest_url = url.to_string();
        
        // Load manifest text
        let manifest_text = self.manifest_loader.load(url).await?;
        
        if is_master_playlist(&manifest_text) {
            // Parse as master playlist
            let master = MasterPlaylist::parse(&manifest_text, url)?;
            
            // Set up ABR with levels
            self.abr.set_levels(master.levels.clone());
            
            // Determine initial level
            let initial_level = if self.config.start_level >= 0 {
                (self.config.start_level as usize).min(master.levels.len().saturating_sub(1))
            } else {
                // Start with middle quality
                master.levels.len() / 2
            };
            
            let levels: Vec<QualityLevel> = master.levels.iter()
                .map(|l| {
                    let mut ql = QualityLevel::from(l);
                    ql.enabled = self.abr.is_level_allowed(l.index);
                    ql
                })
                .collect();
            
            self.master_playlist = Some(master);
            
            Ok(HlsStreamInfo {
                is_master: true,
                levels,
                duration: None, // Will be known after loading media playlist
                is_vod: true, // Assume VOD until we load media playlist
                initial_level,
            })
        } else {
            // Parse as media playlist directly
            let media = MediaPlaylist::parse(&manifest_text, url)?;
            
            // Update date ranges in metadata parser
            self.metadata.set_date_ranges(media.date_ranges.clone());
            
            let duration = if media.total_duration > 0.0 {
                Some(media.total_duration)
            } else {
                None
            };
            
            let is_vod = media.is_vod;
            
            // Create single level for non-master playlist
            let single_level = Level {
                index: 0,
                bitrate: 0,
                avg_bitrate: None,
                resolution: None,
                video_codec: None,
                audio_codec: None,
                codecs: None,
                frame_rate: None,
                hdcp_level: None,
                audio_group: None,
                subtitle_group: None,
                cc_group: None,
                url: url.to_string(),
                name: "Default".to_string(),
            };
            
            self.abr.set_levels(vec![single_level.clone()]);
            self.media_playlist = Some(media);
            
            Ok(HlsStreamInfo {
                is_master: false,
                levels: vec![QualityLevel::from(&single_level)],
                duration,
                is_vod,
                initial_level: 0,
            })
        }
    }
    
    /// Load a specific level's media playlist
    pub async fn load_level_playlist(&mut self, level: usize) -> HlsResult<MediaPlaylist> {
        let master = self.master_playlist.as_ref().ok_or_else(|| {
            HlsError::internal("No master playlist - call load_manifest first")
        })?;
        
        let level_info = master.levels.get(level).ok_or_else(|| {
            HlsError::manifest(ErrorCode::InvalidLevelIndex, format!("Level {} not found", level))
        })?;
        
        let playlist_text = self.manifest_loader.load(&level_info.url).await?;
        let media = MediaPlaylist::parse(&playlist_text, &level_info.url)?;
        
        // Update date ranges in metadata parser
        self.metadata.set_date_ranges(media.date_ranges.clone());
        
        self.media_playlist = Some(media.clone());
        
        Ok(media)
    }
    
    /// Get the current media playlist
    pub fn media_playlist(&self) -> Option<&MediaPlaylist> {
        self.media_playlist.as_ref()
    }
    
    /// Get available quality levels with current enabled status
    pub fn get_levels(&self) -> Vec<QualityLevel> {
        if let Some(ref master) = self.master_playlist {
            master.levels.iter().map(|l| {
                let mut ql = QualityLevel::from(l);
                ql.enabled = self.abr.is_level_allowed(l.index);
                ql
            }).collect()
        } else {
            vec![]
        }
    }
    
    /// Get current quality level index
    pub fn current_level(&self) -> usize {
        self.abr.current_level()
    }
    
    /// Set manual quality level
    pub fn set_level(&mut self, level: usize) {
        self.abr.set_mode(AbrMode::Manual(level));
        self.abr.set_current_level(level);
    }
    
    /// Enable automatic quality selection
    pub fn set_auto_level(&mut self) {
        self.abr.set_mode(AbrMode::Auto);
    }
    
    /// Check if auto quality is enabled
    pub fn is_auto_quality(&self) -> bool {
        matches!(self.abr.mode(), AbrMode::Auto)
    }
    
    /// Add a bandwidth sample from a loaded fragment
    pub fn add_bandwidth_sample(&mut self, loaded_bytes: u64, load_time_ms: f64) {
        self.abr.add_bandwidth_sample(loaded_bytes, load_time_ms, js_sys::Date::now());
    }
    
    /// Get estimated bandwidth in bits per second
    pub fn bandwidth_estimate(&self) -> Option<f64> {
        self.abr.estimated_bandwidth()
    }
    
    /// Update dropped frames count for quality adaptation
    pub fn update_dropped_frames(&mut self, total: u64, dropped: u64) {
        self.abr.update_dropped_frames(total, dropped, js_sys::Date::now());
    }
    
    /// Select the next quality level based on current conditions
    /// Returns Some(new_level) if a switch is recommended
    pub fn select_next_level(&mut self, buffer_length: f64) -> Option<(usize, SwitchReason)> {
        let timestamp = js_sys::Date::now();
        match self.abr.select_level(buffer_length, timestamp) {
            LevelSelection::Switch { to, reason, .. } => {
                Some((to, reason))
            }
            LevelSelection::KeepCurrent => None,
        }
    }
    
    /// Confirm level switch after fragment loaded
    pub fn confirm_switch(&mut self) {
        self.abr.confirm_switch();
    }
    
    /// Set player size for level capping
    pub fn set_player_size(&mut self, width: u32, height: u32, pixel_ratio: f64) {
        self.abr.set_player_size(PlayerSize::new(width, height).with_pixel_ratio(pixel_ratio));
    }
    
    /// Set quality switch mode
    pub fn set_switch_mode(&mut self, mode: QualitySwitchMode) {
        self.abr.set_switch_mode(mode);
    }
    
    /// Get quality switch mode
    pub fn switch_mode(&self) -> QualitySwitchMode {
        self.abr.switch_mode()
    }
    
    /// Get the manifest URL
    pub fn manifest_url(&self) -> &str {
        &self.manifest_url
    }
    
    /// Get the URL for a specific segment
    pub fn get_segment_url(&self, segment_index: usize) -> Option<String> {
        self.media_playlist.as_ref()
            .and_then(|p| p.segments.get(segment_index))
            .map(|s| s.uri.clone())
    }
    
    /// Get segment count
    pub fn segment_count(&self) -> usize {
        self.media_playlist.as_ref()
            .map(|p| p.segments.len())
            .unwrap_or(0)
    }
    
    /// Get segment start time
    pub fn get_segment_start_time(&self, segment_index: usize) -> Option<f64> {
        self.media_playlist.as_ref()
            .and_then(|p| p.segments.get(segment_index))
            .map(|s| s.start_time)
    }
    
    /// Get segment duration
    pub fn get_segment_duration(&self, segment_index: usize) -> Option<f64> {
        self.media_playlist.as_ref()
            .and_then(|p| p.segments.get(segment_index))
            .map(|s| s.duration)
    }
    
    /// Find segment index for a given time position
    pub fn segment_index_at_time(&self, time: f64) -> Option<usize> {
        self.media_playlist.as_ref()
            .and_then(|p| p.segment_index_at_time(time))
    }
    
    /// Get init segment URL if present
    pub fn get_init_segment_url(&self) -> Option<String> {
        self.media_playlist.as_ref()
            .and_then(|p| p.current_map.as_ref())
            .map(|m| m.uri.clone())
    }
    
    /// Check if stream is encrypted
    pub fn is_encrypted(&self) -> bool {
        self.media_playlist.as_ref()
            .map(|p| p.is_encrypted())
            .unwrap_or(false)
    }
    
    /// Get target segment duration from playlist
    pub fn target_duration(&self) -> f64 {
        self.media_playlist.as_ref()
            .map(|p| p.target_duration)
            .unwrap_or(6.0)
    }
    
    /// Reset controller state (for error recovery)
    pub fn reset(&mut self) {
        self.abr.reset();
        self.recovery_attempts = 0;
    }
}

impl Default for HlsController {
    fn default() -> Self {
        Self::new()
    }
}
