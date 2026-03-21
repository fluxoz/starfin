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
