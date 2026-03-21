//! Port of `dash.js/src/offline/`.
//!
//! Offline playback support — stubbed for future implementation.
//! Structure mirrors: OfflineController, OfflineDownload, OfflineStream, OfflineStoreController.

/// Offline controller stub.
#[derive(Clone, Debug, Default)]
pub struct OfflineController { _initialized: bool }
impl OfflineController {
    pub fn new() -> Self { Self::default() }
    pub fn reset(&mut self) { self._initialized = false; }
}

/// Offline download request.
#[derive(Clone, Debug, Default)]
pub struct OfflineDownload {
    pub id: String,
    pub url: String,
    pub progress: f64,
    pub status: OfflineStatus,
}

/// Download status.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub enum OfflineStatus {
    #[default]
    Created,
    Downloading,
    Stopped,
    Finished,
    Error,
}
