//! Adaptive Bitrate (ABR) controller
//!
//! Implements bandwidth estimation and quality level selection with multiple
//! switching modes for optimal playback experience.

use crate::hls::config::{HlsConfig, LevelCapConfig};
use crate::hls::playlist::Level;
use std::collections::VecDeque;

pub use crate::hls::config::QualitySwitchMode;

/// ABR operation mode
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum AbrMode {
    /// Automatic quality selection based on bandwidth
    Auto,
    /// Manual quality selection (locked to specific level)
    Manual(usize),
}

impl Default for AbrMode {
    fn default() -> Self {
        Self::Auto
    }
}

/// Bandwidth sample for estimation
#[allow(dead_code)]
#[derive(Clone, Debug)]
struct BandwidthSample {
    /// Bandwidth in bits per second
    bandwidth: f64,
    /// Sample timestamp (ms since start)
    timestamp: f64,
    /// Weight for EMA calculation
    weight: f64,
}

/// Exponential Moving Average calculator
struct EmaCalculator {
    slow_alpha: f64,
    fast_alpha: f64,
    slow_ema: Option<f64>,
    fast_ema: Option<f64>,
}

impl EmaCalculator {
    fn new(slow_time: f64, fast_time: f64) -> Self {
        // Alpha = 2 / (N + 1) where N is time window in samples
        // Assuming ~1 sample per fragment (~6s), adjust accordingly
        let slow_alpha = 2.0 / (slow_time / 1000.0 + 1.0);
        let fast_alpha = 2.0 / (fast_time / 1000.0 + 1.0);
        
        Self {
            slow_alpha: slow_alpha.min(1.0),
            fast_alpha: fast_alpha.min(1.0),
            slow_ema: None,
            fast_ema: None,
        }
    }
    
    fn update(&mut self, value: f64) {
        self.slow_ema = Some(match self.slow_ema {
            Some(prev) => prev + self.slow_alpha * (value - prev),
            None => value,
        });
        
        self.fast_ema = Some(match self.fast_ema {
            Some(prev) => prev + self.fast_alpha * (value - prev),
            None => value,
        });
    }
    
    /// Get conservative estimate (minimum of slow and fast)
    fn estimate(&self) -> Option<f64> {
        match (self.slow_ema, self.fast_ema) {
            (Some(slow), Some(fast)) => Some(slow.min(fast)),
            (Some(v), None) | (None, Some(v)) => Some(v),
            (None, None) => None,
        }
    }
}

/// Dropped frames tracker for quality adaptation
struct DroppedFramesTracker {
    total_frames: u64,
    dropped_frames: u64,
    window_start_frames: u64,
    window_start_dropped: u64,
    window_duration: f64,
    last_update: f64,
}

impl DroppedFramesTracker {
    fn new(window_duration: f64) -> Self {
        Self {
            total_frames: 0,
            dropped_frames: 0,
            window_start_frames: 0,
            window_start_dropped: 0,
            window_duration,
            last_update: 0.0,
        }
    }
    
    fn update(&mut self, total: u64, dropped: u64, timestamp: f64) {
        self.total_frames = total;
        self.dropped_frames = dropped;
        
        // Reset window periodically
        if timestamp - self.last_update > self.window_duration * 1000.0 {
            self.window_start_frames = total;
            self.window_start_dropped = dropped;
            self.last_update = timestamp;
        }
    }
    
    fn dropped_ratio(&self) -> f64 {
        let frames = self.total_frames - self.window_start_frames;
        let dropped = self.dropped_frames - self.window_start_dropped;
        
        if frames > 0 {
            dropped as f64 / frames as f64
        } else {
            0.0
        }
    }
}

/// Player size for level capping
#[derive(Clone, Debug)]
pub struct PlayerSize {
    pub width: u32,
    pub height: u32,
    pub pixel_ratio: f64,
}

impl PlayerSize {
    pub fn new(width: u32, height: u32) -> Self {
        Self {
            width,
            height,
            pixel_ratio: 1.0,
        }
    }
    
    pub fn with_pixel_ratio(mut self, ratio: f64) -> Self {
        self.pixel_ratio = ratio;
        self
    }
    
    /// Get effective resolution accounting for pixel ratio
    pub fn effective_width(&self) -> u32 {
        (self.width as f64 * self.pixel_ratio).ceil() as u32
    }
    
