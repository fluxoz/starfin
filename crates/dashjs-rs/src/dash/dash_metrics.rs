//! Port of `dash.js/src/dash/DashMetrics.js`.
//!
//! Tracks metrics for DASH playback.

use std::collections::HashMap;

/// Buffer state values.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum BufferState {
    /// Buffer is filling (not yet at target level).
    Filling,
    /// Buffer has reached stable playback level.
    Steady,
}

impl std::fmt::Display for BufferState {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            BufferState::Filling => write!(f, "bufferStalled"),
            BufferState::Steady => write!(f, "bufferLoaded"),
        }
    }
}

/// A single buffer level measurement.
#[derive(Clone, Debug)]
pub struct BufferLevelEntry {
    pub timestamp_ms: f64,
    pub level_ms: f64,
}

/// A single buffer state measurement.
#[derive(Clone, Debug)]
pub struct BufferStateEntry {
    pub timestamp_ms: f64,
    pub state: BufferState,
    pub target: f64,
}

/// HTTP request metrics.
#[derive(Clone, Debug)]
pub struct HttpRequestMetric {
    pub url: String,
    pub actual_url: Option<String>,
    pub request_type: String,
    pub media_type: String,
    pub response_code: Option<u16>,
    pub latency_ms: Option<f64>,
    pub download_time_ms: Option<f64>,
    pub bytes_loaded: Option<u64>,
    pub bandwidth_bps: Option<f64>,
    pub request_start_time_ms: Option<f64>,
    pub first_byte_time_ms: Option<f64>,
    pub request_end_time_ms: Option<f64>,
}

/// A representation switch event.
#[derive(Clone, Debug)]
pub struct RepresentationSwitch {
    pub timestamp_ms: f64,
    pub media_time: f64,
    pub to_id: String,
    pub media_type: String,
}

/// Scheduling info entry.
#[derive(Clone, Debug)]
pub struct SchedulingInfo {
    pub timestamp_ms: f64,
    pub media_type: String,
    pub request_type: String,
    pub start_time: Option<f64>,
    pub duration: Option<f64>,
    pub bandwidth: Option<u64>,
    pub range: Option<String>,
    pub state: String,
}

/// Dropped frames info.
#[derive(Clone, Debug)]
pub struct DroppedFrames {
    pub timestamp_ms: f64,
    pub dropped_frames: u64,
}

/// DVR info for time-shift buffer.
#[derive(Clone, Debug)]
pub struct DvrInfo {
    pub media_type: String,
    pub timestamp_ms: f64,
    pub start: f64,
    pub end: f64,
    pub range: Option<(f64, f64)>,
}

/// Metrics storage for a single media type.
#[derive(Clone, Debug, Default)]
pub struct MediaTypeMetrics {
    pub buffer_level: Vec<BufferLevelEntry>,
    pub buffer_state: Vec<BufferStateEntry>,
    pub http_list: Vec<HttpRequestMetric>,
    pub representation_switch: Vec<RepresentationSwitch>,
    pub scheduling_info: Vec<SchedulingInfo>,
    pub dvr_info: Vec<DvrInfo>,
    pub dropped_frames: Vec<DroppedFrames>,
}

/// DashMetrics collects and provides access to DASH playback metrics.
#[derive(Clone, Debug, Default)]
pub struct DashMetrics {
    metrics: HashMap<String, MediaTypeMetrics>,
}

impl DashMetrics {
    pub fn new() -> Self {
        Self {
            metrics: HashMap::new(),
        }
    }

    /// Get or create metrics for a media type.
    fn get_or_create(&mut self, media_type: &str) -> &mut MediaTypeMetrics {
        self.metrics
            .entry(media_type.to_string())
            .or_default()
    }

    /// Get metrics for a media type.
    pub fn get_metrics_for(&self, media_type: &str) -> Option<&MediaTypeMetrics> {
        self.metrics.get(media_type)
    }

    /// Get the current buffer level for a media type (in seconds).
    pub fn get_current_buffer_level(&self, media_type: &str) -> f64 {
        self.metrics
            .get(media_type)
            .and_then(|m| m.buffer_level.last())
            .map_or(0.0, |entry| entry.level_ms / 1000.0)
    }

