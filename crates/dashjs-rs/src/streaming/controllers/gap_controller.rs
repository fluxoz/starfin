//! Port of the corresponding dash.js streaming controller.
//!
//! Placeholder structure — full logic to be wired in future integration.

#[derive(Clone, Debug, Default)]
pub struct GapController {
    _initialized: bool,
}

impl GapController {
    pub fn new() -> Self { Self::default() }
    pub fn reset(&mut self) { self._initialized = false; }
}
