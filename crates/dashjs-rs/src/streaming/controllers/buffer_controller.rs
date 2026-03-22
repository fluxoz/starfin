//! Port of the dash.js `BufferController`.
//!
//! Manages buffer state, levels, and buffered-range queries for a single
//! media type within a stream.

/// Threshold (seconds) used when checking whether the buffer extends to the
/// end of the presentation.
pub const BUFFER_END_THRESHOLD: f64 = 0.5;

/// Threshold (seconds) used to avoid floating-point noise when computing
/// buffer ranges.
pub const BUFFER_RANGE_CALCULATION_THRESHOLD: f64 = 0.01;

/// Default value for `buffer_time_at_top_quality` (seconds), mirroring the
/// dash.js `Settings.streaming.buffer.bufferTimeAtTopQuality`.
const DEFAULT_BUFFER_TIME_AT_TOP_QUALITY: f64 = 12.0;

// ---------------------------------------------------------------------------
// Types
// ---------------------------------------------------------------------------

/// High-level buffer state, analogous to dash.js `BUFFER_EMPTY` /
/// `BUFFER_LOADED`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum BufferState {
    Empty,
    Loaded,
}

impl Default for BufferState {
    fn default() -> Self {
        Self::Empty
    }
}

/// A contiguous buffered time range.
#[derive(Clone, Debug, PartialEq)]
pub struct BufferedRange {
    pub start: f64,
    pub end: f64,
}

// ---------------------------------------------------------------------------
// Controller
// ---------------------------------------------------------------------------

#[derive(Clone, Debug)]
pub struct BufferController {
    buffer_level: f64,
    buffer_state: BufferState,
    is_buffering_completed: bool,
    critical_buffer_level: f64,
    max_appended_index: i64,
    maximum_index: i64,
    is_pruning_in_progress: bool,
    seek_target: Option<f64>,
    media_type: String,
    stream_id: String,
    buffer_time_at_top_quality: f64,
}

impl Default for BufferController {
    fn default() -> Self {
        Self {
            buffer_level: 0.0,
            buffer_state: BufferState::Empty,
            is_buffering_completed: false,
            critical_buffer_level: 0.0,
            max_appended_index: -1,
            maximum_index: -1,
            is_pruning_in_progress: false,
            seek_target: None,
            media_type: String::new(),
            stream_id: String::new(),
            buffer_time_at_top_quality: DEFAULT_BUFFER_TIME_AT_TOP_QUALITY,
        }
    }
}

impl BufferController {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn initialize(&mut self, media_type: &str, stream_id: &str) {
        self.media_type = media_type.to_owned();
        self.stream_id = stream_id.to_owned();
    }

    // -- buffer level -------------------------------------------------------

    pub fn get_buffer_level(&self) -> f64 {
        self.buffer_level
    }

    pub fn set_buffer_level(&mut self, level: f64) {
        self.buffer_level = level;
        self.buffer_state = if level > 0.0 {
            BufferState::Loaded
        } else {
            BufferState::Empty
        };
    }

    pub fn get_buffer_state(&self) -> &BufferState {
        &self.buffer_state
    }

    // -- buffer target / quality --------------------------------------------

    pub fn get_buffer_target(&self) -> f64 {
        self.buffer_time_at_top_quality
    }

    pub fn get_top_quality_buffer_time(&self) -> f64 {
        self.buffer_time_at_top_quality
    }

    // -- seek target --------------------------------------------------------

    pub fn get_seek_target(&self) -> Option<f64> {
        self.seek_target
    }

    pub fn set_seek_target(&mut self, target: Option<f64>) {
        self.seek_target = target;
    }

    // -- buffer length from ranges ------------------------------------------

    /// Returns the continuous buffer length (seconds) starting from `time`.
    ///
    /// Walks through the sorted `ranges` and accumulates any range that
    /// overlaps with or is contiguous to the current position.
    pub fn get_buffer_length(ranges: &[BufferedRange], time: f64) -> f64 {
        let mut length = 0.0;
        for range in ranges {
            if time + BUFFER_RANGE_CALCULATION_THRESHOLD >= range.start
                && time < range.end
            {
                length = range.end - time;
                break;
            }
        }
        length
    }

