//! Port of `dash.js/src/offline/`.
//!
//! Offline playback support with download management, progress tracking,
//! and key-value store abstraction for persisted content.
//! Structure mirrors: OfflineController, OfflineDownload, OfflineStream, OfflineStoreController.

use std::collections::HashMap;

/// Individual track in an offline download.
#[derive(Clone, Debug, Default)]
pub struct OfflineTrack {
    pub media_type: String,
    pub quality: usize,
    pub segments_total: u64,
    pub segments_downloaded: u64,
}

/// Offline stream info representing a downloaded manifest and its tracks.
#[derive(Clone, Debug, Default)]
pub struct OfflineStream {
    pub id: String,
    pub url: String,
    pub manifest: Option<String>,
    pub tracks: Vec<OfflineTrack>,
    pub total_size_bytes: u64,
    pub downloaded_size_bytes: u64,
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

/// Offline download request.
#[derive(Clone, Debug, Default)]
pub struct OfflineDownload {
    pub id: String,
    pub url: String,
    pub progress: f64,
    pub status: OfflineStatus,
}

/// Offline store controller — key-value store abstraction for IndexedDB/local storage.
///
/// Port of `dash.js/src/offline/OfflineStoreController.js`.
#[derive(Clone, Debug, Default)]
pub struct OfflineStoreController {
    store: HashMap<String, Vec<u8>>,
}

impl OfflineStoreController {
    pub fn new() -> Self { Self::default() }

    pub fn save(&mut self, key: String, data: Vec<u8>) {
        self.store.insert(key, data);
    }

    pub fn get(&self, key: &str) -> Option<&[u8]> {
        self.store.get(key).map(|v| v.as_slice())
    }

    pub fn remove(&mut self, key: &str) -> bool {
        self.store.remove(key).is_some()
    }

    pub fn clear(&mut self) {
        self.store.clear();
    }

    pub fn get_all_keys(&self) -> Vec<String> {
        self.store.keys().cloned().collect()
    }

    pub fn contains(&self, key: &str) -> bool {
        self.store.contains_key(key)
    }

    pub fn len(&self) -> usize {
        self.store.len()
    }

    pub fn is_empty(&self) -> bool {
        self.store.is_empty()
    }
}

/// Offline controller managing download lifecycle.
///
/// Port of `dash.js/src/offline/controllers/OfflineController.js`.
#[derive(Clone, Debug, Default)]
pub struct OfflineController {
    _initialized: bool,
    downloads: Vec<OfflineDownload>,
    next_id: u64,
}

impl OfflineController {
    pub fn new() -> Self { Self::default() }

    pub fn initialize(&mut self) {
        self._initialized = true;
    }

    pub fn is_initialized(&self) -> bool { self._initialized }

    /// Create a new download for the given URL. Returns the download ID.
    pub fn create_download(&mut self, url: &str) -> String {
        let id = format!("dl-{}", self.next_id);
        self.next_id += 1;
        self.downloads.push(OfflineDownload {
            id: id.clone(),
            url: url.to_string(),
            progress: 0.0,
            status: OfflineStatus::Created,
        });
        id
    }

    /// Start downloading the given download ID.
    pub fn start_download(&mut self, id: &str) -> Result<(), String> {
        match self.downloads.iter_mut().find(|d| d.id == id) {
            Some(dl) => match dl.status {
                OfflineStatus::Created | OfflineStatus::Stopped => {
                    dl.status = OfflineStatus::Downloading;
                    Ok(())
                }
                _ => Err(format!("Cannot start download in {:?} state", dl.status)),
            },
            None => Err(format!("Download {} not found", id)),
        }
    }

    /// Stop a running download.
    pub fn stop_download(&mut self, id: &str) -> Result<(), String> {
        match self.downloads.iter_mut().find(|d| d.id == id) {
            Some(dl) => match dl.status {
                OfflineStatus::Downloading => {
                    dl.status = OfflineStatus::Stopped;
                    Ok(())
                }
                _ => Err(format!("Cannot stop download in {:?} state", dl.status)),
            },
            None => Err(format!("Download {} not found", id)),
        }
    }

