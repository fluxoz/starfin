//! Port of `dash.js/src/streaming/controllers/ClientDataReportingController.js`.
//!
//! Decides whether a particular service-location or adaptation-set should be
//! included in client-data reporting based on the service-description settings.

/// Settings sourced from the `ServiceDescription` element of the MPD.
#[derive(Clone, Debug, Default)]
pub struct ClientDataReportingSettings {
    /// When `Some`, only the listed service-location strings are reported.
    /// An empty vec means *all* locations are included.
    pub service_locations_array: Option<Vec<String>>,
    /// When `Some`, only the listed adaptation-set identifiers are reported.
    /// An empty vec means *all* adaptation sets are included.
    pub adaptation_sets_array: Option<Vec<String>>,
}

/// Controls whether requests and adaptation sets should contribute to client
/// data reporting.
#[derive(Clone, Debug, Default)]
pub struct ClientDataReportingController {
    settings: ClientDataReportingSettings,
}

impl ClientDataReportingController {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn set_settings(&mut self, settings: ClientDataReportingSettings) {
        self.settings = settings;
    }

    /// Returns `true` when the given `service_location` should be included in
    /// reporting for the specified `request_type`.
    ///
    /// Content-steering requests are always included.
    pub fn is_service_location_included(&self, request_type: &str, service_location: &str) -> bool {
        if request_type == "ContentSteering" {
            return true;
        }
        match &self.settings.service_locations_array {
            None => true,
            Some(arr) => arr.is_empty() || arr.iter().any(|s| s == service_location),
        }
    }

    /// Returns `true` when the given adaptation-set identifier should be
    /// included in reporting.
    pub fn is_adaptations_included(&self, adaptation_set: &str) -> bool {
        match &self.settings.adaptation_sets_array {
            None => true,
            Some(arr) => arr.is_empty() || arr.iter().any(|a| a == adaptation_set),
        }
    }

    pub fn reset(&mut self) {
        self.settings = ClientDataReportingSettings::default();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn content_steering_always_included() {
        let ctrl = ClientDataReportingController::new();
        assert!(ctrl.is_service_location_included("ContentSteering", "cdn-a"));
    }

    #[test]
    fn no_filter_includes_all() {
        let ctrl = ClientDataReportingController::new();
        assert!(ctrl.is_service_location_included("MediaSegment", "cdn-a"));
        assert!(ctrl.is_adaptations_included("video"));
    }

    #[test]
    fn empty_array_includes_all() {
        let mut ctrl = ClientDataReportingController::new();
        ctrl.set_settings(ClientDataReportingSettings {
            service_locations_array: Some(vec![]),
            adaptation_sets_array: Some(vec![]),
        });
        assert!(ctrl.is_service_location_included("MediaSegment", "cdn-a"));
        assert!(ctrl.is_adaptations_included("video"));
    }

    #[test]
    fn specific_filter() {
        let mut ctrl = ClientDataReportingController::new();
        ctrl.set_settings(ClientDataReportingSettings {
            service_locations_array: Some(vec!["cdn-a".to_string()]),
            adaptation_sets_array: Some(vec!["video".to_string()]),
        });
        assert!(ctrl.is_service_location_included("MediaSegment", "cdn-a"));
        assert!(!ctrl.is_service_location_included("MediaSegment", "cdn-b"));
        assert!(ctrl.is_adaptations_included("video"));
        assert!(!ctrl.is_adaptations_included("audio"));
    }
}
