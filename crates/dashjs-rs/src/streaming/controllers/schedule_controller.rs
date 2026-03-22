//! Port of the dash.js `ScheduleController`.
//!
//! Decides *when* the next fragment should be requested based on the current
//! buffer level, scheduling state, and quality-switch bookkeeping.

/// Default buffer target (seconds), matching
/// `Settings.streaming.buffer.bufferTimeAtTopQuality` in dash.js.
const DEFAULT_BUFFER_TARGET: f64 = 12.0;

// ---------------------------------------------------------------------------
// Types
// ---------------------------------------------------------------------------

/// Scheduling state machine.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ScheduleState {
    Stopped,
    Started,
    Scheduling,
}

impl Default for ScheduleState {
    fn default() -> Self {
        Self::Stopped
    }
}

// ---------------------------------------------------------------------------
// Controller
// ---------------------------------------------------------------------------

#[derive(Clone, Debug)]
pub struct ScheduleController {
    state: ScheduleState,
    media_type: String,
    stream_id: String,
    has_video_track: bool,
    time_to_load_delay_ms: f64,
    last_initialized_representation_id: Option<String>,
    init_segment_required: bool,
    switch_track: bool,
    should_check_playback_quality: bool,
    buffer_target: f64,
}

impl Default for ScheduleController {
    fn default() -> Self {
        Self {
            state: ScheduleState::Stopped,
            media_type: String::new(),
            stream_id: String::new(),
            has_video_track: false,
            time_to_load_delay_ms: 0.0,
            last_initialized_representation_id: None,
            init_segment_required: false,
            switch_track: false,
            should_check_playback_quality: false,
            buffer_target: DEFAULT_BUFFER_TARGET,
        }
    }
}

impl ScheduleController {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn initialize(
        &mut self,
        media_type: &str,
        stream_id: &str,
        has_video_track: bool,
    ) {
        self.media_type = media_type.to_owned();
        self.stream_id = stream_id.to_owned();
        self.has_video_track = has_video_track;
    }

    // -- scheduling state ---------------------------------------------------

    pub fn start_scheduling(&mut self) {
        self.state = ScheduleState::Started;
    }

    pub fn stop_scheduling(&mut self) {
        self.state = ScheduleState::Stopped;
    }

    pub fn is_started(&self) -> bool {
        self.state == ScheduleState::Started
    }

    pub fn get_state(&self) -> &ScheduleState {
        &self.state
    }

    // -- should_schedule ----------------------------------------------------

    /// Returns `true` when the scheduler is started **and** the buffer has
    /// room for at least one more fragment.
    pub fn should_schedule(
        &self,
        buffer_level: f64,
        fragment_duration: f64,
    ) -> bool {
        self.state == ScheduleState::Started
            && buffer_level + fragment_duration < self.buffer_target
    }

    // -- buffer target ------------------------------------------------------

    pub fn get_buffer_target(&self) -> f64 {
        self.buffer_target
    }

    pub fn set_buffer_target(&mut self, target: f64) {
        self.buffer_target = target;
    }

    // -- time-to-load delay -------------------------------------------------

    pub fn set_time_to_load_delay(&mut self, delay: f64) {
        self.time_to_load_delay_ms = delay;
    }

    pub fn get_time_to_load_delay(&self) -> f64 {
        self.time_to_load_delay_ms
    }

    // -- switch track -------------------------------------------------------

    pub fn set_switch_track(&mut self, value: bool) {
        self.switch_track = value;
    }

    pub fn get_switch_track(&self) -> bool {
        self.switch_track
    }

    // -- init segment required ----------------------------------------------

    pub fn set_init_segment_required(&mut self, value: bool) {
        self.init_segment_required = value;
    }

    pub fn get_init_segment_required(&self) -> bool {
        self.init_segment_required
    }

    // -- playback quality check ---------------------------------------------

    pub fn set_should_check_playback_quality(&mut self, value: bool) {
        self.should_check_playback_quality = value;
    }

    pub fn get_should_check_playback_quality(&self) -> bool {
        self.should_check_playback_quality
    }

    // -- last initialized representation ------------------------------------

    pub fn set_last_initialized_representation_id(&mut self, id: Option<String>) {
        self.last_initialized_representation_id = id;
    }

    pub fn get_last_initialized_representation_id(&self) -> Option<&str> {
        self.last_initialized_representation_id.as_deref()
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
    fn state_transitions() {
        let mut ctrl = ScheduleController::new();
        assert_eq!(*ctrl.get_state(), ScheduleState::Stopped);
        assert!(!ctrl.is_started());

        ctrl.start_scheduling();
        assert_eq!(*ctrl.get_state(), ScheduleState::Started);
        assert!(ctrl.is_started());

        ctrl.stop_scheduling();
        assert_eq!(*ctrl.get_state(), ScheduleState::Stopped);
    }

    #[test]
    fn should_schedule_logic() {
        let mut ctrl = ScheduleController::new();
        // Not started — never schedule.
        assert!(!ctrl.should_schedule(0.0, 4.0));

        ctrl.start_scheduling();
        // Buffer has room (0.0 + 4.0 < 12.0).
        assert!(ctrl.should_schedule(0.0, 4.0));
        // Buffer full (10.0 + 4.0 >= 12.0).
        assert!(!ctrl.should_schedule(10.0, 4.0));
    }

    #[test]
    fn getters_and_setters() {
        let mut ctrl = ScheduleController::new();
        ctrl.set_buffer_target(20.0);
        assert_eq!(ctrl.get_buffer_target(), 20.0);

        ctrl.set_time_to_load_delay(150.0);
        assert_eq!(ctrl.get_time_to_load_delay(), 150.0);

        ctrl.set_switch_track(true);
        assert!(ctrl.get_switch_track());

        ctrl.set_init_segment_required(true);
        assert!(ctrl.get_init_segment_required());

        ctrl.set_should_check_playback_quality(true);
        assert!(ctrl.get_should_check_playback_quality());

        ctrl.set_last_initialized_representation_id(Some("rep-1".into()));
        assert_eq!(
            ctrl.get_last_initialized_representation_id(),
            Some("rep-1")
        );
    }
}