    /// Remove a download by ID.
    pub fn remove_download(&mut self, id: &str) -> Result<(), String> {
        let len_before = self.downloads.len();
        self.downloads.retain(|d| d.id != id);
        if self.downloads.len() < len_before {
            Ok(())
        } else {
            Err(format!("Download {} not found", id))
        }
    }

    /// Get download by ID.
    pub fn get_download(&self, id: &str) -> Option<&OfflineDownload> {
        self.downloads.iter().find(|d| d.id == id)
    }

    /// Get all downloads.
    pub fn get_all_downloads(&self) -> &[OfflineDownload] {
        &self.downloads
    }

    /// Get download progress (0.0 - 1.0).
    pub fn get_download_progress(&self, id: &str) -> f64 {
        self.downloads.iter().find(|d| d.id == id)
            .map(|d| d.progress).unwrap_or(0.0)
    }

    /// Check if a download is complete.
    pub fn is_download_complete(&self, id: &str) -> bool {
        self.downloads.iter().find(|d| d.id == id)
            .map(|d| d.status == OfflineStatus::Finished).unwrap_or(false)
    }

    /// Update download progress. Used internally during download.
    pub fn update_progress(&mut self, id: &str, progress: f64) {
        if let Some(dl) = self.downloads.iter_mut().find(|d| d.id == id) {
            dl.progress = progress.clamp(0.0, 1.0);
            if dl.progress >= 1.0 {
                dl.status = OfflineStatus::Finished;
            }
        }
    }

