//! Port of the dash.js `GapController`.
//!
//! Detects gaps in the media buffer and optionally jumps over them to keep
//! playback moving. Also detects playback stalls caused by unbridged gaps.

use super::buffer_controller::BufferedRange;

/// Number of wallclock ticks without time advancement before we consider
/// playback stalled.
const THRESHOLD_TO_STALL: u32 = 10;

/// Small offset (seconds) added when jumping past a gap so the seek lands
/// safely inside the next buffered range.
const GAP_JUMP_WAITING_TIME_OFFSET: f64 = 0.1;

// ---------------------------------------------------------------------------
// Types
// ---------------------------------------------------------------------------

/// Describes a detected gap between two buffered ranges.
#[derive(Clone, Debug, PartialEq)]
pub struct GapInfo {
    pub start: f64,
    pub end: f64,
    pub duration: f64,
}

// ---------------------------------------------------------------------------
// Controller
// ---------------------------------------------------------------------------

#[derive(Clone, Debug)]
pub struct GapController {
    small_gap_limit: f64,
    jump_gaps_enabled: bool,
    jump_large_gaps: bool,
    gap_threshold: f64,
    last_playback_time: f64,
    wallclock_ticked: u32,
    last_gap_jump_position: f64,
    initialized: bool,
}

impl Default for GapController {
    fn default() -> Self {
        Self {
            small_gap_limit: 0.8,
            jump_gaps_enabled: true,
            jump_large_gaps: true,
            gap_threshold: 0.1,
            last_playback_time: -1.0,
            wallclock_ticked: 0,
            last_gap_jump_position: -1.0,
            initialized: false,
        }
    }
}

impl GapController {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn with_settings(
        small_gap_limit: f64,
        jump_gaps: bool,
        jump_large_gaps: bool,
    ) -> Self {
        Self {
            small_gap_limit,
            jump_gaps_enabled: jump_gaps,
            jump_large_gaps,
            ..Self::default()
        }
    }

    pub fn initialize(&mut self) {
        self.initialized = true;
        self.wallclock_ticked = 0;
        self.last_playback_time = -1.0;
        self.last_gap_jump_position = -1.0;
    }

    /// Detect a gap at `current_time` by examining the buffered ranges.
    ///
    /// Returns `Some(GapInfo)` when `current_time` falls between two buffered
    /// ranges (i.e. past the end of one range and before the start of the
    /// next).
    pub fn detect_gap(
        &self,
        buffered_ranges: &[BufferedRange],
        current_time: f64,
    ) -> Option<GapInfo> {
        if buffered_ranges.len() < 2 {
            return None;
        }

        for i in 0..buffered_ranges.len() - 1 {
            let gap_start = buffered_ranges[i].end;
            let gap_end = buffered_ranges[i + 1].start;

            if current_time >= gap_start - self.gap_threshold
                && current_time < gap_end
            {
                let duration = gap_end - gap_start;
                return Some(GapInfo {
                    start: gap_start,
                    end: gap_end,
                    duration,
                });
            }
        }
        None
    }

    /// Decide whether the controller should jump over a given gap based on
    /// the current settings.
    pub fn should_jump_gap(&self, gap: &GapInfo) -> bool {
        if !self.jump_gaps_enabled {
            return false;
        }
        gap.duration <= self.small_gap_limit || self.jump_large_gaps
    }

    /// If a gap is detected at `current_time` **and** the settings allow
    /// jumping it, return the target time to seek to. Also updates the
    /// internal `last_gap_jump_position`.
    pub fn jump_gap(
        &mut self,
        buffered_ranges: &[BufferedRange],
        current_time: f64,
    ) -> Option<f64> {
        let gap = self.detect_gap(buffered_ranges, current_time)?;
        if !self.should_jump_gap(&gap) {
            return None;
        }
        let target = gap.end + GAP_JUMP_WAITING_TIME_OFFSET;
        self.last_gap_jump_position = target;
        Some(target)
    }

    /// Called once per wallclock interval. Tracks whether playback time is
    /// advancing; used for stall detection.
    pub fn on_wallclock_tick(&mut self, current_time: f64) {
        if (current_time - self.last_playback_time).abs() < 0.001 {
            self.wallclock_ticked += 1;
        } else {
            self.wallclock_ticked = 0;
        }
        self.last_playback_time = current_time;
    }

    /// Returns `true` when playback appears stalled — i.e. the playback
    /// position has not changed for at least `THRESHOLD_TO_STALL` ticks.
    pub fn is_stalled(&self) -> bool {
        self.wallclock_ticked >= THRESHOLD_TO_STALL
            && self.last_playback_time >= 0.0
    }

    pub fn reset(&mut self) {
        *self = Self {
            small_gap_limit: self.small_gap_limit,
            jump_gaps_enabled: self.jump_gaps_enabled,
            jump_large_gaps: self.jump_large_gaps,
            gap_threshold: self.gap_threshold,
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

    fn sample_ranges() -> Vec<BufferedRange> {
        vec![
            BufferedRange { start: 0.0, end: 5.0 },
            BufferedRange { start: 6.0, end: 12.0 },
        ]
    }

    #[test]
    fn detect_gap_between_ranges() {
        let ctrl = GapController::new();
        let gap = ctrl.detect_gap(&sample_ranges(), 5.0).unwrap();
        assert_eq!(gap.start, 5.0);
        assert_eq!(gap.end, 6.0);
        assert!((gap.duration - 1.0).abs() < f64::EPSILON);
    }

    #[test]
    fn no_gap_when_continuous() {
        let ctrl = GapController::new();
        let ranges = vec![
            BufferedRange { start: 0.0, end: 5.0 },
            BufferedRange { start: 5.0, end: 10.0 },
        ];
        // At time 3.0 there's no gap — we're inside the first range.
        assert!(ctrl.detect_gap(&ranges, 3.0).is_none());
    }

    #[test]
    fn should_jump_small_gap() {
        let ctrl = GapController::with_settings(0.8, true, false);
        let small_gap = GapInfo { start: 5.0, end: 5.5, duration: 0.5 };
        assert!(ctrl.should_jump_gap(&small_gap));
    }

    #[test]
    fn should_jump_large_gap() {
        let ctrl = GapController::with_settings(0.8, true, true);
        let large_gap = GapInfo { start: 5.0, end: 10.0, duration: 5.0 };
        assert!(ctrl.should_jump_gap(&large_gap));
    }

    #[test]
    fn should_not_jump_large_gap_when_disabled() {
        let ctrl = GapController::with_settings(0.8, true, false);
        let large_gap = GapInfo { start: 5.0, end: 10.0, duration: 5.0 };
        assert!(!ctrl.should_jump_gap(&large_gap));
    }

    #[test]
    fn jump_gap_returns_correct_position() {
        let mut ctrl = GapController::new();
        ctrl.initialize();
        let target = ctrl.jump_gap(&sample_ranges(), 5.0).unwrap();
        assert!((target - (6.0 + GAP_JUMP_WAITING_TIME_OFFSET)).abs() < f64::EPSILON);
    }

    #[test]
    fn stall_detection() {
        let mut ctrl = GapController::new();
        ctrl.initialize();
        assert!(!ctrl.is_stalled());

        // First tick sets the baseline; subsequent ticks increment the counter.
        for _ in 0..=THRESHOLD_TO_STALL {
            ctrl.on_wallclock_tick(5.0);
        }
        assert!(ctrl.is_stalled());

        // Time advances → no longer stalled.
        ctrl.on_wallclock_tick(6.0);
        assert!(!ctrl.is_stalled());
    }
}
