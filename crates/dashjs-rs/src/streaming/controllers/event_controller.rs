//! Port of dash.js `EventController`.
//!
//! Manages in-band and MPD-level DASH events, tracking which events are active
//! at a given presentation time and which have already been processed.

use std::collections::HashSet;

/// A single DASH event.
#[derive(Clone, Debug)]
pub struct DashEvent {
    pub id: String,
    pub event_stream: String,
    pub start_time: f64,
    pub duration: f64,
    pub message_data: String,
    pub scheme_id_uri: String,
    pub value: Option<String>,
    pub presentation_time: f64,
}

/// Tracks DASH events and their processed state.
#[derive(Clone, Debug, Default)]
pub struct EventController {
    events: Vec<DashEvent>,
    processed_event_ids: HashSet<String>,
    initialized: bool,
}

impl EventController {
    pub fn new() -> Self {
        Self {
            initialized: true,
            ..Self::default()
        }
    }

    pub fn add_event(&mut self, event: DashEvent) {
        self.events.push(event);
    }

    /// Returns references to all events whose time window covers `time` and
    /// that have not yet been marked as processed.
    pub fn get_events_for_time(&self, time: f64) -> Vec<&DashEvent> {
        self.events
            .iter()
            .filter(|e| {
                e.start_time <= time
                    && time < e.start_time + e.duration
                    && !self.processed_event_ids.contains(&e.id)
            })
            .collect()
    }

    pub fn mark_processed(&mut self, event_id: &str) {
        self.processed_event_ids.insert(event_id.to_string());
    }

    pub fn is_processed(&self, event_id: &str) -> bool {
        self.processed_event_ids.contains(event_id)
    }

    pub fn get_all_events(&self) -> &[DashEvent] {
        &self.events
    }

    pub fn clear_events(&mut self) {
        self.events.clear();
        self.processed_event_ids.clear();
    }

    pub fn reset(&mut self) {
        self.events.clear();
        self.processed_event_ids.clear();
        self.initialized = false;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn make_event(id: &str, start: f64, duration: f64) -> DashEvent {
        DashEvent {
            id: id.to_string(),
            event_stream: "urn:test".to_string(),
            start_time: start,
            duration,
            message_data: "payload".to_string(),
            scheme_id_uri: "urn:test".to_string(),
            value: None,
            presentation_time: start,
        }
    }

    #[test]
    fn add_and_retrieve_by_time() {
        let mut ctrl = EventController::new();
        ctrl.add_event(make_event("e1", 0.0, 5.0));
        ctrl.add_event(make_event("e2", 3.0, 5.0));
        ctrl.add_event(make_event("e3", 10.0, 2.0));

        let at_4 = ctrl.get_events_for_time(4.0);
        assert_eq!(at_4.len(), 2);

        let at_0 = ctrl.get_events_for_time(0.0);
        assert_eq!(at_0.len(), 1);
        assert_eq!(at_0[0].id, "e1");

        let at_10 = ctrl.get_events_for_time(10.0);
        assert_eq!(at_10.len(), 1);
        assert_eq!(at_10[0].id, "e3");

        let at_12 = ctrl.get_events_for_time(12.0);
        assert!(at_12.is_empty());
    }

    #[test]
    fn mark_processed_excludes_from_results() {
        let mut ctrl = EventController::new();
        ctrl.add_event(make_event("e1", 0.0, 10.0));
        ctrl.add_event(make_event("e2", 0.0, 10.0));

        ctrl.mark_processed("e1");
        assert!(ctrl.is_processed("e1"));
        assert!(!ctrl.is_processed("e2"));

        let at_5 = ctrl.get_events_for_time(5.0);
        assert_eq!(at_5.len(), 1);
        assert_eq!(at_5[0].id, "e2");
    }

    #[test]
    fn time_window_filtering() {
        let mut ctrl = EventController::new();
        ctrl.add_event(make_event("e1", 5.0, 3.0));

        assert!(ctrl.get_events_for_time(4.9).is_empty());
        assert_eq!(ctrl.get_events_for_time(5.0).len(), 1);
        assert_eq!(ctrl.get_events_for_time(7.9).len(), 1);
        assert!(ctrl.get_events_for_time(8.0).is_empty());
    }

    #[test]
    fn reset_clears_all() {
        let mut ctrl = EventController::new();
        ctrl.add_event(make_event("e1", 0.0, 5.0));
        ctrl.mark_processed("e1");
        ctrl.reset();

        assert!(ctrl.get_all_events().is_empty());
        assert!(!ctrl.is_processed("e1"));
    }
}
