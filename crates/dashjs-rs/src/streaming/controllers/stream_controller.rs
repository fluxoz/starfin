//! Port of the dash.js `StreamController`.
//!
//! Manages the set of streams (periods) in a presentation: tracks which
//! stream is active, supports switching, and exposes error state.

// ---------------------------------------------------------------------------
// Types
// ---------------------------------------------------------------------------

/// Metadata for a single stream (period) in the presentation.
#[derive(Clone, Debug)]
pub struct StreamInfo {
    pub id: String,
    pub index: usize,
    pub start: f64,
    pub duration: f64,
    pub is_last: bool,
}

// ---------------------------------------------------------------------------
// Controller
// ---------------------------------------------------------------------------

#[derive(Clone, Debug)]
pub struct StreamController {
    active_stream_id: Option<String>,
    streams: Vec<StreamInfo>,
    has_media_or_init_error: bool,
    initialized: bool,
    auto_play: bool,
}

impl Default for StreamController {
    fn default() -> Self {
        Self {
            active_stream_id: None,
            streams: Vec::new(),
            has_media_or_init_error: false,
            initialized: false,
            auto_play: false,
        }
    }
}

impl StreamController {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn initialize(&mut self, auto_play: bool) {
        self.initialized = true;
        self.auto_play = auto_play;
    }

    pub fn add_stream(&mut self, info: StreamInfo) {
        if self.active_stream_id.is_none() {
            self.active_stream_id = Some(info.id.clone());
        }
        self.streams.push(info);
    }

    pub fn get_active_stream_info(&self) -> Option<&StreamInfo> {
        let id = self.active_stream_id.as_deref()?;
        self.streams.iter().find(|s| s.id == id)
    }

    pub fn get_active_stream_id(&self) -> Option<&str> {
        self.active_stream_id.as_deref()
    }

    /// Switch to the stream identified by `stream_id`.
    ///
    /// Returns `true` if the stream was found and activated; `false`
    /// otherwise.
    pub fn switch_stream(&mut self, stream_id: &str) -> bool {
        if self.streams.iter().any(|s| s.id == stream_id) {
            self.active_stream_id = Some(stream_id.to_owned());
            true
        } else {
            false
        }
    }

    pub fn get_stream_by_id(&self, id: &str) -> Option<&StreamInfo> {
        self.streams.iter().find(|s| s.id == id)
    }

    pub fn get_streams(&self) -> &[StreamInfo] {
        &self.streams
    }

    pub fn has_media_or_init_error(&self) -> bool {
        self.has_media_or_init_error
    }

    pub fn set_media_or_init_error(&mut self, error: bool) {
        self.has_media_or_init_error = error;
    }

    pub fn is_auto_play(&self) -> bool {
        self.auto_play
    }

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

    fn make_stream(id: &str, idx: usize, start: f64, dur: f64, last: bool) -> StreamInfo {
        StreamInfo {
            id: id.to_owned(),
            index: idx,
            start,
            duration: dur,
            is_last: last,
        }
    }

    #[test]
    fn add_and_get_active_stream() {
        let mut ctrl = StreamController::new();
        ctrl.initialize(true);
        ctrl.add_stream(make_stream("s0", 0, 0.0, 30.0, false));
        ctrl.add_stream(make_stream("s1", 1, 30.0, 30.0, true));

        assert_eq!(ctrl.get_active_stream_id(), Some("s0"));
        let info = ctrl.get_active_stream_info().unwrap();
        assert_eq!(info.index, 0);
    }

    #[test]
    fn switch_stream() {
        let mut ctrl = StreamController::new();
        ctrl.add_stream(make_stream("s0", 0, 0.0, 30.0, false));
        ctrl.add_stream(make_stream("s1", 1, 30.0, 30.0, true));

        assert!(ctrl.switch_stream("s1"));
        assert_eq!(ctrl.get_active_stream_id(), Some("s1"));

        assert!(!ctrl.switch_stream("nonexistent"));
        assert_eq!(ctrl.get_active_stream_id(), Some("s1"));
    }

    #[test]
    fn get_stream_by_id() {
        let mut ctrl = StreamController::new();
        ctrl.add_stream(make_stream("s0", 0, 0.0, 30.0, false));
        let info = ctrl.get_stream_by_id("s0").unwrap();
        assert!((info.duration - 30.0).abs() < f64::EPSILON);
        assert!(ctrl.get_stream_by_id("nope").is_none());
    }

    #[test]
    fn error_tracking() {
        let mut ctrl = StreamController::new();
        assert!(!ctrl.has_media_or_init_error());
        ctrl.set_media_or_init_error(true);
        assert!(ctrl.has_media_or_init_error());
    }

    #[test]
    fn reset_clears_state() {
        let mut ctrl = StreamController::new();
        ctrl.initialize(true);
        ctrl.add_stream(make_stream("s0", 0, 0.0, 30.0, false));
        ctrl.reset();
        assert!(ctrl.get_active_stream_id().is_none());
        assert!(ctrl.get_streams().is_empty());
        assert!(!ctrl.is_auto_play());
    }
}
