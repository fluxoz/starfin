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

/// Thumbnail controller stub.
#[derive(Clone, Debug, Default)]
pub struct ThumbnailController {
    _initialized: bool,
}

impl ThumbnailController {
    pub fn new() -> Self { Self::default() }
    pub fn reset(&mut self) { self._initialized = false; }
    /// Stub — returns None until wired to real segment data.
    pub fn get_thumbnail(&self, _time: f64) -> Option<Thumbnail> { None }
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
}
