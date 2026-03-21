//! Port of the corresponding dash.js streaming controller.
//!
//! Placeholder structure — full logic to be wired in future integration.

#[derive(Clone, Debug, Default)]
pub struct MediaSourceController {
    _initialized: bool,
}

impl MediaSourceController {
    pub fn new() -> Self { Self::default() }
    pub fn reset(&mut self) { self._initialized = false; }
}
