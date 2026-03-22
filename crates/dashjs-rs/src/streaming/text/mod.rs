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

/// Text controller stub.
#[derive(Clone, Debug, Default)]
pub struct TextController {
    tracks: Vec<TextTrack>,
    enabled: bool,
}

impl TextController {
    pub fn new() -> Self { Self::default() }
    pub fn add_track(&mut self, track: TextTrack) { self.tracks.push(track); }
    pub fn get_tracks(&self) -> &[TextTrack] { &self.tracks }
    pub fn set_enabled(&mut self, enabled: bool) { self.enabled = enabled; }
    pub fn is_enabled(&self) -> bool { self.enabled }
    pub fn reset(&mut self) { self.tracks.clear(); self.enabled = false; }
}

/// Cue interval tree stub for efficient cue lookup.
#[derive(Clone, Debug, Default)]
pub struct CueIntervalTree {
    cues: Vec<Cue>,
}

impl CueIntervalTree {
    pub fn new() -> Self { Self::default() }
    pub fn insert(&mut self, cue: Cue) { self.cues.push(cue); }
    pub fn query(&self, time: f64) -> Vec<&Cue> {
        self.cues.iter().filter(|c| c.start <= time && time < c.end).collect()
    }
    pub fn clear(&mut self) { self.cues.clear(); }
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
}
