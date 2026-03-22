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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn offline_controller_new() {
        let ctrl = OfflineController::new();
        assert!(!ctrl._initialized);
    }

    #[test]
    fn offline_controller_reset() {
        let mut ctrl = OfflineController::new();
        ctrl._initialized = true;
        ctrl.reset();
        assert!(!ctrl._initialized);
    }

    #[test]
    fn offline_controller_reset_idempotent() {
        let mut ctrl = OfflineController::new();
        ctrl.reset();
        ctrl.reset();
        assert!(!ctrl._initialized);
    }

    #[test]
    fn offline_download_defaults() {
        let dl = OfflineDownload::default();
        assert!(dl.id.is_empty());
        assert!(dl.url.is_empty());
        assert_eq!(dl.progress, 0.0);
        assert_eq!(dl.status, OfflineStatus::Created);
    }

    #[test]
    fn offline_status_default_is_created() {
        let status = OfflineStatus::default();
        assert_eq!(status, OfflineStatus::Created);
    }

    #[test]
    fn offline_status_equality() {
        assert_eq!(OfflineStatus::Downloading, OfflineStatus::Downloading);
        assert_ne!(OfflineStatus::Downloading, OfflineStatus::Stopped);
        assert_ne!(OfflineStatus::Finished, OfflineStatus::Error);
    }

    #[test]
    fn offline_download_custom() {
        let dl = OfflineDownload {
            id: "dl-001".into(),
            url: "http://example.com/manifest.mpd".into(),
            progress: 0.5,
            status: OfflineStatus::Downloading,
        };
        assert_eq!(dl.id, "dl-001");
        assert_eq!(dl.progress, 0.5);
        assert_eq!(dl.status, OfflineStatus::Downloading);
    }

    #[test]
    fn offline_status_clone() {
        let s = OfflineStatus::Finished;
        let s2 = s.clone();
        assert_eq!(s, s2);
    }
}