    pub fn effective_height(&self) -> u32 {
        (self.height as f64 * self.pixel_ratio).ceil() as u32
    }
}

/// ABR Controller for adaptive streaming
pub struct AbrController {
    /// Current ABR mode
    mode: AbrMode,
    /// Quality switch mode
    switch_mode: QualitySwitchMode,
    /// Current level index
    current_level: usize,
    /// Next level (for smooth switching)
    next_level: Option<usize>,
    /// Bandwidth estimator
    ema: EmaCalculator,
    /// Recent bandwidth samples
    samples: VecDeque<BandwidthSample>,
    /// Maximum samples to keep
    max_samples: usize,
    /// Dropped frames tracker
    dropped_tracker: DroppedFramesTracker,
    /// Player size for level capping
    player_size: Option<PlayerSize>,
    /// Level capping config
    level_cap: LevelCapConfig,
    /// Bandwidth safety factor
    bandwidth_factor: f64,
    /// Emergency switch threshold
    emergency_threshold: f64,
    /// Whether to cap to player size
    cap_to_player_size: bool,
    /// Cached capped levels (indices of allowed levels)
    capped_levels: Option<Vec<usize>>,
    /// Available levels reference
    levels: Vec<Level>,
    /// Last switch timestamp (for cooldown)
    last_switch_time: f64,
    /// Minimum time between switches (ms)
    switch_cooldown: f64,
}

impl AbrController {
    /// Create a new ABR controller with the given configuration
    pub fn new(config: &HlsConfig) -> Self {
        Self {
            mode: if config.start_level >= 0 {
                AbrMode::Manual(config.start_level as usize)
            } else {
                AbrMode::Auto
            },
            switch_mode: config.quality_switch_mode,
            current_level: 0,
            next_level: None,
            ema: EmaCalculator::new(
                config.abr_ema_slow_time as f64,
                config.abr_ema_fast_time as f64,
            ),
            samples: VecDeque::new(),
            max_samples: 20,
            dropped_tracker: DroppedFramesTracker::new(10.0),
            player_size: None,
            level_cap: config.level_cap.clone(),
            bandwidth_factor: config.abr_bandwidth_factor,
            emergency_threshold: config.emergency_switch_threshold,
            cap_to_player_size: config.cap_level_to_player_size,
            capped_levels: None,
            levels: Vec::new(),
            last_switch_time: 0.0,
            switch_cooldown: 3000.0, // 3 second minimum between switches
        }
    }
    
    /// Set available levels
    pub fn set_levels(&mut self, levels: Vec<Level>) {
        self.levels = levels;
        self.capped_levels = None; // Invalidate cache
        self.update_level_cap();
    }
    
    /// Set player size for level capping
    pub fn set_player_size(&mut self, size: PlayerSize) {
        self.player_size = Some(size);
        self.capped_levels = None;
        self.update_level_cap();
    }
    
    /// Update level cap based on current constraints
    fn update_level_cap(&mut self) {
        let mut allowed_levels: Vec<usize> = Vec::new();
        
        for level in &self.levels {
            let mut allowed = true;
            
            // Check resolution cap
            if self.level_cap.max_width > 0 || self.level_cap.max_height > 0 {
                if !level.fits_resolution(self.level_cap.max_width, self.level_cap.max_height) {
                    allowed = false;
                }
            }
            
            // Check player size cap
            if allowed && self.cap_to_player_size {
                if let Some(ref size) = self.player_size {
                    if !level.fits_resolution(size.effective_width(), size.effective_height()) {
                        allowed = false;
                    }
                }
            }
            
            // Check bitrate cap
            if allowed && self.level_cap.max_bitrate > 0 {
                if level.bitrate > self.level_cap.max_bitrate {
                    allowed = false;
                }
            }
            
            // Check HDCP
            if allowed {
                if !level.is_hdcp_compatible(self.level_cap.hdcp_level.as_deref()) {
                    allowed = false;
                }
            }
            
            if allowed {
                allowed_levels.push(level.index);
            }
        }
        
        // Ensure at least one level is allowed (lowest bitrate)
        if allowed_levels.is_empty() && !self.levels.is_empty() {
            let lowest = self.levels.iter().min_by_key(|l| l.bitrate);
            if let Some(l) = lowest {
                allowed_levels.push(l.index);
            }
        }
        
        self.capped_levels = Some(allowed_levels);
    }
    
