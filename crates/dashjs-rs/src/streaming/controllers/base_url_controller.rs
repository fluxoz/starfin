//! Port of the corresponding dash.js streaming controller.
//!
//! Placeholder structure — full logic to be wired in future integration.

#[derive(Clone, Debug, Default)]
pub struct BaseUrlController {
    _initialized: bool,
}

impl BaseUrlController {
    pub fn new() -> Self { Self::default() }
    pub fn reset(&mut self) { self._initialized = false; }
}
