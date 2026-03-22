//! Port of `dash.js/src/streaming/metrics/`.
//!
//! Metrics collection and reporting infrastructure.

use serde::{Deserialize, Serialize};

// ---------------------------------------------------------------------------
// Metric value objects
// ---------------------------------------------------------------------------

/// Scheduling info metric.
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct SchedulingInfo {
    pub media_type: String,
    pub t: f64,
    pub quality: usize,
    pub state: String,
}

/// Buffer level metric.
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct BufferLevel {
    pub t: f64,
    pub level: f64,
}

/// HTTP request metric.
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct HttpRequestMetric {
    pub url: String,
    pub actual_url: Option<String>,
    pub media_type: String,
    pub response_code: u16,
    pub t_request: f64,
    pub t_response: f64,
    pub bytes_loaded: u64,
    pub interval: f64,
}

/// Representation switch event.
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct RepresentationSwitch {
    /// Wall-clock time of the switch.
    pub t: f64,
    /// Media time at switch.
    pub mt: f64,
    /// New representation id.
    pub to: String,
    /// Previous representation id.
    pub lto: String,
}

/// Playback rate metric for DVB reporting.
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct PlayList {
    pub start: f64,
    pub mstart: f64,
    pub start_type: String,
    pub trace: Vec<PlayListTrace>,
}

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct PlayListTrace {
    pub representationid: Option<String>,
    pub start: f64,
    pub mstart: f64,
    pub duration: f64,
    pub playback_speed: f64,
    pub stopreason: Option<String>,
}

// ---------------------------------------------------------------------------
// MetricsCollector
// ---------------------------------------------------------------------------

/// Collects all metric types for a single media type.
#[derive(Clone, Debug, Default)]
pub struct MetricsCollector {
    pub scheduling_info: Vec<SchedulingInfo>,
    pub buffer_levels: Vec<BufferLevel>,
    pub http_list: Vec<HttpRequestMetric>,
    pub play_lists: Vec<PlayList>,
    pub dropped_frames: u64,
    pub representation_switches: Vec<RepresentationSwitch>,
}

impl MetricsCollector {
    pub fn new() -> Self { Self::default() }

    pub fn add_scheduling_info(&mut self, info: SchedulingInfo) {
        self.scheduling_info.push(info);
    }

    pub fn add_buffer_level(&mut self, level: BufferLevel) {
        self.buffer_levels.push(level);
    }

    pub fn add_http_request(&mut self, req: HttpRequestMetric) {
        self.http_list.push(req);
    }

    pub fn add_representation_switch(&mut self, sw: RepresentationSwitch) {
        self.representation_switches.push(sw);
    }

    pub fn add_play_list(&mut self, pl: PlayList) {
        self.play_lists.push(pl);
    }

    /// Returns the most recent buffer level, if any.
    pub fn get_current_buffer_level(&self) -> Option<&BufferLevel> {
        self.buffer_levels.last()
    }

    /// Returns all recorded HTTP requests.
    pub fn get_http_requests(&self) -> &[HttpRequestMetric] {
        &self.http_list
    }

    pub fn reset(&mut self) {
        self.scheduling_info.clear();
        self.buffer_levels.clear();
        self.http_list.clear();
        self.play_lists.clear();
        self.dropped_frames = 0;
        self.representation_switches.clear();
    }
}

// ---------------------------------------------------------------------------
// MetricsReporting (kept from original)
// ---------------------------------------------------------------------------

/// Metrics reporting events stub.
pub struct MetricsReporting;
impl MetricsReporting {
    pub fn new() -> Self { Self }
}

#[cfg(test)]
mod tests {
    use super::*;

    // --- original tests ---

    #[test]
    fn scheduling_info_defaults() {
        let si = SchedulingInfo::default();
        assert!(si.media_type.is_empty());
        assert_eq!(si.t, 0.0);
        assert_eq!(si.quality, 0);
        assert!(si.state.is_empty());
    }

    #[test]
    fn scheduling_info_custom() {
        let si = SchedulingInfo { media_type: "video".into(), t: 12.5, quality: 2, state: "ready".into() };
        assert_eq!(si.media_type, "video");
        assert_eq!(si.t, 12.5);
    }

    #[test]
    fn buffer_level_defaults() {
        let bl = BufferLevel::default();
        assert_eq!(bl.t, 0.0);
        assert_eq!(bl.level, 0.0);
    }

    #[test]
    fn buffer_level_custom() {
        let bl = BufferLevel { t: 5.0, level: 30.5 };
        assert_eq!(bl.t, 5.0);
        assert_eq!(bl.level, 30.5);
    }

