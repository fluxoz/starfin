//! Port of the dash.js `PlaybackController`.
//!
//! Tracks playback position, play/pause/seek state, and live-stream timing
//! for a single stream.

#[derive(Clone, Debug)]
pub struct PlaybackController {
    current_time: f64,
    duration: f64,
    is_seeking: bool,
    is_paused: bool,
    playback_rate: f64,
    is_dynamic: bool,
    live_delay: f64,
    original_live_delay: f64,
    availability_start_time: Option<f64>,
    playback_stalled: bool,
    stream_id: String,
    initialized: bool,
}

impl Default for PlaybackController {
    fn default() -> Self {
        Self {
            current_time: 0.0,
            duration: 0.0,
            is_seeking: false,
            is_paused: true,
            playback_rate: 1.0,
            is_dynamic: false,
            live_delay: 0.0,
            original_live_delay: 0.0,
            availability_start_time: None,
            playback_stalled: false,
            stream_id: String::new(),
            initialized: false,
        }
    }
}

impl PlaybackController {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn initialize(&mut self, stream_id: &str, is_dynamic: bool) {
        self.stream_id = stream_id.to_owned();
        self.is_dynamic = is_dynamic;
        self.initialized = true;
    }

    // -- time / duration ----------------------------------------------------

    pub fn get_time(&self) -> f64 {
        self.current_time
    }

    pub fn set_time(&mut self, time: f64) {
        self.current_time = time;
    }

    pub fn get_duration(&self) -> f64 {
        self.duration
    }

    pub fn set_duration(&mut self, duration: f64) {
        self.duration = duration;
    }

    // -- play state ---------------------------------------------------------

    /// `true` when the player is neither paused nor seeking.
    pub fn is_playing(&self) -> bool {
        !self.is_paused && !self.is_seeking
    }

    pub fn is_paused(&self) -> bool {
        self.is_paused
    }

    pub fn is_seeking(&self) -> bool {
        self.is_seeking
    }

    // -- playback rate ------------------------------------------------------

    pub fn get_playback_rate(&self) -> f64 {
        self.playback_rate
    }

    pub fn set_playback_rate(&mut self, rate: f64) {
        self.playback_rate = rate;
    }

    // -- play / pause / seek ------------------------------------------------

    pub fn play(&mut self) {
        self.is_paused = false;
    }

    pub fn pause(&mut self) {
        self.is_paused = true;
    }

    pub fn seek(&mut self, time: f64) {
        self.current_time = time;
        self.is_seeking = true;
    }

    pub fn on_seek_complete(&mut self) {
        self.is_seeking = false;
    }

    // -- stream end ---------------------------------------------------------

    pub fn get_time_to_stream_end(&self) -> f64 {
        self.duration - self.current_time
    }

    // -- live ---------------------------------------------------------------

    pub fn is_dynamic(&self) -> bool {
        self.is_dynamic
    }

    pub fn get_live_delay(&self) -> f64 {
        self.live_delay
    }

    pub fn set_live_delay(&mut self, delay: f64) {
        self.live_delay = delay;
    }

    pub fn get_original_live_delay(&self) -> f64 {
        self.original_live_delay
    }

    pub fn set_original_live_delay(&mut self, delay: f64) {
        self.original_live_delay = delay;
    }

    pub fn get_playback_stalled(&self) -> bool {
        self.playback_stalled
    }

    pub fn set_playback_stalled(&mut self, stalled: bool) {
        self.playback_stalled = stalled;
    }

    /// Computes the current live latency in seconds.
    ///
    /// Both `wall_clock_time` and `availability_start_time` are in
    /// **milliseconds** (matching the dash.js `Date.now()` convention).
    ///
    /// When `availability_start_time` has not been set (`None`), this falls
    /// back to `0.0` — matching the dash.js behaviour — which will yield a
    /// meaningless value. Callers should only rely on the result when the
    /// stream is dynamic **and** `availability_start_time` has been
    /// configured.
    pub fn get_current_live_latency(&self, wall_clock_time: f64) -> f64 {
        let ast = self.availability_start_time.unwrap_or(0.0);
        (wall_clock_time - ast) / 1000.0 - self.current_time
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
    fn play_and_pause() {
        let mut ctrl = PlaybackController::new();
        assert!(ctrl.is_paused());

        ctrl.play();
        assert!(!ctrl.is_paused());

        ctrl.pause();
        assert!(ctrl.is_paused());
    }

    #[test]
    fn seek_sets_time_and_flag() {
        let mut ctrl = PlaybackController::new();
        ctrl.seek(42.0);
        assert_eq!(ctrl.get_time(), 42.0);
        assert!(ctrl.is_seeking());

        ctrl.on_seek_complete();
        assert!(!ctrl.is_seeking());
    }

    #[test]
    fn time_tracking() {
        let mut ctrl = PlaybackController::new();
        ctrl.set_duration(100.0);
        ctrl.set_time(30.0);
        assert_eq!(ctrl.get_time_to_stream_end(), 70.0);
    }

    #[test]
    fn is_playing_requires_not_paused_and_not_seeking() {
        let mut ctrl = PlaybackController::new();
        assert!(!ctrl.is_playing()); // paused by default

        ctrl.play();
        assert!(ctrl.is_playing());

        ctrl.seek(10.0);
        assert!(!ctrl.is_playing()); // seeking

        ctrl.on_seek_complete();
        assert!(ctrl.is_playing());
    }

    #[test]
    fn live_latency_calculation() {
        let mut ctrl = PlaybackController::new();
        ctrl.availability_start_time = Some(1_000_000.0); // ms
        ctrl.set_time(50.0);
        // wall_clock = 1_060_000 ms  →  (1_060_000 - 1_000_000)/1000 - 50 = 10
        let latency = ctrl.get_current_live_latency(1_060_000.0);
        assert!((latency - 10.0).abs() < 1e-9);
    }
}