    /// Get allowed levels after capping
    pub fn allowed_levels(&self) -> &[usize] {
        self.capped_levels.as_deref().unwrap_or(&[])
    }
    
    /// Check if a level is allowed
    pub fn is_level_allowed(&self, level: usize) -> bool {
        self.capped_levels
            .as_ref()
            .map(|levels| levels.contains(&level))
            .unwrap_or(true)
    }
    
    /// Set ABR mode
    pub fn set_mode(&mut self, mode: AbrMode) {
        self.mode = mode;
    }
    
    /// Get current ABR mode
    pub fn mode(&self) -> AbrMode {
        self.mode
    }
    
    /// Set quality switch mode
    pub fn set_switch_mode(&mut self, mode: QualitySwitchMode) {
        self.switch_mode = mode;
    }
    
    /// Get quality switch mode
    pub fn switch_mode(&self) -> QualitySwitchMode {
        self.switch_mode
    }
    
    /// Add a bandwidth sample from fragment load
    pub fn add_bandwidth_sample(&mut self, loaded_bytes: u64, load_time_ms: f64, timestamp: f64) {
        if load_time_ms <= 0.0 {
            return;
        }
        
        // Calculate bandwidth in bits per second
        let bandwidth = (loaded_bytes as f64 * 8.0 * 1000.0) / load_time_ms;
        
        // Update EMA
        self.ema.update(bandwidth);
        
        // Store sample
        let sample = BandwidthSample {
            bandwidth,
            timestamp,
            weight: 1.0,
        };
        
        self.samples.push_back(sample);
        while self.samples.len() > self.max_samples {
            self.samples.pop_front();
        }
    }
    
    /// Get estimated bandwidth in bits per second
    pub fn estimated_bandwidth(&self) -> Option<f64> {
        self.ema.estimate()
    }
    
    /// Update dropped frames count
    pub fn update_dropped_frames(&mut self, total: u64, dropped: u64, timestamp: f64) {
        self.dropped_tracker.update(total, dropped, timestamp);
    }
    
    /// Get dropped frames ratio
    pub fn dropped_ratio(&self) -> f64 {
        self.dropped_tracker.dropped_ratio()
    }
    
    /// Get current level index
    pub fn current_level(&self) -> usize {
        self.current_level
    }
    
    /// Set current level directly (for manual mode)
    pub fn set_current_level(&mut self, level: usize) {
        self.current_level = level;
        if matches!(self.mode, AbrMode::Manual(_)) {
            self.mode = AbrMode::Manual(level);
        }
    }
    
    /// Get next level for loading (may differ from current in smooth mode)
    pub fn next_load_level(&self) -> usize {
        self.next_level.unwrap_or(self.current_level)
    }
    
    /// Select the next quality level based on current conditions
    pub fn select_level(&mut self, buffer_length: f64, timestamp: f64) -> LevelSelection {
        // Check cooldown
        if timestamp - self.last_switch_time < self.switch_cooldown {
            return LevelSelection::KeepCurrent;
        }
        
        match self.mode {
            AbrMode::Manual(level) => {
                if level != self.current_level && self.is_level_allowed(level) {
                    self.switch_to_level(level, timestamp);
                    return LevelSelection::Switch {
                        from: self.current_level,
                        to: level,
                        reason: SwitchReason::Manual,
                    };
                }
                LevelSelection::KeepCurrent
            }
            AbrMode::Auto => self.auto_select_level(buffer_length, timestamp),
        }
    }
    
