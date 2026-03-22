//! Port of `dash.js/src/streaming/vo/`.
use serde::{Deserialize, Serialize};

/// Port of `FragmentRequest.js`.
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct FragmentRequest {
    pub action: String,
    pub url: Option<String>,
    pub range: Option<String>,
    pub media_type: String,
    pub quality: usize,
    pub index: Option<u64>,
    pub representation_id: Option<String>,
    pub start_time: f64,
    pub duration: f64,
    pub time_threshold: f64,
    pub available_at: Option<f64>,
    pub wall_start_time: Option<f64>,
    pub bytes_total: u64,
    pub bytes_loaded: u64,
    pub request_start_date: Option<f64>,
    pub first_byte_date: Option<f64>,
    pub request_end_date: Option<f64>,
}

/// Port of `BitrateInfo.js`.
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct BitrateInfo {
    pub media_type: String,
    pub bitrate: u64,
    pub width: Option<u32>,
    pub height: Option<u32>,
    pub quality_index: usize,
}

/// Port of `DataChunk.js`.
#[derive(Clone, Debug, Default)]
pub struct DataChunk {
    pub stream_id: String,
    pub media_type: String,
    pub quality: usize,
    pub index: u64,
    pub bytes: Vec<u8>,
    pub start: f64,
    pub end: f64,
    pub duration: f64,
    pub representation_id: Option<String>,
    pub end_fragment: bool,
}

/// Port of `DashJSError.js`.
#[derive(Clone, Debug)]
pub struct DashJSError {
    pub code: u32,
    pub message: String,
    pub data: Option<String>,
}

/// Port of `TextTrackInfo.js`.
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct TextTrackInfo {
    pub id: Option<String>,
    pub index: usize,
    pub lang: Option<String>,
    pub label: Option<String>,
    pub kind: String,
    pub is_embedded: bool,
    pub is_default_track: bool,
    pub roles: Vec<String>,
    pub accessibility: Vec<String>,
    pub codec: Option<String>,
    pub mime_type: Option<String>,
}

/// Port of `ThumbnailTrackInfo.js`.
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct ThumbnailTrackInfo {
    pub bitrate: u64,
    pub width: u32,
    pub height: u32,
    pub tiles_horizontal: u32,
    pub tiles_vertical: u32,
    pub start_number: u64,
    pub segment_duration: f64,
    pub timescale: u64,
    pub template_url: Option<String>,
}

/// Port of metrics VOs.
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct HttpRequest {
    pub tcp_id: Option<String>,
    pub request_type: String,
    pub url: String,
    pub actual_url: Option<String>,
    pub range: Option<String>,
    pub trequest: Option<f64>,
    pub tresponse: Option<f64>,
    pub responsecode: Option<u16>,
    pub interval: Option<f64>,
    pub trace: Vec<HttpRequestTrace>,
}

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct HttpRequestTrace {
    pub s: f64,
    pub d: f64,
    pub b: Vec<u64>,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fragment_request_defaults() {
        let req = FragmentRequest::default();
        assert!(req.action.is_empty());
        assert!(req.url.is_none());
        assert!(req.range.is_none());
        assert_eq!(req.quality, 0);
        assert_eq!(req.start_time, 0.0);
        assert_eq!(req.duration, 0.0);
        assert_eq!(req.bytes_total, 0);
        assert_eq!(req.bytes_loaded, 0);
    }

    #[test]
    fn fragment_request_custom() {
        let req = FragmentRequest {
            action: "download".into(),
            url: Some("http://example.com/seg1.m4s".into()),
            media_type: "video".into(),
            quality: 3,
            start_time: 10.0,
            duration: 4.0,
            ..Default::default()
        };
        assert_eq!(req.action, "download");
        assert_eq!(req.quality, 3);
    }

    #[test]
    fn bitrate_info_defaults() {
        let bi = BitrateInfo::default();
        assert!(bi.media_type.is_empty());
        assert_eq!(bi.bitrate, 0);
        assert!(bi.width.is_none());
        assert!(bi.height.is_none());
        assert_eq!(bi.quality_index, 0);
    }

    #[test]
    fn data_chunk_defaults() {
        let dc = DataChunk::default();
        assert!(dc.stream_id.is_empty());
        assert!(dc.bytes.is_empty());
        assert!(!dc.end_fragment);
        assert_eq!(dc.index, 0);
    }

    #[test]
    fn dashjs_error_fields() {
        let err = DashJSError { code: 404, message: "Not found".into(), data: Some("extra".into()) };
        assert_eq!(err.code, 404);
        assert_eq!(err.message, "Not found");
        assert_eq!(err.data.as_deref(), Some("extra"));
    }

    #[test]
    fn text_track_info_defaults() {
        let tti = TextTrackInfo::default();
        assert!(tti.id.is_none());
        assert!(tti.roles.is_empty());
        assert!(!tti.is_embedded);
        assert!(!tti.is_default_track);
    }

    #[test]
    fn thumbnail_track_info_defaults() {
        let info = ThumbnailTrackInfo::default();
        assert_eq!(info.bitrate, 0);
        assert_eq!(info.tiles_horizontal, 0);
        assert_eq!(info.tiles_vertical, 0);
        assert!(info.template_url.is_none());
    }

    #[test]
    fn http_request_defaults() {
        let req = HttpRequest::default();
        assert!(req.tcp_id.is_none());
        assert!(req.url.is_empty());
        assert!(req.trace.is_empty());
        assert!(req.responsecode.is_none());
    }

    #[test]
    fn http_request_trace_defaults() {
        let trace = HttpRequestTrace::default();
        assert_eq!(trace.s, 0.0);
        assert_eq!(trace.d, 0.0);
        assert!(trace.b.is_empty());
    }

    #[test]
    fn http_request_with_traces() {
        let req = HttpRequest {
            request_type: "MediaSegment".into(),
            url: "http://example.com/seg.m4s".into(),
            responsecode: Some(200),
            trace: vec![
                HttpRequestTrace { s: 0.0, d: 100.0, b: vec![1024] },
                HttpRequestTrace { s: 100.0, d: 50.0, b: vec![512] },
            ],
            ..Default::default()
        };
        assert_eq!(req.trace.len(), 2);
        assert_eq!(req.responsecode, Some(200));
    }
}
