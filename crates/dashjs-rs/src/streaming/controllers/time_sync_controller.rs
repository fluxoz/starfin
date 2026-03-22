//! Port of dash.js `TimeSyncController`.
//!
//! Tracks the clock offset between a server-provided UTC time and the local
//! client clock, allowing conversion in both directions.

/// Manages time synchronisation between client and server clocks.
#[derive(Clone, Debug)]
pub struct TimeSyncController {
    time_offset_ms: f64,
    is_synced: bool,
    last_sync_time: Option<f64>,
    sync_source: Option<String>,
}

impl Default for TimeSyncController {
    fn default() -> Self {
        Self {
            time_offset_ms: 0.0,
            is_synced: false,
            last_sync_time: None,
            sync_source: None,
        }
    }
}

impl TimeSyncController {
    pub fn new() -> Self {
        Self::default()
    }

    /// Records a measured offset and marks the controller as synchronised.
    pub fn set_time_offset(&mut self, offset_ms: f64, source: &str) {
        self.time_offset_ms = offset_ms;
        self.is_synced = true;
        self.last_sync_time = Some(offset_ms);
        self.sync_source = Some(source.to_string());
    }

    pub fn get_time_offset(&self) -> f64 {
        self.time_offset_ms
    }

    /// Converts a server timestamp to the equivalent client timestamp.
    pub fn get_client_time_from_server(&self, server_time_ms: f64) -> f64 {
        server_time_ms - self.time_offset_ms
    }

    /// Converts a client timestamp to the equivalent server timestamp.
    pub fn get_server_time_from_client(&self, client_time_ms: f64) -> f64 {
        client_time_ms + self.time_offset_ms
    }

    pub fn is_synced(&self) -> bool {
        self.is_synced
    }

    pub fn get_sync_source(&self) -> Option<&str> {
        self.sync_source.as_deref()
    }

    pub fn reset(&mut self) {
        self.time_offset_ms = 0.0;
        self.is_synced = false;
        self.last_sync_time = None;
        self.sync_source = None;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_is_not_synced() {
        let ctrl = TimeSyncController::new();
        assert!(!ctrl.is_synced());
        assert_eq!(ctrl.get_time_offset(), 0.0);
        assert!(ctrl.get_sync_source().is_none());
    }

    #[test]
    fn set_offset_marks_synced() {
        let mut ctrl = TimeSyncController::new();
        ctrl.set_time_offset(500.0, "urn:mpeg:dash:utc:http-xsdate:2014");
        assert!(ctrl.is_synced());
        assert_eq!(ctrl.get_time_offset(), 500.0);
        assert_eq!(
            ctrl.get_sync_source().unwrap(),
            "urn:mpeg:dash:utc:http-xsdate:2014"
        );
    }

    #[test]
    fn server_to_client_conversion() {
        let mut ctrl = TimeSyncController::new();
        ctrl.set_time_offset(1000.0, "http-head");
        assert_eq!(ctrl.get_client_time_from_server(5000.0), 4000.0);
    }

    #[test]
    fn client_to_server_conversion() {
        let mut ctrl = TimeSyncController::new();
        ctrl.set_time_offset(1000.0, "http-head");
        assert_eq!(ctrl.get_server_time_from_client(4000.0), 5000.0);
    }

    #[test]
    fn reset_clears_state() {
        let mut ctrl = TimeSyncController::new();
        ctrl.set_time_offset(500.0, "http-head");
        ctrl.reset();

        assert!(!ctrl.is_synced());
        assert_eq!(ctrl.get_time_offset(), 0.0);
        assert!(ctrl.get_sync_source().is_none());
    }
}
