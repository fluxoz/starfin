//! Port of `dash.js/src/streaming/rules/DroppedFramesHistory.js`.
//!
//! Tracks dropped video frames per representation quality.

use std::collections::HashMap;

/// Frame statistics for a single representation.
#[derive(Clone, Debug, Default)]
pub struct FrameHistoryEntry {
    pub dropped_video_frames: u32,
    pub total_video_frames: u32,
}

/// Tracks dropped frames history per stream and representation.
#[derive(Clone, Debug, Default)]
pub struct DroppedFramesHistory {
    /// `stream_id` → `representation_id` → FrameHistoryEntry
    history: HashMap<String, HashMap<String, FrameHistoryEntry>>,
}

impl DroppedFramesHistory {
    pub fn new() -> Self {
        Self::default()
    }

    /// Record frame data for a given representation.
    pub fn push(
        &mut self,
        stream_id: &str,
        representation_id: &str,
        dropped_video_frames: u32,
        total_video_frames: u32,
    ) {
        let entry = self
            .history
            .entry(stream_id.to_string())
            .or_default()
            .entry(representation_id.to_string())
            .or_insert_with(FrameHistoryEntry::default);

        entry.dropped_video_frames = dropped_video_frames;
        entry.total_video_frames = total_video_frames;
    }

    /// Get the frame history for a given stream.
    pub fn get_frame_history(&self, stream_id: &str) -> Option<&HashMap<String, FrameHistoryEntry>> {
        self.history.get(stream_id)
    }

    /// Clear history for a specific stream.
    pub fn clear_for_stream(&mut self, stream_id: &str) {
        self.history.remove(stream_id);
    }

    /// Reset all history.
    pub fn reset(&mut self) {
        self.history.clear();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn push_and_get() {
        let mut h = DroppedFramesHistory::new();
        h.push("s1", "rep0", 5, 100);
        h.push("s1", "rep1", 10, 200);

        let data = h.get_frame_history("s1").unwrap();
        assert_eq!(data["rep0"].dropped_video_frames, 5);
        assert_eq!(data["rep0"].total_video_frames, 100);
        assert_eq!(data["rep1"].dropped_video_frames, 10);
    }

    #[test]
    fn update_overwrites() {
        let mut h = DroppedFramesHistory::new();
        h.push("s1", "rep0", 5, 100);
        h.push("s1", "rep0", 15, 300);

        let data = h.get_frame_history("s1").unwrap();
        assert_eq!(data["rep0"].dropped_video_frames, 15);
        assert_eq!(data["rep0"].total_video_frames, 300);
    }

    #[test]
    fn clear_and_reset() {
        let mut h = DroppedFramesHistory::new();
        h.push("s1", "rep0", 5, 100);
        h.push("s2", "rep0", 10, 200);
        h.clear_for_stream("s1");
        assert!(h.get_frame_history("s1").is_none());
        assert!(h.get_frame_history("s2").is_some());
        h.reset();
        assert!(h.get_frame_history("s2").is_none());
    }
}
