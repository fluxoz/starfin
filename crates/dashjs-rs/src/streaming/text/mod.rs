//! Port of `dash.js/src/streaming/text/`.
//!
//! Text/subtitle track controller and cue infrastructure stubs.

use serde::{Deserialize, Serialize};

/// Text track representation.
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct TextTrack {
    pub id: Option<String>,
    pub index: usize,
    pub kind: String,
    pub label: Option<String>,
    pub lang: Option<String>,
    pub is_default: bool,
    pub is_embedded: bool,
    pub roles: Vec<String>,
}

/// Subtitle cue.
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct Cue {
    pub start: f64,
    pub end: f64,
    pub text: String,
    pub id: Option<String>,
}

/// Media info used to construct text tracks.
#[derive(Clone, Debug, Default)]
pub struct TextTrackMediaInfo {
    pub id: Option<String>,
    pub lang: Option<String>,
    pub label: Option<String>,
    pub kind: String,
    pub roles: Vec<String>,
    pub is_embedded: bool,
}

/// Text controller stub.
#[derive(Clone, Debug, Default)]
pub struct TextController {
    tracks: Vec<TextTrack>,
    enabled: bool,
    current_track_index: Option<usize>,
    text_default_enabled: bool,
    cue_tree: CueIntervalTree,
}

impl TextController {
    pub fn new() -> Self { Self::default() }
    pub fn add_track(&mut self, track: TextTrack) { self.tracks.push(track); }
    pub fn get_tracks(&self) -> &[TextTrack] { &self.tracks }
    pub fn set_enabled(&mut self, enabled: bool) { self.enabled = enabled; }
    pub fn is_enabled(&self) -> bool { self.enabled }
    pub fn reset(&mut self) {
        self.tracks.clear();
        self.enabled = false;
        self.current_track_index = None;
        self.text_default_enabled = false;
        self.cue_tree.clear();
    }

    /// Creates `TextTrack` entries from a media info struct.
    pub fn get_text_tracks_from_media_info(media_info: &TextTrackMediaInfo) -> Vec<TextTrack> {
        vec![TextTrack {
            id: media_info.id.clone(),
            index: 0,
            kind: media_info.kind.clone(),
            label: media_info.label.clone(),
            lang: media_info.lang.clone(),
            is_default: false,
            is_embedded: media_info.is_embedded,
            roles: media_info.roles.clone(),
        }]
    }

    /// Sets the current text track by index. Returns `false` if out of bounds.
    pub fn set_text_track(&mut self, index: usize) -> bool {
        if index < self.tracks.len() {
            self.current_track_index = Some(index);
            true
        } else {
            false
        }
    }

    /// Returns the index of the currently selected text track.
    pub fn get_current_text_track_index(&self) -> Option<usize> {
        self.current_track_index
    }

    /// Adds a cue to the internal cue tree if `track_index` is valid.
    pub fn add_cue_to_track(&mut self, track_index: usize, cue: Cue) -> bool {
        if track_index < self.tracks.len() {
            self.cue_tree.insert(cue);
            true
        } else {
            false
        }
    }

    /// Returns all cues whose interval contains `time`.
    pub fn get_cues_for_time(&self, time: f64) -> Vec<&Cue> {
        self.cue_tree.query(time)
    }

    /// Removes a track by index. Adjusts `current_track_index` accordingly.
    pub fn remove_track(&mut self, index: usize) -> bool {
        if index >= self.tracks.len() {
            return false;
        }
        self.tracks.remove(index);
        // Re-index remaining tracks so that TextTrack.index stays consistent.
        for (i, track) in self.tracks.iter_mut().enumerate() {
            track.index = i;
        }
        // Adjust the current track pointer.
        if let Some(cur) = self.current_track_index {
            if cur == index {
                self.current_track_index = None;
            } else if cur > index {
                self.current_track_index = Some(cur - 1);
            }
        }
        true
    }

    /// Returns whether text tracks are enabled by default.
    pub fn get_text_default_enabled(&self) -> bool {
        self.text_default_enabled
    }

    /// Sets whether text tracks should be enabled by default.
    pub fn set_text_default_enabled(&mut self, enabled: bool) {
        self.text_default_enabled = enabled;
    }
}

/// Cue interval tree stub for efficient cue lookup.
#[derive(Clone, Debug, Default)]
pub struct CueIntervalTree {
    cues: Vec<Cue>,
}

impl CueIntervalTree {
    pub fn new() -> Self { Self::default() }

