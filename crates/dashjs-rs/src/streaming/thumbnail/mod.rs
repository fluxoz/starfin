//! Port of `dash.js/src/streaming/thumbnail/`.
//!
//! Thumbnail track controller stubs.

use serde::{Deserialize, Serialize};

/// Thumbnail metadata.
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct Thumbnail {
    pub url: String,
    pub x: u32,
    pub y: u32,
    pub width: u32,
    pub height: u32,
    pub time: f64,
}

/// Metadata describing a thumbnail sprite track.
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct ThumbnailTrackInfo {
    pub id: String,
    pub url: String,
    pub width: u32,
    pub height: u32,
    pub tile_width: u32,
    pub tile_height: u32,
    pub tiles_horizontal: u32,
    pub tiles_vertical: u32,
    pub segment_duration: f64,
    pub start_number: u32,
    pub bandwidth: u64,
}

/// Thumbnail controller — resolves presentation times to sprite tile
/// coordinates within image-based thumbnail tracks.
#[derive(Clone, Debug, Default)]
pub struct ThumbnailController {
    _initialized: bool,
    tracks: Vec<ThumbnailTrackInfo>,
    current_track_index: Option<usize>,
}

impl ThumbnailController {
    pub fn new() -> Self { Self::default() }

    /// Store the available thumbnail tracks and mark the controller ready.
    pub fn initialize(&mut self, tracks: Vec<ThumbnailTrackInfo>) {
        self.current_track_index = if tracks.is_empty() { None } else { Some(0) };
        self.tracks = tracks;
        self._initialized = true;
    }

    /// Reset controller to its initial (uninitialised) state.
    pub fn reset(&mut self) {
        self._initialized = false;
        self.tracks.clear();
        self.current_track_index = None;
    }

    /// Return a reference to the available thumbnail tracks.
    pub fn get_thumbnail_tracks(&self) -> &[ThumbnailTrackInfo] {
        &self.tracks
    }

    /// Select the active thumbnail track by index.  Returns `false` when
    /// the index is out of bounds (the selection is unchanged in that case).
    pub fn set_thumbnail_track(&mut self, index: usize) -> bool {
        if index < self.tracks.len() {
            self.current_track_index = Some(index);
            true
        } else {
            false
        }
    }

    /// Return the index of the currently selected thumbnail track.
    pub fn get_current_track_index(&self) -> Option<usize> {
        self.current_track_index
    }

