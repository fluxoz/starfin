//! Port of dash.js `MediaController`.
//!
//! Manages available tracks (audio, video, text) per media type and tracks
//! which track is currently active for each type.

use std::collections::HashMap;

/// Metadata describing a single media track.
#[derive(Clone, Debug)]
pub struct TrackInfo {
    pub id: String,
    pub media_type: String,
    pub lang: Option<String>,
    pub label: Option<String>,
    pub codec: Option<String>,
    pub bitrate: Option<u64>,
    pub is_default: bool,
}

const EMPTY_TRACKS: &[TrackInfo] = &[];

/// Controls track selection per media type.
#[derive(Clone, Debug, Default)]
pub struct MediaController {
    tracks: HashMap<String, Vec<TrackInfo>>,
    active_tracks: HashMap<String, String>,
}

impl MediaController {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn add_track(&mut self, track: TrackInfo) {
        self.tracks
            .entry(track.media_type.clone())
            .or_default()
            .push(track);
    }

    pub fn get_tracks_for_type(&self, media_type: &str) -> &[TrackInfo] {
        self.tracks.get(media_type).map_or(EMPTY_TRACKS, |v| v.as_slice())
    }

    /// Returns the default track for the given type, falling back to the first
    /// track if none is marked as default.
    pub fn get_initial_track(&self, media_type: &str) -> Option<&TrackInfo> {
        let tracks = self.tracks.get(media_type)?;
        tracks
            .iter()
            .find(|t| t.is_default)
            .or_else(|| tracks.first())
    }

    pub fn get_active_track(&self, media_type: &str) -> Option<&TrackInfo> {
        let track_id = self.active_tracks.get(media_type)?;
        let tracks = self.tracks.get(media_type)?;
        tracks.iter().find(|t| t.id == *track_id)
    }

    /// Switches the active track. Returns `true` if the track was found.
    pub fn switch_track(&mut self, media_type: &str, track_id: &str) -> bool {
        if let Some(tracks) = self.tracks.get(media_type) {
            if tracks.iter().any(|t| t.id == track_id) {
                self.active_tracks
                    .insert(media_type.to_string(), track_id.to_string());
                return true;
            }
        }
        false
    }

    /// Auto-selects the initial track for the given media type based on the
    /// `is_default` flag (or first track).
    pub fn select_initial_track(&mut self, media_type: &str) {
        if let Some(track) = self.get_initial_track(media_type) {
            let id = track.id.clone();
            self.active_tracks.insert(media_type.to_string(), id);
        }
    }

    pub fn reset(&mut self) {
        self.tracks.clear();
        self.active_tracks.clear();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn video_track(id: &str, is_default: bool) -> TrackInfo {
        TrackInfo {
            id: id.to_string(),
            media_type: "video".to_string(),
            lang: None,
            label: None,
            codec: Some("avc1".to_string()),
            bitrate: Some(5_000_000),
            is_default,
        }
    }

    fn audio_track(id: &str, lang: &str, is_default: bool) -> TrackInfo {
        TrackInfo {
            id: id.to_string(),
            media_type: "audio".to_string(),
            lang: Some(lang.to_string()),
            label: None,
            codec: Some("mp4a.40.2".to_string()),
            bitrate: Some(128_000),
            is_default,
        }
    }

    #[test]
    fn add_and_get_tracks() {
        let mut ctrl = MediaController::new();
        ctrl.add_track(video_track("v0", true));
        ctrl.add_track(audio_track("a0", "en", false));

        assert_eq!(ctrl.get_tracks_for_type("video").len(), 1);
        assert_eq!(ctrl.get_tracks_for_type("audio").len(), 1);
        assert!(ctrl.get_tracks_for_type("text").is_empty());
    }

    #[test]
    fn initial_track_prefers_default() {
        let mut ctrl = MediaController::new();
        ctrl.add_track(audio_track("a0", "en", false));
        ctrl.add_track(audio_track("a1", "fr", true));

        let initial = ctrl.get_initial_track("audio").unwrap();
        assert_eq!(initial.id, "a1");
    }

    #[test]
    fn initial_track_falls_back_to_first() {
        let mut ctrl = MediaController::new();
        ctrl.add_track(audio_track("a0", "en", false));
        ctrl.add_track(audio_track("a1", "fr", false));

        let initial = ctrl.get_initial_track("audio").unwrap();
        assert_eq!(initial.id, "a0");
    }

    #[test]
    fn switch_and_active_track() {
        let mut ctrl = MediaController::new();
        ctrl.add_track(audio_track("a0", "en", true));
        ctrl.add_track(audio_track("a1", "fr", false));

        ctrl.select_initial_track("audio");
        assert_eq!(ctrl.get_active_track("audio").unwrap().id, "a0");

        assert!(ctrl.switch_track("audio", "a1"));
        assert_eq!(ctrl.get_active_track("audio").unwrap().id, "a1");

        assert!(!ctrl.switch_track("audio", "nonexistent"));
    }

    #[test]
    fn reset_clears() {
        let mut ctrl = MediaController::new();
        ctrl.add_track(video_track("v0", true));
        ctrl.select_initial_track("video");
        ctrl.reset();

        assert!(ctrl.get_tracks_for_type("video").is_empty());
        assert!(ctrl.get_active_track("video").is_none());
    }
}