    pub fn reset(&mut self) {
        self._initialized = false;
        self.downloads.clear();
        self.next_id = 0;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // OfflineController tests
    #[test]
    fn offline_controller_lifecycle() {
        let mut ctrl = OfflineController::new();
        assert!(!ctrl.is_initialized());
        ctrl.initialize();
        assert!(ctrl.is_initialized());
        ctrl.reset();
        assert!(!ctrl.is_initialized());
    }

    #[test]
    fn offline_controller_reset_idempotent() {
        let mut ctrl = OfflineController::new();
        ctrl.reset();
        ctrl.reset();
        assert!(!ctrl.is_initialized());
    }

    #[test]
    fn create_download() {
        let mut ctrl = OfflineController::new();
        let id = ctrl.create_download("http://example.com/manifest.mpd");
        assert_eq!(id, "dl-0");
        assert_eq!(ctrl.get_all_downloads().len(), 1);
        let dl = ctrl.get_download(&id).unwrap();
        assert_eq!(dl.url, "http://example.com/manifest.mpd");
        assert_eq!(dl.status, OfflineStatus::Created);
    }

    #[test]
    fn create_multiple_downloads() {
        let mut ctrl = OfflineController::new();
        let id1 = ctrl.create_download("url1");
        let id2 = ctrl.create_download("url2");
        assert_ne!(id1, id2);
        assert_eq!(ctrl.get_all_downloads().len(), 2);
    }

    #[test]
    fn start_and_stop_download() {
        let mut ctrl = OfflineController::new();
        let id = ctrl.create_download("url");
        assert!(ctrl.start_download(&id).is_ok());
        assert_eq!(ctrl.get_download(&id).unwrap().status, OfflineStatus::Downloading);
        assert!(ctrl.stop_download(&id).is_ok());
        assert_eq!(ctrl.get_download(&id).unwrap().status, OfflineStatus::Stopped);
    }

    #[test]
    fn start_stopped_download() {
        let mut ctrl = OfflineController::new();
        let id = ctrl.create_download("url");
        ctrl.start_download(&id).unwrap();
        ctrl.stop_download(&id).unwrap();
        assert!(ctrl.start_download(&id).is_ok());
        assert_eq!(ctrl.get_download(&id).unwrap().status, OfflineStatus::Downloading);
    }

    #[test]
    fn cannot_stop_non_downloading() {
        let mut ctrl = OfflineController::new();
        let id = ctrl.create_download("url");
        assert!(ctrl.stop_download(&id).is_err());
    }

    #[test]
    fn remove_download() {
        let mut ctrl = OfflineController::new();
        let id = ctrl.create_download("url");
        assert!(ctrl.remove_download(&id).is_ok());
        assert!(ctrl.get_download(&id).is_none());
        assert!(ctrl.remove_download(&id).is_err());
    }

    #[test]
    fn download_progress() {
        let mut ctrl = OfflineController::new();
        let id = ctrl.create_download("url");
        assert_eq!(ctrl.get_download_progress(&id), 0.0);
        ctrl.update_progress(&id, 0.5);
        assert_eq!(ctrl.get_download_progress(&id), 0.5);
        assert!(!ctrl.is_download_complete(&id));
        ctrl.update_progress(&id, 1.0);
        assert!(ctrl.is_download_complete(&id));
        assert_eq!(ctrl.get_download(&id).unwrap().status, OfflineStatus::Finished);
    }

    #[test]
    fn progress_clamps() {
        let mut ctrl = OfflineController::new();
        let id = ctrl.create_download("url");
        ctrl.update_progress(&id, 2.0);
        assert_eq!(ctrl.get_download_progress(&id), 1.0);
        ctrl.update_progress(&id, -1.0);
        // After setting to 1.0, download is Finished; further update still clamps
    }

    #[test]
    fn nonexistent_download_operations() {
        let mut ctrl = OfflineController::new();
        assert!(ctrl.start_download("nope").is_err());
        assert!(ctrl.stop_download("nope").is_err());
        assert!(ctrl.get_download("nope").is_none());
        assert_eq!(ctrl.get_download_progress("nope"), 0.0);
        assert!(!ctrl.is_download_complete("nope"));
    }

    #[test]
    fn reset_clears_downloads() {
        let mut ctrl = OfflineController::new();
        ctrl.initialize();
        ctrl.create_download("url1");
        ctrl.create_download("url2");
        ctrl.reset();
        assert!(ctrl.get_all_downloads().is_empty());
        assert!(!ctrl.is_initialized());
    }

    // OfflineDownload tests
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

    // OfflineStoreController tests
    #[test]
    fn store_controller_save_get() {
        let mut store = OfflineStoreController::new();
        assert!(store.is_empty());
        store.save("key1".into(), vec![1, 2, 3]);
        assert_eq!(store.get("key1"), Some(&[1u8, 2, 3][..]));
        assert!(store.get("key2").is_none());
        assert_eq!(store.len(), 1);
    }

    #[test]
    fn store_controller_remove() {
        let mut store = OfflineStoreController::new();
        store.save("key1".into(), vec![1]);
        assert!(store.remove("key1"));
        assert!(!store.remove("key1"));
        assert!(store.is_empty());
    }

    #[test]
    fn store_controller_clear() {
        let mut store = OfflineStoreController::new();
        store.save("k1".into(), vec![1]);
        store.save("k2".into(), vec![2]);
        store.clear();
        assert!(store.is_empty());
    }

    #[test]
    fn store_controller_get_all_keys() {
        let mut store = OfflineStoreController::new();
        store.save("a".into(), vec![]);
        store.save("b".into(), vec![]);
        let mut keys = store.get_all_keys();
        keys.sort();
        assert_eq!(keys, vec!["a".to_string(), "b".to_string()]);
    }

    #[test]
    fn store_controller_contains() {
        let mut store = OfflineStoreController::new();
        assert!(!store.contains("key"));
        store.save("key".into(), vec![]);
        assert!(store.contains("key"));
    }

    // OfflineStream / OfflineTrack tests
    #[test]
    fn offline_stream_defaults() {
        let s = OfflineStream::default();
        assert!(s.id.is_empty());
        assert!(s.manifest.is_none());
        assert!(s.tracks.is_empty());
        assert_eq!(s.total_size_bytes, 0);
    }

    #[test]
    fn offline_track_defaults() {
        let t = OfflineTrack::default();
        assert!(t.media_type.is_empty());
        assert_eq!(t.quality, 0);
        assert_eq!(t.segments_total, 0);
        assert_eq!(t.segments_downloaded, 0);
    }
}
