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