    /// Resolve a presentation `time` (seconds) to the corresponding
    /// thumbnail sprite tile.  Returns `None` when the controller has not
    /// been initialised with tracks or the time is negative.
    pub fn get_thumbnail(&self, time: f64) -> Option<Thumbnail> {
        if !self._initialized || time < 0.0 {
            return None;
        }
        let idx = self.current_track_index?;
        let track = self.tracks.get(idx)?;
        if track.segment_duration <= 0.0
            || track.tiles_horizontal == 0
            || track.tiles_vertical == 0
        {
            return None;
        }

        let tile_index = (time / track.segment_duration).floor() as u32;
        let tiles_per_image = track.tiles_horizontal * track.tiles_vertical;
        let image_index = tile_index / tiles_per_image;
        let tile_in_image = tile_index % tiles_per_image;
        let x = (tile_in_image % track.tiles_horizontal) * track.tile_width;
        let y = (tile_in_image / track.tiles_horizontal) * track.tile_height;

        let number = image_index + track.start_number;
        let url = if track.url.contains("$Number$") {
            track.url.replace("$Number$", &number.to_string())
        } else {
            track.url.clone()
        };

        Some(Thumbnail {
            url,
            x,
            y,
            width: track.tile_width,
            height: track.tile_height,
            time,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn thumbnail_default_values() {
        let t = Thumbnail::default();
        assert!(t.url.is_empty());
        assert_eq!(t.x, 0);
        assert_eq!(t.y, 0);
        assert_eq!(t.width, 0);
        assert_eq!(t.height, 0);
        assert_eq!(t.time, 0.0);
    }

    #[test]
    fn thumbnail_custom_values() {
        let t = Thumbnail { url: "http://example.com/thumb.jpg".into(), x: 10, y: 20, width: 160, height: 90, time: 5.5 };
        assert_eq!(t.url, "http://example.com/thumb.jpg");
        assert_eq!(t.width, 160);
        assert_eq!(t.time, 5.5);
    }

    #[test]
    fn controller_new_and_reset() {
        let mut ctrl = ThumbnailController::new();
        ctrl.reset();
        // should not panic, controller remains usable
        assert!(ctrl.get_thumbnail(0.0).is_none());
    }

    #[test]
    fn controller_get_thumbnail_returns_none() {
        let ctrl = ThumbnailController::new();
        assert!(ctrl.get_thumbnail(0.0).is_none());
        assert!(ctrl.get_thumbnail(100.0).is_none());
        assert!(ctrl.get_thumbnail(-1.0).is_none());
    }

    #[test]
    fn thumbnail_clone() {
        let t = Thumbnail { url: "a.jpg".into(), x: 1, y: 2, width: 3, height: 4, time: 1.0 };
        let t2 = t.clone();
        assert_eq!(t.url, t2.url);
        assert_eq!(t.x, t2.x);
    }

    // --- ThumbnailTrackInfo tests ---

    #[test]
    fn track_info_default_values() {
        let ti = ThumbnailTrackInfo::default();
        assert!(ti.id.is_empty());
        assert!(ti.url.is_empty());
        assert_eq!(ti.width, 0);
        assert_eq!(ti.tile_width, 0);
        assert_eq!(ti.segment_duration, 0.0);
        assert_eq!(ti.start_number, 0);
        assert_eq!(ti.bandwidth, 0);
    }

    #[test]
    fn track_info_clone() {
        let ti = ThumbnailTrackInfo {
            id: "thumb_track".into(),
            url: "thumb_$Number$.jpg".into(),
            width: 3200,
            height: 1800,
            tile_width: 320,
            tile_height: 180,
            tiles_horizontal: 10,
            tiles_vertical: 10,
            segment_duration: 2.0,
            start_number: 1,
            bandwidth: 10000,
        };
        let ti2 = ti.clone();
        assert_eq!(ti.id, ti2.id);
        assert_eq!(ti.tiles_horizontal, ti2.tiles_horizontal);
    }

    // --- initialize tests ---

    fn sample_track() -> ThumbnailTrackInfo {
        ThumbnailTrackInfo {
            id: "t1".into(),
            url: "thumb_$Number$.jpg".into(),
            width: 3200,
            height: 1800,
            tile_width: 320,
            tile_height: 180,
            tiles_horizontal: 10,
            tiles_vertical: 10,
            segment_duration: 2.0,
            start_number: 1,
            bandwidth: 10000,
        }
    }

    #[test]
    fn initialize_with_tracks() {
        let mut ctrl = ThumbnailController::new();
        ctrl.initialize(vec![sample_track()]);
        assert_eq!(ctrl.get_current_track_index(), Some(0));
        assert_eq!(ctrl.get_thumbnail_tracks().len(), 1);
    }

    #[test]
    fn initialize_with_empty_tracks() {
        let mut ctrl = ThumbnailController::new();
        ctrl.initialize(vec![]);
        assert_eq!(ctrl.get_current_track_index(), None);
        assert!(ctrl.get_thumbnail_tracks().is_empty());
    }

    #[test]
    fn initialize_sets_initialized_flag() {
        let mut ctrl = ThumbnailController::new();
        ctrl.initialize(vec![sample_track()]);
        // should now return Some, proving _initialized is true
        assert!(ctrl.get_thumbnail(0.0).is_some());
    }

    // --- get_thumbnail_tracks tests ---

    #[test]
    fn get_thumbnail_tracks_empty_by_default() {
        let ctrl = ThumbnailController::new();
        assert!(ctrl.get_thumbnail_tracks().is_empty());
    }

    #[test]
    fn get_thumbnail_tracks_after_initialize() {
        let mut ctrl = ThumbnailController::new();
        let t1 = sample_track();
        let mut t2 = sample_track();
        t2.id = "t2".into();
        ctrl.initialize(vec![t1, t2]);
        assert_eq!(ctrl.get_thumbnail_tracks().len(), 2);
        assert_eq!(ctrl.get_thumbnail_tracks()[1].id, "t2");
    }

    // --- set_thumbnail_track tests ---

    #[test]
    fn set_thumbnail_track_valid() {
        let mut ctrl = ThumbnailController::new();
        let mut t2 = sample_track();
        t2.id = "t2".into();
        ctrl.initialize(vec![sample_track(), t2]);
        assert!(ctrl.set_thumbnail_track(1));
        assert_eq!(ctrl.get_current_track_index(), Some(1));
    }

    #[test]
    fn set_thumbnail_track_out_of_bounds() {
        let mut ctrl = ThumbnailController::new();
        ctrl.initialize(vec![sample_track()]);
        assert!(!ctrl.set_thumbnail_track(5));
        // index unchanged
        assert_eq!(ctrl.get_current_track_index(), Some(0));
    }

    #[test]
    fn set_thumbnail_track_empty_tracks() {
        let mut ctrl = ThumbnailController::new();
        ctrl.initialize(vec![]);
        assert!(!ctrl.set_thumbnail_track(0));
        assert_eq!(ctrl.get_current_track_index(), None);
    }

    // --- get_current_track_index tests ---

    #[test]
    fn get_current_track_index_default() {
        let ctrl = ThumbnailController::new();
        assert_eq!(ctrl.get_current_track_index(), None);
    }

    #[test]
    fn get_current_track_index_after_set() {
        let mut ctrl = ThumbnailController::new();
        let mut t2 = sample_track();
        t2.id = "t2".into();
        let mut t3 = sample_track();
        t3.id = "t3".into();
        ctrl.initialize(vec![sample_track(), t2, t3]);
        ctrl.set_thumbnail_track(2);
        assert_eq!(ctrl.get_current_track_index(), Some(2));
    }

    // --- reset tests ---

    #[test]
    fn reset_clears_tracks_and_index() {
        let mut ctrl = ThumbnailController::new();
        ctrl.initialize(vec![sample_track()]);
        assert!(ctrl.get_thumbnail(0.0).is_some());
        ctrl.reset();
        assert!(ctrl.get_thumbnail(0.0).is_none());
        assert!(ctrl.get_thumbnail_tracks().is_empty());
        assert_eq!(ctrl.get_current_track_index(), None);
    }

    #[test]
    fn reset_then_reinitialize() {
        let mut ctrl = ThumbnailController::new();
        ctrl.initialize(vec![sample_track()]);
        ctrl.reset();
        let mut t2 = sample_track();
        t2.id = "t2".into();
        ctrl.initialize(vec![t2]);
        assert_eq!(ctrl.get_thumbnail_tracks().len(), 1);
        assert_eq!(ctrl.get_thumbnail_tracks()[0].id, "t2");
    }

    // --- get_thumbnail tile calculation tests ---

    #[test]
    fn get_thumbnail_first_tile() {
        let mut ctrl = ThumbnailController::new();
        ctrl.initialize(vec![sample_track()]);
        let t = ctrl.get_thumbnail(0.0).unwrap();
        assert_eq!(t.url, "thumb_1.jpg");
        assert_eq!(t.x, 0);
        assert_eq!(t.y, 0);
        assert_eq!(t.width, 320);
        assert_eq!(t.height, 180);
        assert_eq!(t.time, 0.0);
    }

    #[test]
    fn get_thumbnail_second_tile() {
        let mut ctrl = ThumbnailController::new();
        ctrl.initialize(vec![sample_track()]);
        // time=2.0 with segment_duration=2.0 → tile_index=1
        let t = ctrl.get_thumbnail(2.0).unwrap();
        assert_eq!(t.x, 320); // second column
        assert_eq!(t.y, 0);   // still first row
    }

    #[test]
    fn get_thumbnail_second_row() {
        let mut ctrl = ThumbnailController::new();
        ctrl.initialize(vec![sample_track()]);
        // tile_index=10 → second row (tiles_horizontal=10)
        // time = 10 * 2.0 = 20.0
        let t = ctrl.get_thumbnail(20.0).unwrap();
        assert_eq!(t.x, 0);
        assert_eq!(t.y, 180); // second row
    }

    #[test]
    fn get_thumbnail_wraps_to_second_image() {
        let mut ctrl = ThumbnailController::new();
        ctrl.initialize(vec![sample_track()]);
        // tiles_per_image = 10 * 10 = 100
        // tile_index = 100 → image_index = 1, tile_in_image = 0
        // time = 100 * 2.0 = 200.0
        let t = ctrl.get_thumbnail(200.0).unwrap();
        assert_eq!(t.url, "thumb_2.jpg"); // start_number(1) + image_index(1)
        assert_eq!(t.x, 0);
        assert_eq!(t.y, 0);
    }

    #[test]
    fn get_thumbnail_mid_second_image() {
        let mut ctrl = ThumbnailController::new();
        ctrl.initialize(vec![sample_track()]);
        // tile_index = 105 → image_index=1, tile_in_image=5
        // x = (5 % 10) * 320 = 1600, y = (5 / 10) * 180 = 0
        let t = ctrl.get_thumbnail(210.0).unwrap();
        assert_eq!(t.url, "thumb_2.jpg");
        assert_eq!(t.x, 1600);
        assert_eq!(t.y, 0);
    }

    #[test]
    fn get_thumbnail_url_without_template() {
        let mut ctrl = ThumbnailController::new();
        let mut track = sample_track();
        track.url = "static_thumb.jpg".into();
        ctrl.initialize(vec![track]);
        let t = ctrl.get_thumbnail(0.0).unwrap();
        assert_eq!(t.url, "static_thumb.jpg");
    }

    #[test]
    fn get_thumbnail_negative_time_returns_none() {
        let mut ctrl = ThumbnailController::new();
        ctrl.initialize(vec![sample_track()]);
        assert!(ctrl.get_thumbnail(-1.0).is_none());
    }

    #[test]
    fn get_thumbnail_not_initialized_returns_none() {
        let ctrl = ThumbnailController::new();
        assert!(ctrl.get_thumbnail(5.0).is_none());
    }

    #[test]
    fn get_thumbnail_fractional_time() {
        let mut ctrl = ThumbnailController::new();
        ctrl.initialize(vec![sample_track()]);
        // time=3.5, segment_duration=2.0 → floor(3.5/2.0) = floor(1.75) = 1
        let t = ctrl.get_thumbnail(3.5).unwrap();
        assert_eq!(t.x, 320); // tile_index=1, col=1
        assert_eq!(t.y, 0);
        assert_eq!(t.time, 3.5);
    }

    #[test]
    fn get_thumbnail_uses_current_track() {
        let mut ctrl = ThumbnailController::new();
        let mut t2 = sample_track();
        t2.url = "alt_$Number$.jpg".into();
        t2.tile_width = 160;
        ctrl.initialize(vec![sample_track(), t2]);
        ctrl.set_thumbnail_track(1);
        let t = ctrl.get_thumbnail(0.0).unwrap();
        assert_eq!(t.url, "alt_1.jpg");
        assert_eq!(t.width, 160);
    }
}
