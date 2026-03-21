//! Port of `dash.js/src/streaming/metrics/`.
//!
//! Metrics collection and reporting infrastructure.

use serde::{Deserialize, Serialize};

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

/// Metrics reporting events stub.
pub struct MetricsReporting;
impl MetricsReporting {
    pub fn new() -> Self { Self }
}
