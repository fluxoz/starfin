//! Port of `dash.js/src/streaming/MediaPlayer.js`.
//!
//! Placeholder — full logic to be wired in future integration.

#[derive(Clone, Debug, Default)]
pub struct MediaPlayer {
    _initialized: bool,
}

impl MediaPlayer {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn reset(&mut self) {
        self._initialized = false;
    }
}
