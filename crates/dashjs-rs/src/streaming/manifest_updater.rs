//! Port of `dash.js/src/streaming/ManifestUpdater.js`.
//!
//! Manages periodic MPD refresh for live streams, tracking update state and
//! the next scheduled refresh delay.

/// State of the manifest updater.
#[derive(Clone, Debug, PartialEq)]
pub enum UpdaterState {
    Stopped,
    Paused,
    Running,
}

impl Default for UpdaterState {
    fn default() -> Self {
        UpdaterState::Paused
    }
}

/// Tracks manifest refresh scheduling state.
///
/// The actual HTTP fetch and timer callbacks are handled by the host
/// application; this struct exposes the state needed to drive them.
#[derive(Clone, Debug, Default)]
pub struct ManifestUpdater {
    state: UpdaterState,
    is_updating: bool,
    /// Cached refresh delay in seconds derived from the last manifest update.
    refresh_delay: Option<f64>,
    manifest_url: String,
}

/// Constant from the JS implementation – caps `setTimeout` at ~24.8 days.
const MAX_REFRESH_DELAY_SECS: f64 = (0x7FFF_FFFFu32 as f64) / 1000.0;

impl ManifestUpdater {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn initialize(&mut self, manifest_url: &str) {
        self.manifest_url = manifest_url.to_string();
        self.reset_initial_settings();
    }

    fn reset_initial_settings(&mut self) {
        self.refresh_delay = None;
        self.is_updating = false;
        self.state = UpdaterState::Paused;
    }

    pub fn reset(&mut self) {
        self.reset_initial_settings();
        self.state = UpdaterState::Stopped;
        self.manifest_url.clear();
    }

    /// Called when playback starts; enables the refresh timer.
    pub fn on_playback_started(&mut self) {
        self.state = UpdaterState::Running;
    }

    /// Called when playback pauses; stops the refresh timer.
    pub fn on_playback_paused(&mut self, schedule_while_paused: bool) {
        if !schedule_while_paused {
            self.state = UpdaterState::Paused;
        }
    }

    /// Called when streams have been composed; clears the `is_updating` flag.
    pub fn on_streams_composed(&mut self) {
        self.is_updating = false;
    }

    /// Records the refresh delay reported by the manifest adapter and returns
    /// the clamped delay in milliseconds to pass to the next timer.
    pub fn set_refresh_delay(&mut self, delay_secs: f64) -> f64 {
        let clamped = if delay_secs * 1000.0 > MAX_REFRESH_DELAY_SECS * 1000.0 {
            MAX_REFRESH_DELAY_SECS
        } else {
            delay_secs
        };
        self.refresh_delay = Some(clamped);
        clamped * 1000.0
    }

    /// Returns the URL that should be fetched for the next manifest refresh.
    pub fn get_manifest_url(&self) -> &str {
        &self.manifest_url
    }

    /// Returns `true` when a manifest fetch is currently in flight.
    pub fn is_updating(&self) -> bool {
        self.is_updating
    }

    /// Marks that a manifest refresh fetch has started.
    pub fn begin_refresh(&mut self) {
        self.is_updating = true;
    }

    pub fn get_state(&self) -> &UpdaterState {
        &self.state
    }

    /// Returns the next refresh delay in milliseconds, if known.
    pub fn next_refresh_delay_ms(&self) -> Option<f64> {
        self.refresh_delay.map(|d| d * 1000.0)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn initial_state_is_paused() {
        let mut u = ManifestUpdater::new();
        u.initialize("http://example.com/manifest.mpd");
        assert_eq!(u.get_state(), &UpdaterState::Paused);
        assert!(!u.is_updating());
    }

    #[test]
    fn playback_started_sets_running() {
        let mut u = ManifestUpdater::new();
        u.initialize("http://example.com/manifest.mpd");
        u.on_playback_started();
        assert_eq!(u.get_state(), &UpdaterState::Running);
    }

    #[test]
    fn refresh_delay_clamped() {
        let mut u = ManifestUpdater::new();
        u.initialize("http://example.com/manifest.mpd");
        let ms = u.set_refresh_delay(f64::MAX);
        assert!(ms <= MAX_REFRESH_DELAY_SECS * 1000.0);
    }

    #[test]
    fn reset_clears_state() {
        let mut u = ManifestUpdater::new();
        u.initialize("http://example.com/manifest.mpd");
        u.on_playback_started();
        u.reset();
        assert_eq!(u.get_state(), &UpdaterState::Stopped);
        assert!(u.get_manifest_url().is_empty());
    }
}
