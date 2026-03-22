//! Port of the dash.js `CatchupController`.
//!
//! Computes playback-rate adjustments for low-latency live streams so the
//! player can catch up to (or slow down towards) the target live latency.

// ---------------------------------------------------------------------------
// Types
// ---------------------------------------------------------------------------

/// Strategy used to compute playback-rate adjustments.
#[derive(Clone, Debug, PartialEq)]
pub enum CatchupMode {
    /// Smooth sigmoid-based rate adjustment (default dash.js behaviour).
    Default,
    /// LoLP (Low on Latency Prioritised) — buffer-aware sigmoid.
    LoLP,
    /// Discrete step: jump straight to min/max rate when drift exceeds
    /// threshold.
    Step,
}

impl std::fmt::Display for CatchupMode {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Default => write!(f, "Default"),
            Self::LoLP => write!(f, "LoLP"),
            Self::Step => write!(f, "Step"),
        }
    }
}

// ---------------------------------------------------------------------------
// Controller
// ---------------------------------------------------------------------------

#[derive(Clone, Debug)]
pub struct CatchupController {
    mode: CatchupMode,
    target_latency: f64,
    max_drift: f64,
    min_playback_rate: f64,
    max_playback_rate: f64,
    playback_buffer_min: f64,
    initialized: bool,
    is_catchup_seek_in_progress: bool,
}

impl Default for CatchupController {
    fn default() -> Self {
        Self {
            mode: CatchupMode::Default,
            target_latency: 0.0,
            max_drift: 0.5,
            min_playback_rate: 0.95,
            max_playback_rate: 1.05,
            playback_buffer_min: 0.5,
            initialized: false,
            is_catchup_seek_in_progress: false,
        }
    }
}

impl CatchupController {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn with_mode(mode: CatchupMode) -> Self {
        Self {
            mode,
            ..Self::default()
        }
    }

    pub fn initialize(&mut self, target_latency: f64, max_drift: f64) {
        self.target_latency = target_latency;
        self.max_drift = max_drift;
        self.initialized = true;
    }

    // -- core rate computation ---------------------------------------------

    /// Returns the recommended playback rate given the current latency and
    /// buffer level. The result is always clamped to
    /// `[min_playback_rate, max_playback_rate]`.
    pub fn get_playback_rate_adjustment(
        &self,
        current_latency: f64,
        buffer_level: f64,
    ) -> f64 {
        match self.mode {
            CatchupMode::Default => self.rate_default(current_latency),
            CatchupMode::LoLP => self.rate_lolp(current_latency, buffer_level),
            CatchupMode::Step => self.rate_step(current_latency),
        }
    }

    /// Default mode — smooth sigmoid around `target_latency`.
    fn rate_default(&self, current_latency: f64) -> f64 {
        let delta = current_latency - self.target_latency;
        if delta.abs() < 0.01 {
            return 1.0;
        }
        let rate = self.min_playback_rate
            + (self.max_playback_rate - self.min_playback_rate)
                / (1.0 + (-5.0 * delta).exp());
        self.clamp_rate(rate)
    }

    /// LoLP mode — when buffer is low, bias towards slowing down; otherwise
    /// fall back to the default latency-based sigmoid.
    fn rate_lolp(&self, current_latency: f64, buffer_level: f64) -> f64 {
        if buffer_level < self.playback_buffer_min {
            // Buffer-aware slowdown: sigmoid centred on buffer_min.
            let buf_delta = buffer_level - self.playback_buffer_min;
            let rate = self.min_playback_rate
                + (1.0 - self.min_playback_rate)
                    / (1.0 + (-5.0 * buf_delta).exp());
            self.clamp_rate(rate)
        } else {
            self.rate_default(current_latency)
        }
    }

    /// Step mode — discrete jumps: max, min, or 1.0.
    fn rate_step(&self, current_latency: f64) -> f64 {
        if current_latency > self.target_latency + self.max_drift {
            self.max_playback_rate
        } else if current_latency < self.target_latency - self.max_drift {
            self.min_playback_rate
        } else {
            1.0
        }
    }

    fn clamp_rate(&self, rate: f64) -> f64 {
        rate.clamp(self.min_playback_rate, self.max_playback_rate)
    }

