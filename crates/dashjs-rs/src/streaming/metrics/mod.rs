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

#[cfg(test)]
mod tests {
    use super::*;

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
        // should not panic
    }
}
