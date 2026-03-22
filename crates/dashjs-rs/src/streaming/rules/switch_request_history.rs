//! Port of `dash.js/src/streaming/rules/SwitchRequestHistory.js`.
//!
//! Tracks the history of quality switches to detect oscillation patterns.

use std::collections::HashMap;

/// Per-representation switch statistics.
#[derive(Clone, Debug, Default)]
pub struct SwitchEntry {
    pub drops: u32,
    pub no_drops: u32,
    pub drops_count: u32,
}

/// Tracks quality switch history per stream and media type.
#[derive(Clone, Debug, Default)]
pub struct SwitchRequestHistory {
    /// `stream_id` → `media_type` → `representation_id` → SwitchEntry
    history: HashMap<String, HashMap<String, HashMap<String, SwitchEntry>>>,
}

impl SwitchRequestHistory {
    pub fn new() -> Self {
        Self::default()
    }

    /// Record a switch request. `was_dropped` indicates that the quality was dropped
    /// from the previous request.
    pub fn push(&mut self, stream_id: &str, media_type: &str, representation_id: &str, was_dropped: bool) {
        let entry = self
            .history
            .entry(stream_id.to_string())
            .or_default()
            .entry(media_type.to_string())
            .or_default()
            .entry(representation_id.to_string())
            .or_insert_with(SwitchEntry::default);

        if was_dropped {
            entry.drops += 1;
            entry.drops_count += 1;
        } else {
            entry.no_drops += 1;
        }
    }

    /// Get switch requests for a given stream and media type.
    pub fn get_switch_requests(&self, stream_id: &str, media_type: &str) -> Option<&HashMap<String, SwitchEntry>> {
        self.history.get(stream_id)?.get(media_type)
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
    fn push_and_retrieve() {
        let mut h = SwitchRequestHistory::new();
        h.push("s1", "video", "rep0", true);
        h.push("s1", "video", "rep0", false);
        h.push("s1", "video", "rep0", true);

        let requests = h.get_switch_requests("s1", "video").unwrap();
        let entry = &requests["rep0"];
        assert_eq!(entry.drops, 2);
        assert_eq!(entry.no_drops, 1);
    }

    #[test]
    fn clear_for_stream() {
        let mut h = SwitchRequestHistory::new();
        h.push("s1", "video", "rep0", true);
        h.push("s2", "video", "rep0", true);
        h.clear_for_stream("s1");
        assert!(h.get_switch_requests("s1", "video").is_none());
        assert!(h.get_switch_requests("s2", "video").is_some());
    }

    #[test]
    fn reset_clears_all() {
        let mut h = SwitchRequestHistory::new();
        h.push("s1", "video", "rep0", false);
        h.reset();
        assert!(h.get_switch_requests("s1", "video").is_none());
    }
}