    /// Get the current buffer state for a media type.
    pub fn get_current_buffer_state(&self, media_type: &str) -> Option<&BufferStateEntry> {
        self.metrics
            .get(media_type)
            .and_then(|m| m.buffer_state.last())
    }

    /// Add a buffer level measurement.
    pub fn add_buffer_level(&mut self, media_type: &str, timestamp_ms: f64, level_ms: f64) {
        self.get_or_create(media_type)
            .buffer_level
            .push(BufferLevelEntry {
                timestamp_ms,
                level_ms,
            });
    }

    /// Add a buffer state measurement.
    pub fn add_buffer_state(&mut self, media_type: &str, state: BufferState, target: f64) {
        let now = 0.0; // In real impl would use current time
        self.get_or_create(media_type)
            .buffer_state
            .push(BufferStateEntry {
                timestamp_ms: now,
                state,
                target,
            });
    }

    /// Get the current representation switch for a media type.
    pub fn get_current_representation_switch(
        &self,
        media_type: &str,
    ) -> Option<&RepresentationSwitch> {
        self.metrics
            .get(media_type)
            .and_then(|m| m.representation_switch.last())
    }

    /// Add a representation switch event.
    pub fn add_representation_switch(
        &mut self,
        media_type: &str,
        timestamp_ms: f64,
        media_time: f64,
        to_id: &str,
    ) {
        self.get_or_create(media_type)
            .representation_switch
            .push(RepresentationSwitch {
                timestamp_ms,
                media_time,
                to_id: to_id.to_string(),
                media_type: media_type.to_string(),
            });
    }

    /// Get the current HTTP request for a media type.
    pub fn get_current_http_request(&self, media_type: &str) -> Option<&HttpRequestMetric> {
        self.metrics.get(media_type).and_then(|m| {
            m.http_list
                .iter()
                .rev()
                .find(|r| r.response_code.is_some())
        })
    }

    /// Get all HTTP requests for a media type.
    pub fn get_http_requests(&self, media_type: &str) -> &[HttpRequestMetric] {
        self.metrics
            .get(media_type)
            .map_or(&[], |m| &m.http_list)
    }

    /// Add an HTTP request metric.
    pub fn add_http_request(&mut self, media_type: &str, metric: HttpRequestMetric) {
        self.get_or_create(media_type).http_list.push(metric);
    }

    /// Get current DVR info for a media type.
    pub fn get_current_dvr_info(&self, media_type: Option<&str>) -> Option<&DvrInfo> {
        let mt = media_type.unwrap_or("video");
        self.metrics
            .get(mt)
            .and_then(|m| m.dvr_info.last())
            .or_else(|| {
                self.metrics
                    .get("audio")
                    .and_then(|m| m.dvr_info.last())
            })
    }

    /// Add DVR info.
    pub fn add_dvr_info(
        &mut self,
        media_type: &str,
        timestamp_ms: f64,
        start: f64,
        end: f64,
    ) {
        self.get_or_create(media_type).dvr_info.push(DvrInfo {
            media_type: media_type.to_string(),
            timestamp_ms,
            start,
            end,
            range: Some((start, end)),
        });
    }

    /// Get current dropped frames.
    pub fn get_current_dropped_frames(&self) -> Option<&DroppedFrames> {
        self.metrics
            .get("video")
            .and_then(|m| m.dropped_frames.last())
    }

    /// Add dropped frames info.
    pub fn add_dropped_frames(&mut self, timestamp_ms: f64, dropped: u64) {
        self.get_or_create("video")
            .dropped_frames
            .push(DroppedFrames {
                timestamp_ms,
                dropped_frames: dropped,
            });
    }

    /// Get current scheduling info for a media type.
    pub fn get_current_scheduling_info(&self, media_type: &str) -> Option<&SchedulingInfo> {
        self.metrics
            .get(media_type)
            .and_then(|m| m.scheduling_info.last())
    }

