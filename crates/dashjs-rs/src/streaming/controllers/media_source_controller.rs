//! Port of dash.js `MediaSourceController`.
//!
//! Manages the lifecycle of a MediaSource-like object, tracking state
//! transitions and the set of active source buffers.

/// Represents the lifecycle state of a media source.
#[derive(Clone, Debug, PartialEq)]
pub enum MediaSourceState {
    Created,
    Open,
    Closed,
    Ended,
}

impl Default for MediaSourceState {
    fn default() -> Self {
        Self::Created
    }
}

/// Metadata for a single source buffer attached to the media source.
#[derive(Clone, Debug)]
pub struct SourceBufferInfo {
    pub id: String,
    pub mime_type: String,
    pub codec: String,
}

/// Controls MediaSource state transitions and source buffer management.
#[derive(Clone, Debug)]
pub struct MediaSourceController {
    state: MediaSourceState,
    source_buffers: Vec<SourceBufferInfo>,
    duration: Option<f64>,
    next_buffer_id: u64,
}

impl Default for MediaSourceController {
    fn default() -> Self {
        Self {
            state: MediaSourceState::Created,
            source_buffers: Vec::new(),
            duration: None,
            next_buffer_id: 0,
        }
    }
}

impl MediaSourceController {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn open(&mut self) {
        self.state = MediaSourceState::Open;
    }

    pub fn close(&mut self) {
        self.state = MediaSourceState::Closed;
    }

    pub fn end_of_stream(&mut self) {
        self.state = MediaSourceState::Ended;
    }

    pub fn get_state(&self) -> &MediaSourceState {
        &self.state
    }

    pub fn is_open(&self) -> bool {
        self.state == MediaSourceState::Open
    }

    /// Adds a source buffer. The media source must be in the `Open` state and
    /// the `mime_type` must not already have a buffer registered.
    pub fn add_source_buffer(&mut self, mime_type: &str, codec: &str) -> Result<String, String> {
        if self.state != MediaSourceState::Open {
            return Err("MediaSource is not open".to_string());
        }
        if self.source_buffers.iter().any(|sb| sb.mime_type == mime_type) {
            return Err(format!("Source buffer for mime_type '{}' already exists", mime_type));
        }
        let id = format!("sb_{}", self.next_buffer_id);
        self.next_buffer_id += 1;
        self.source_buffers.push(SourceBufferInfo {
            id: id.clone(),
            mime_type: mime_type.to_string(),
            codec: codec.to_string(),
        });
        Ok(id)
    }

    pub fn remove_source_buffer(&mut self, id: &str) -> bool {
        let len_before = self.source_buffers.len();
        self.source_buffers.retain(|sb| sb.id != id);
        self.source_buffers.len() < len_before
    }

    pub fn get_source_buffers(&self) -> &[SourceBufferInfo] {
        &self.source_buffers
    }

    pub fn set_duration(&mut self, duration: f64) {
        self.duration = Some(duration);
    }

    pub fn get_duration(&self) -> Option<f64> {
        self.duration
    }

    pub fn reset(&mut self) {
        self.state = MediaSourceState::Created;
        self.source_buffers.clear();
        self.duration = None;
        self.next_buffer_id = 0;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn state_transitions() {
        let mut ctrl = MediaSourceController::new();
        assert_eq!(*ctrl.get_state(), MediaSourceState::Created);
        assert!(!ctrl.is_open());

        ctrl.open();
        assert_eq!(*ctrl.get_state(), MediaSourceState::Open);
        assert!(ctrl.is_open());

        ctrl.end_of_stream();
        assert_eq!(*ctrl.get_state(), MediaSourceState::Ended);

        ctrl.close();
        assert_eq!(*ctrl.get_state(), MediaSourceState::Closed);
    }

    #[test]
    fn add_and_remove_source_buffers() {
        let mut ctrl = MediaSourceController::new();
        ctrl.open();

        let id = ctrl.add_source_buffer("video/mp4", "avc1.42E01E").unwrap();
        assert_eq!(ctrl.get_source_buffers().len(), 1);
        assert_eq!(ctrl.get_source_buffers()[0].id, id);

        let id2 = ctrl.add_source_buffer("audio/mp4", "mp4a.40.2").unwrap();
        assert_eq!(ctrl.get_source_buffers().len(), 2);

        assert!(ctrl.remove_source_buffer(&id));
        assert_eq!(ctrl.get_source_buffers().len(), 1);
        assert_eq!(ctrl.get_source_buffers()[0].id, id2);

        assert!(!ctrl.remove_source_buffer("nonexistent"));
    }

    #[test]
    fn cannot_add_when_closed() {
        let mut ctrl = MediaSourceController::new();
        ctrl.close();
        assert!(ctrl.add_source_buffer("video/mp4", "avc1").is_err());
    }

    #[test]
    fn duplicate_mime_type_rejected() {
        let mut ctrl = MediaSourceController::new();
        ctrl.open();
        ctrl.add_source_buffer("video/mp4", "avc1").unwrap();
        assert!(ctrl.add_source_buffer("video/mp4", "avc1").is_err());
    }

    #[test]
    fn reset_clears_state() {
        let mut ctrl = MediaSourceController::new();
        ctrl.open();
        ctrl.add_source_buffer("video/mp4", "avc1").unwrap();
        ctrl.set_duration(120.0);
        ctrl.reset();

        assert_eq!(*ctrl.get_state(), MediaSourceState::Created);
        assert!(ctrl.get_source_buffers().is_empty());
        assert!(ctrl.get_duration().is_none());
    }
}
