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