    /// Inserts a cue while keeping `cues` sorted by `start` time.
    /// NaN start times are ordered to the end.
    pub fn insert(&mut self, cue: Cue) {
        let pos = self
            .cues
            .binary_search_by(|probe| {
                probe.start.partial_cmp(&cue.start).unwrap_or(std::cmp::Ordering::Less)
            })
            .unwrap_or_else(|e| e);
        self.cues.insert(pos, cue);
    }

    pub fn query(&self, time: f64) -> Vec<&Cue> {
        self.cues.iter().filter(|c| c.start <= time && time < c.end).collect()
    }
    pub fn clear(&mut self) { self.cues.clear(); }

    /// Removes all cues whose interval overlaps `[start, end)`.
    pub fn remove_cues_in_range(&mut self, start: f64, end: f64) {
        self.cues.retain(|c| {
            // A cue overlaps [start, end) when cue.start < end && cue.end > start.
            !(c.start < end && c.end > start)
        });
    }

    /// Returns the number of stored cues.
    pub fn get_cue_count(&self) -> usize {
        self.cues.len()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn text_controller_add_and_get_tracks() {
        let mut ctrl = TextController::new();
        assert!(ctrl.get_tracks().is_empty());
        ctrl.add_track(TextTrack { kind: "subtitles".into(), ..Default::default() });
        ctrl.add_track(TextTrack { kind: "captions".into(), ..Default::default() });
        assert_eq!(ctrl.get_tracks().len(), 2);
        assert_eq!(ctrl.get_tracks()[0].kind, "subtitles");
        assert_eq!(ctrl.get_tracks()[1].kind, "captions");
    }

    #[test]
    fn text_controller_enable_disable() {
        let mut ctrl = TextController::new();
        assert!(!ctrl.is_enabled());
        ctrl.set_enabled(true);
        assert!(ctrl.is_enabled());
        ctrl.set_enabled(false);
        assert!(!ctrl.is_enabled());
    }

    #[test]
    fn text_controller_reset() {
        let mut ctrl = TextController::new();
        ctrl.add_track(TextTrack::default());
        ctrl.set_enabled(true);
        ctrl.reset();
        assert!(ctrl.get_tracks().is_empty());
        assert!(!ctrl.is_enabled());
    }

    #[test]
    fn cue_interval_tree_insert_and_query() {
        let mut tree = CueIntervalTree::new();
        tree.insert(Cue { start: 0.0, end: 5.0, text: "first".into(), id: None });
        tree.insert(Cue { start: 3.0, end: 8.0, text: "second".into(), id: None });
        let hits = tree.query(4.0);
        assert_eq!(hits.len(), 2);
    }

    #[test]
    fn cue_interval_tree_query_no_match() {
        let mut tree = CueIntervalTree::new();
        tree.insert(Cue { start: 1.0, end: 2.0, text: "a".into(), id: None });
        assert!(tree.query(3.0).is_empty());
    }

    #[test]
    fn cue_interval_tree_boundary_exclusive_end() {
        let mut tree = CueIntervalTree::new();
        tree.insert(Cue { start: 0.0, end: 5.0, text: "cue".into(), id: None });
        // end is exclusive: query at exactly end should miss
        assert!(tree.query(5.0).is_empty());
        // start is inclusive
        assert_eq!(tree.query(0.0).len(), 1);
    }

    #[test]
    fn cue_interval_tree_clear() {
        let mut tree = CueIntervalTree::new();
        tree.insert(Cue { start: 0.0, end: 1.0, text: "x".into(), id: None });
        tree.clear();
        assert!(tree.query(0.5).is_empty());
    }

    // ──────────────────────────────────────────────
    // New tests for TextTrackMediaInfo / TextController
    // ──────────────────────────────────────────────

    #[test]
    fn get_text_tracks_from_media_info_basic() {
        let info = TextTrackMediaInfo {
            id: Some("t1".into()),
            lang: Some("en".into()),
            label: Some("English".into()),
            kind: "subtitles".into(),
            roles: vec!["main".into()],
            is_embedded: false,
        };
        let tracks = TextController::get_text_tracks_from_media_info(&info);
        assert_eq!(tracks.len(), 1);
        assert_eq!(tracks[0].id.as_deref(), Some("t1"));
        assert_eq!(tracks[0].lang.as_deref(), Some("en"));
        assert_eq!(tracks[0].label.as_deref(), Some("English"));
        assert_eq!(tracks[0].kind, "subtitles");
        assert_eq!(tracks[0].roles, vec!["main"]);
        assert!(!tracks[0].is_embedded);
    }

    #[test]
    fn get_text_tracks_from_media_info_defaults() {
        let info = TextTrackMediaInfo::default();
        let tracks = TextController::get_text_tracks_from_media_info(&info);
        assert_eq!(tracks.len(), 1);
        assert!(tracks[0].id.is_none());
        assert!(tracks[0].lang.is_none());
        assert!(tracks[0].label.is_none());
    }

    #[test]
    fn set_text_track_valid() {
        let mut ctrl = TextController::new();
        ctrl.add_track(TextTrack { kind: "subtitles".into(), ..Default::default() });
        ctrl.add_track(TextTrack { kind: "captions".into(), ..Default::default() });
        assert!(ctrl.set_text_track(1));
        assert_eq!(ctrl.get_current_text_track_index(), Some(1));
    }

    #[test]
    fn set_text_track_out_of_bounds() {
        let mut ctrl = TextController::new();
        ctrl.add_track(TextTrack::default());
        assert!(!ctrl.set_text_track(5));
        assert_eq!(ctrl.get_current_text_track_index(), None);
    }

    #[test]
    fn get_current_text_track_index_none_by_default() {
        let ctrl = TextController::new();
        assert_eq!(ctrl.get_current_text_track_index(), None);
    }

    #[test]
    fn get_current_text_track_index_after_set() {
        let mut ctrl = TextController::new();
        ctrl.add_track(TextTrack::default());
        ctrl.set_text_track(0);
        assert_eq!(ctrl.get_current_text_track_index(), Some(0));
    }

    #[test]
    fn add_cue_to_track_valid() {
        let mut ctrl = TextController::new();
        ctrl.add_track(TextTrack::default());
        let cue = Cue { start: 1.0, end: 3.0, text: "hello".into(), id: None };
        assert!(ctrl.add_cue_to_track(0, cue));
        assert_eq!(ctrl.get_cues_for_time(2.0).len(), 1);
    }

    #[test]
    fn add_cue_to_track_invalid_index() {
        let mut ctrl = TextController::new();
        let cue = Cue { start: 0.0, end: 1.0, text: "x".into(), id: None };
        assert!(!ctrl.add_cue_to_track(0, cue));
        assert!(ctrl.get_cues_for_time(0.5).is_empty());
    }

    #[test]
    fn get_cues_for_time_multiple() {
        let mut ctrl = TextController::new();
        ctrl.add_track(TextTrack::default());
        ctrl.add_cue_to_track(0, Cue { start: 0.0, end: 5.0, text: "a".into(), id: None });
        ctrl.add_cue_to_track(0, Cue { start: 3.0, end: 7.0, text: "b".into(), id: None });
        assert_eq!(ctrl.get_cues_for_time(4.0).len(), 2);
    }

    #[test]
    fn get_cues_for_time_no_match() {
        let mut ctrl = TextController::new();
        ctrl.add_track(TextTrack::default());
        ctrl.add_cue_to_track(0, Cue { start: 1.0, end: 2.0, text: "a".into(), id: None });
        assert!(ctrl.get_cues_for_time(3.0).is_empty());
    }

    #[test]
    fn remove_track_valid() {
        let mut ctrl = TextController::new();
        ctrl.add_track(TextTrack { kind: "subtitles".into(), index: 0, ..Default::default() });
        ctrl.add_track(TextTrack { kind: "captions".into(), index: 1, ..Default::default() });
        assert!(ctrl.remove_track(0));
        assert_eq!(ctrl.get_tracks().len(), 1);
        assert_eq!(ctrl.get_tracks()[0].kind, "captions");
        // Index should be re-numbered.
        assert_eq!(ctrl.get_tracks()[0].index, 0);
    }

    #[test]
    fn remove_track_out_of_bounds() {
        let mut ctrl = TextController::new();
        ctrl.add_track(TextTrack::default());
        assert!(!ctrl.remove_track(10));
        assert_eq!(ctrl.get_tracks().len(), 1);
    }

    #[test]
    fn remove_track_adjusts_current_index_none() {
        let mut ctrl = TextController::new();
        ctrl.add_track(TextTrack::default());
        ctrl.add_track(TextTrack::default());
        ctrl.set_text_track(0);
        ctrl.remove_track(0);
        // Removed the selected track → index becomes None.
        assert_eq!(ctrl.get_current_text_track_index(), None);
    }

    #[test]
    fn remove_track_adjusts_current_index_shifted() {
        let mut ctrl = TextController::new();
        ctrl.add_track(TextTrack::default());
        ctrl.add_track(TextTrack::default());
        ctrl.add_track(TextTrack::default());
        ctrl.set_text_track(2);
        ctrl.remove_track(0);
        // Current was 2, removed index 0 → new current should be 1.
        assert_eq!(ctrl.get_current_text_track_index(), Some(1));
    }

    #[test]
    fn text_default_enabled_defaults_false() {
        let ctrl = TextController::new();
        assert!(!ctrl.get_text_default_enabled());
    }

    #[test]
    fn text_default_enabled_set_and_get() {
        let mut ctrl = TextController::new();
        ctrl.set_text_default_enabled(true);
        assert!(ctrl.get_text_default_enabled());
        ctrl.set_text_default_enabled(false);
        assert!(!ctrl.get_text_default_enabled());
    }

    #[test]
    fn reset_clears_new_fields() {
        let mut ctrl = TextController::new();
        ctrl.add_track(TextTrack::default());
        ctrl.set_text_track(0);
        ctrl.set_text_default_enabled(true);
        ctrl.add_cue_to_track(0, Cue { start: 0.0, end: 1.0, text: "c".into(), id: None });
        ctrl.reset();
        assert_eq!(ctrl.get_current_text_track_index(), None);
        assert!(!ctrl.get_text_default_enabled());
        assert!(ctrl.get_cues_for_time(0.5).is_empty());
    }

    // ──────────────────────────────────────────────
    // New tests for CueIntervalTree
    // ──────────────────────────────────────────────

    #[test]
    fn cue_interval_tree_insert_maintains_sorted_order() {
        let mut tree = CueIntervalTree::new();
        tree.insert(Cue { start: 5.0, end: 6.0, text: "c".into(), id: None });
        tree.insert(Cue { start: 1.0, end: 2.0, text: "a".into(), id: None });
        tree.insert(Cue { start: 3.0, end: 4.0, text: "b".into(), id: None });
        let starts: Vec<f64> = tree.cues.iter().map(|c| c.start).collect();
        assert_eq!(starts, vec![1.0, 3.0, 5.0]);
    }

    #[test]
    fn cue_interval_tree_insert_duplicate_start_times() {
        let mut tree = CueIntervalTree::new();
        tree.insert(Cue { start: 2.0, end: 3.0, text: "x".into(), id: None });
        tree.insert(Cue { start: 2.0, end: 4.0, text: "y".into(), id: None });
        assert_eq!(tree.get_cue_count(), 2);
        assert_eq!(tree.query(2.5).len(), 2);
    }

    #[test]
    fn remove_cues_in_range_removes_overlapping() {
        let mut tree = CueIntervalTree::new();
        tree.insert(Cue { start: 0.0, end: 3.0, text: "a".into(), id: None });
        tree.insert(Cue { start: 2.0, end: 5.0, text: "b".into(), id: None });
        tree.insert(Cue { start: 6.0, end: 8.0, text: "c".into(), id: None });
        tree.remove_cues_in_range(1.0, 4.0);
        // "a" overlaps (0<4 && 3>1) → removed
        // "b" overlaps (2<4 && 5>1) → removed
        // "c" does not overlap (6<4 is false) → kept
        assert_eq!(tree.get_cue_count(), 1);
        assert_eq!(tree.cues[0].text, "c");
    }

    #[test]
    fn remove_cues_in_range_no_overlap() {
        let mut tree = CueIntervalTree::new();
        tree.insert(Cue { start: 0.0, end: 1.0, text: "a".into(), id: None });
        tree.insert(Cue { start: 5.0, end: 6.0, text: "b".into(), id: None });
        tree.remove_cues_in_range(2.0, 4.0);
        assert_eq!(tree.get_cue_count(), 2);
    }

    #[test]
    fn remove_cues_in_range_all() {
        let mut tree = CueIntervalTree::new();
        tree.insert(Cue { start: 1.0, end: 3.0, text: "a".into(), id: None });
        tree.insert(Cue { start: 2.0, end: 4.0, text: "b".into(), id: None });
        tree.remove_cues_in_range(0.0, 10.0);
        assert_eq!(tree.get_cue_count(), 0);
    }

    #[test]
    fn get_cue_count_empty() {
        let tree = CueIntervalTree::new();
        assert_eq!(tree.get_cue_count(), 0);
    }

    #[test]
    fn get_cue_count_after_inserts_and_clear() {
        let mut tree = CueIntervalTree::new();
        tree.insert(Cue { start: 0.0, end: 1.0, text: "a".into(), id: None });
        tree.insert(Cue { start: 1.0, end: 2.0, text: "b".into(), id: None });
        assert_eq!(tree.get_cue_count(), 2);
        tree.clear();
        assert_eq!(tree.get_cue_count(), 0);
    }
}