    /// Add scheduling info.
    pub fn add_scheduling_info(&mut self, info: SchedulingInfo) {
        let media_type = info.media_type.clone();
        self.get_or_create(&media_type)
            .scheduling_info
            .push(info);
    }

    /// Clear all metrics.
    pub fn clear_all(&mut self) {
        self.metrics.clear();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_new_metrics() {
        let metrics = DashMetrics::new();
        assert_eq!(metrics.get_current_buffer_level("video"), 0.0);
        assert!(metrics.get_current_buffer_state("video").is_none());
    }

    #[test]
    fn test_buffer_level() {
        let mut metrics = DashMetrics::new();
        metrics.add_buffer_level("video", 1000.0, 5000.0);
        metrics.add_buffer_level("video", 2000.0, 8000.0);

        assert!((metrics.get_current_buffer_level("video") - 8.0).abs() < 0.001);
    }

    #[test]
    fn test_buffer_state() {
        let mut metrics = DashMetrics::new();
        metrics.add_buffer_state("video", BufferState::Filling, 30.0);
        metrics.add_buffer_state("video", BufferState::Steady, 30.0);

        let state = metrics.get_current_buffer_state("video").unwrap();
        assert_eq!(state.state, BufferState::Steady);
    }

    #[test]
    fn test_representation_switch() {
        let mut metrics = DashMetrics::new();
        metrics.add_representation_switch("video", 1000.0, 5.0, "rep-1");
        metrics.add_representation_switch("video", 2000.0, 10.0, "rep-2");

        let switch = metrics.get_current_representation_switch("video").unwrap();
        assert_eq!(switch.to_id, "rep-2");
    }

    #[test]
    fn test_http_request_metrics() {
        let mut metrics = DashMetrics::new();
        metrics.add_http_request(
            "video",
            HttpRequestMetric {
                url: "https://cdn.example.com/seg-1.m4s".to_string(),
                actual_url: None,
                request_type: "MediaSegment".to_string(),
                media_type: "video".to_string(),
                response_code: Some(200),
                latency_ms: Some(50.0),
                download_time_ms: Some(200.0),
                bytes_loaded: Some(50000),
                bandwidth_bps: Some(2000000.0),
                request_start_time_ms: Some(1000.0),
                first_byte_time_ms: Some(1050.0),
                request_end_time_ms: Some(1200.0),
            },
        );

        let req = metrics.get_current_http_request("video").unwrap();
        assert_eq!(req.response_code, Some(200));
    }

    #[test]
    fn test_dvr_info() {
        let mut metrics = DashMetrics::new();
        metrics.add_dvr_info("video", 1000.0, 0.0, 30.0);

        let dvr = metrics.get_current_dvr_info(Some("video")).unwrap();
        assert!((dvr.start - 0.0).abs() < 0.001);
        assert!((dvr.end - 30.0).abs() < 0.001);
    }

    #[test]
    fn test_dropped_frames() {
        let mut metrics = DashMetrics::new();
        metrics.add_dropped_frames(1000.0, 5);
        metrics.add_dropped_frames(2000.0, 12);

        let df = metrics.get_current_dropped_frames().unwrap();
        assert_eq!(df.dropped_frames, 12);
    }

    #[test]
    fn test_clear_all() {
        let mut metrics = DashMetrics::new();
        metrics.add_buffer_level("video", 1000.0, 5000.0);
        metrics.add_buffer_level("audio", 1000.0, 3000.0);
        metrics.clear_all();

        assert_eq!(metrics.get_current_buffer_level("video"), 0.0);
        assert_eq!(metrics.get_current_buffer_level("audio"), 0.0);
    }

    #[test]
    fn test_scheduling_info() {
        let mut metrics = DashMetrics::new();
        metrics.add_scheduling_info(SchedulingInfo {
            timestamp_ms: 1000.0,
            media_type: "video".to_string(),
            request_type: "MediaSegment".to_string(),
            start_time: Some(0.0),
            duration: Some(2.0),
            bandwidth: Some(1000000),
            range: None,
            state: "executed".to_string(),
        });

        let info = metrics.get_current_scheduling_info("video").unwrap();
        assert_eq!(info.state, "executed");
    }
}