    #[test]
    fn playlist_defaults() {
        let pl = PlayList::default();
        assert_eq!(pl.start, 0.0);
        assert_eq!(pl.mstart, 0.0);
        assert!(pl.start_type.is_empty());
        assert!(pl.trace.is_empty());
    }

    #[test]
    fn playlist_trace_defaults() {
        let plt = PlayListTrace::default();
        assert!(plt.representationid.is_none());
        assert_eq!(plt.playback_speed, 0.0);
        assert!(plt.stopreason.is_none());
    }

    #[test]
    fn playlist_with_traces() {
        let pl = PlayList {
            start: 0.0,
            mstart: 0.0,
            start_type: "initial_playback".into(),
            trace: vec![
                PlayListTrace { representationid: Some("v1".into()), start: 0.0, mstart: 0.0, duration: 4.0, playback_speed: 1.0, stopreason: None },
            ],
        };
        assert_eq!(pl.trace.len(), 1);
        assert_eq!(pl.trace[0].duration, 4.0);
    }

    #[test]
    fn metrics_reporting_new() {
        let _mr = MetricsReporting::new();
    }

    // --- MetricsCollector tests ---

    #[test]
    fn collector_add_scheduling_info() {
        let mut mc = MetricsCollector::new();
        mc.add_scheduling_info(SchedulingInfo { media_type: "video".into(), t: 1.0, quality: 2, state: "executing".into() });
        assert_eq!(mc.scheduling_info.len(), 1);
    }

    #[test]
    fn collector_add_buffer_level() {
        let mut mc = MetricsCollector::new();
        mc.add_buffer_level(BufferLevel { t: 1.0, level: 10.0 });
        mc.add_buffer_level(BufferLevel { t: 2.0, level: 15.0 });
        assert_eq!(mc.get_current_buffer_level().unwrap().level, 15.0);
    }

    #[test]
    fn collector_get_current_buffer_level_empty() {
        let mc = MetricsCollector::new();
        assert!(mc.get_current_buffer_level().is_none());
    }

    #[test]
    fn collector_add_http_request() {
        let mut mc = MetricsCollector::new();
        mc.add_http_request(HttpRequestMetric { url: "http://a.com/seg.m4s".into(), response_code: 200, ..Default::default() });
        assert_eq!(mc.get_http_requests().len(), 1);
        assert_eq!(mc.get_http_requests()[0].response_code, 200);
    }

    #[test]
    fn collector_add_representation_switch() {
        let mut mc = MetricsCollector::new();
        mc.add_representation_switch(RepresentationSwitch { t: 10.0, mt: 10.0, to: "v2".into(), lto: "v1".into() });
        assert_eq!(mc.representation_switches.len(), 1);
        assert_eq!(mc.representation_switches[0].to, "v2");
    }

    #[test]
    fn collector_add_play_list() {
        let mut mc = MetricsCollector::new();
        mc.add_play_list(PlayList { start_type: "seek".into(), ..Default::default() });
        assert_eq!(mc.play_lists.len(), 1);
    }

    #[test]
    fn collector_dropped_frames() {
        let mut mc = MetricsCollector::new();
        mc.dropped_frames = 42;
        assert_eq!(mc.dropped_frames, 42);
    }

    #[test]
    fn collector_reset() {
        let mut mc = MetricsCollector::new();
        mc.add_scheduling_info(SchedulingInfo::default());
        mc.add_buffer_level(BufferLevel::default());
        mc.add_http_request(HttpRequestMetric::default());
        mc.add_play_list(PlayList::default());
        mc.add_representation_switch(RepresentationSwitch::default());
        mc.dropped_frames = 10;
        mc.reset();
        assert!(mc.scheduling_info.is_empty());
        assert!(mc.buffer_levels.is_empty());
        assert!(mc.http_list.is_empty());
        assert!(mc.play_lists.is_empty());
        assert!(mc.representation_switches.is_empty());
        assert_eq!(mc.dropped_frames, 0);
    }

    // --- RepresentationSwitch tests ---
    #[test]
    fn representation_switch_defaults() {
        let rs = RepresentationSwitch::default();
        assert_eq!(rs.t, 0.0);
        assert!(rs.to.is_empty());
    }

    // --- HttpRequestMetric tests ---
    #[test]
    fn http_request_metric_defaults() {
        let h = HttpRequestMetric::default();
        assert!(h.url.is_empty());
        assert_eq!(h.response_code, 0);
        assert_eq!(h.bytes_loaded, 0);
    }
}