    /// Automatic level selection
    fn auto_select_level(&mut self, buffer_length: f64, timestamp: f64) -> LevelSelection {
        let allowed = match &self.capped_levels {
            Some(levels) => levels.clone(),
            None => return LevelSelection::KeepCurrent,
        };
        
        if allowed.is_empty() {
            return LevelSelection::KeepCurrent;
        }
        
        // Check for dropped frames
        if self.dropped_tracker.dropped_ratio() > self.level_cap.max_dropped_frames_ratio {
            // Try to switch down
            if let Some(lower) = self.find_lower_level(self.current_level, &allowed) {
                self.switch_to_level(lower, timestamp);
                return LevelSelection::Switch {
                    from: self.current_level,
                    to: lower,
                    reason: SwitchReason::DroppedFrames,
                };
            }
        }
        
        // Get bandwidth estimate
        let bandwidth = match self.ema.estimate() {
            Some(bw) => bw,
            None => return LevelSelection::KeepCurrent,
        };
        
        // Emergency switch if buffer is critical
        if buffer_length < 2.0 {
            let effective_bw = bandwidth * self.emergency_threshold;
            if let Some(emergency_level) = self.find_level_for_bandwidth(effective_bw, &allowed) {
                if emergency_level < self.current_level {
                    self.switch_to_level(emergency_level, timestamp);
                    return LevelSelection::Switch {
                        from: self.current_level,
                        to: emergency_level,
                        reason: SwitchReason::Emergency,
                    };
                }
            }
        }
        
        // Normal ABR selection
        let effective_bw = bandwidth * self.bandwidth_factor;
        let selected_level = self.find_level_for_bandwidth(effective_bw, &allowed);
        
        match selected_level {
            Some(level) if level != self.current_level => {
                let reason = if level > self.current_level {
                    SwitchReason::BandwidthIncrease
                } else {
                    SwitchReason::BandwidthDecrease
                };
                
                // For upward switches, require more buffer
                if level > self.current_level && buffer_length < 10.0 {
                    return LevelSelection::KeepCurrent;
                }
                
                self.switch_to_level(level, timestamp);
                LevelSelection::Switch {
                    from: self.current_level,
                    to: level,
                    reason,
                }
            }
            _ => LevelSelection::KeepCurrent,
        }
    }
    
    /// Find the best level for a given bandwidth
    fn find_level_for_bandwidth(&self, bandwidth: f64, allowed: &[usize]) -> Option<usize> {
        let bandwidth_u64 = bandwidth as u64;
        
        // Find highest allowed level that fits within bandwidth
        allowed.iter()
            .filter_map(|&idx| self.levels.get(idx).map(|l| (idx, l)))
            .filter(|(_, level)| level.bitrate <= bandwidth_u64)
            .max_by_key(|(_, level)| level.bitrate)
            .map(|(idx, _)| idx)
    }
    
    /// Find a lower quality level than current
    fn find_lower_level(&self, current: usize, allowed: &[usize]) -> Option<usize> {
        let current_bitrate = self.levels.get(current)?.bitrate;
        
        allowed.iter()
            .filter_map(|&idx| self.levels.get(idx).map(|l| (idx, l)))
            .filter(|(_, level)| level.bitrate < current_bitrate)
            .max_by_key(|(_, level)| level.bitrate)
            .map(|(idx, _)| idx)
    }
    
    /// Switch to a new level
    fn switch_to_level(&mut self, level: usize, timestamp: f64) {
        match self.switch_mode {
            QualitySwitchMode::Instant => {
                self.current_level = level;
                self.next_level = Some(level);
            }
            QualitySwitchMode::Smooth => {
                self.next_level = Some(level);
                // Current level will be updated after fragment loads
            }
            QualitySwitchMode::BandwidthConservative => {
                self.next_level = Some(level);
                // Don't flush buffer, switch on next fragment only
            }
        }
        self.last_switch_time = timestamp;
    }
    
    /// Confirm level switch after fragment loaded
    pub fn confirm_switch(&mut self) {
        if let Some(next) = self.next_level.take() {
            self.current_level = next;
        }
    }
    
    /// Reset ABR state
    pub fn reset(&mut self) {
        self.samples.clear();
        self.ema = EmaCalculator::new(5000.0, 2000.0);
        self.next_level = None;
        self.last_switch_time = 0.0;
    }
}

/// Result of level selection
#[derive(Clone, Debug)]
pub enum LevelSelection {
    /// Keep current level
    KeepCurrent,
    /// Switch to new level
    Switch {
        from: usize,
        to: usize,
        reason: SwitchReason,
    },
}

/// Reason for quality switch
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum SwitchReason {
    /// Manual selection by user
    Manual,
    /// Bandwidth estimate increased
    BandwidthIncrease,
    /// Bandwidth estimate decreased
    BandwidthDecrease,
    /// Emergency switch due to buffer starvation
    Emergency,
    /// Too many dropped frames
    DroppedFrames,
    /// Level capping applied
    LevelCap,
}