    // -- seek-to-live ------------------------------------------------------

    /// Returns `true` when the player is so far behind that a seek back to
    /// the live edge is warranted (latency exceeds `target + 2 × max_drift`).
    pub fn should_seek_to_live(&self, current_latency: f64) -> bool {
        current_latency > self.target_latency + 2.0 * self.max_drift
    }

    pub fn is_catchup_seek_in_progress(&self) -> bool {
        self.is_catchup_seek_in_progress
    }

    pub fn set_catchup_seek_in_progress(&mut self, value: bool) {
        self.is_catchup_seek_in_progress = value;
    }

    pub fn reset(&mut self) {
        *self = Self {
            mode: self.mode.clone(),
            ..Self::default()
        };
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    fn init_default() -> CatchupController {
        let mut c = CatchupController::new();
        c.initialize(3.0, 0.5);
        c
    }

    // -- Default mode -------------------------------------------------------

    #[test]
    fn default_mode_near_target_returns_one() {
        let c = init_default();
        let rate = c.get_playback_rate_adjustment(3.005, 5.0);
        assert!((rate - 1.0).abs() < f64::EPSILON);
    }

    #[test]
    fn default_mode_high_latency_speeds_up() {
        let c = init_default();
        let rate = c.get_playback_rate_adjustment(5.0, 5.0);
        assert!(rate > 1.0);
        assert!(rate <= c.max_playback_rate);
    }

    #[test]
    fn default_mode_low_latency_slows_down() {
        let c = init_default();
        let rate = c.get_playback_rate_adjustment(1.0, 5.0);
        assert!(rate < 1.0);
        assert!(rate >= c.min_playback_rate);
    }

    // -- LoLP mode ----------------------------------------------------------

    #[test]
    fn lolp_low_buffer_slows_down() {
        let mut c = CatchupController::with_mode(CatchupMode::LoLP);
        c.initialize(3.0, 0.5);
        let rate = c.get_playback_rate_adjustment(3.0, 0.1);
        assert!(rate < 1.0);
    }

    #[test]
    fn lolp_sufficient_buffer_uses_default() {
        let mut c = CatchupController::with_mode(CatchupMode::LoLP);
        c.initialize(3.0, 0.5);
        let rate_lolp = c.get_playback_rate_adjustment(5.0, 5.0);
        let rate_def = init_default().get_playback_rate_adjustment(5.0, 5.0);
        assert!((rate_lolp - rate_def).abs() < f64::EPSILON);
    }

    // -- Step mode ----------------------------------------------------------

    #[test]
    fn step_mode_high_latency() {
        let mut c = CatchupController::with_mode(CatchupMode::Step);
        c.initialize(3.0, 0.5);
        let rate = c.get_playback_rate_adjustment(4.0, 5.0);
        assert!((rate - c.max_playback_rate).abs() < f64::EPSILON);
    }

    #[test]
    fn step_mode_low_latency() {
        let mut c = CatchupController::with_mode(CatchupMode::Step);
        c.initialize(3.0, 0.5);
        let rate = c.get_playback_rate_adjustment(2.0, 5.0);
        assert!((rate - c.min_playback_rate).abs() < f64::EPSILON);
    }

    #[test]
    fn step_mode_within_drift() {
        let mut c = CatchupController::with_mode(CatchupMode::Step);
        c.initialize(3.0, 0.5);
        let rate = c.get_playback_rate_adjustment(3.2, 5.0);
        assert!((rate - 1.0).abs() < f64::EPSILON);
    }

    // -- seek-to-live -------------------------------------------------------

    #[test]
    fn should_seek_to_live_when_far_behind() {
        let c = init_default();
        assert!(c.should_seek_to_live(5.0));
        assert!(!c.should_seek_to_live(3.5));
    }

    // -- configuration ------------------------------------------------------

    #[test]
    fn parameter_configuration() {
        let mut c = CatchupController::new();
        c.min_playback_rate = 0.9;
        c.max_playback_rate = 1.1;
        c.initialize(2.0, 1.0);
        let rate = c.get_playback_rate_adjustment(5.0, 5.0);
        assert!(rate <= 1.1);
        assert!(rate >= 0.9);
    }
}