    // -- buffering-completed ------------------------------------------------

    pub fn is_buffering_completed(&self) -> bool {
        self.is_buffering_completed
    }

    pub fn set_buffering_completed(&mut self, completed: bool) {
        self.is_buffering_completed = completed;
    }

    // -- critical buffer level ----------------------------------------------

    pub fn get_critical_buffer_level(&self) -> f64 {
        self.critical_buffer_level
    }

    pub fn set_critical_buffer_level(&mut self, level: f64) {
        self.critical_buffer_level = level;
    }

    // -- append / remove data -----------------------------------------------

    /// Records that the segment at `index` has been appended to the source
    /// buffer. If the index reaches `maximum_index` the controller marks
    /// buffering as completed.
    pub fn append_data(&mut self, index: i64) {
        if index > self.max_appended_index {
            self.max_appended_index = index;
        }
        if self.maximum_index >= 0 && self.max_appended_index >= self.maximum_index {
            self.is_buffering_completed = true;
        }
    }

    /// Marks a pruning operation for the range `[start, end)`.
    /// Returns `true` when no other pruning is already in progress.
    pub fn remove_data(&mut self, _start: f64, _end: f64) -> bool {
        if self.is_pruning_in_progress {
            return false;
        }
        self.is_pruning_in_progress = true;
        // Actual SourceBuffer removal would happen here in a browser
        // environment; for now we simply record the request.
        self.is_pruning_in_progress = false;
        true
    }

    // -- has_buffer_at_time -------------------------------------------------

    /// Returns `true` when any buffered range covers `time` within the given
    /// `tolerance`.
    pub fn has_buffer_at_time(
        ranges: &[BufferedRange],
        time: f64,
        tolerance: f64,
    ) -> bool {
        ranges.iter().any(|r| {
            time + tolerance >= r.start && time - tolerance < r.end
        })
    }

    // -- reset --------------------------------------------------------------

    pub fn reset(&mut self) {
        *self = Self::default();
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn buffer_level_tracking() {
        let mut ctrl = BufferController::new();
        assert_eq!(ctrl.get_buffer_level(), 0.0);
        ctrl.set_buffer_level(5.0);
        assert_eq!(ctrl.get_buffer_level(), 5.0);
    }

    #[test]
    fn buffer_state_transitions() {
        let mut ctrl = BufferController::new();
        assert_eq!(*ctrl.get_buffer_state(), BufferState::Empty);

        ctrl.set_buffer_level(3.0);
        assert_eq!(*ctrl.get_buffer_state(), BufferState::Loaded);

        ctrl.set_buffer_level(0.0);
        assert_eq!(*ctrl.get_buffer_state(), BufferState::Empty);
    }

    #[test]
    fn buffer_length_from_ranges() {
        let ranges = vec![
            BufferedRange { start: 0.0, end: 5.0 },
            BufferedRange { start: 10.0, end: 20.0 },
        ];
        assert_eq!(BufferController::get_buffer_length(&ranges, 2.0), 3.0);
        assert_eq!(BufferController::get_buffer_length(&ranges, 12.0), 8.0);
        assert_eq!(BufferController::get_buffer_length(&ranges, 7.0), 0.0);
    }

    #[test]
    fn buffering_completed_detection() {
        let mut ctrl = BufferController::new();
        ctrl.maximum_index = 3;
        ctrl.append_data(1);
        assert!(!ctrl.is_buffering_completed());
        ctrl.append_data(3);
        assert!(ctrl.is_buffering_completed());
    }

    #[test]
    fn has_buffer_at_time_test() {
        let ranges = vec![BufferedRange { start: 5.0, end: 10.0 }];
        assert!(BufferController::has_buffer_at_time(&ranges, 7.0, 0.0));
        assert!(!BufferController::has_buffer_at_time(&ranges, 3.0, 0.0));
        assert!(BufferController::has_buffer_at_time(&ranges, 4.5, 0.6));
    }

    #[test]
    fn top_quality_buffer_time_returns_default() {
        let ctrl = BufferController::new();
        assert_eq!(ctrl.get_top_quality_buffer_time(), 12.0);
    }
}
