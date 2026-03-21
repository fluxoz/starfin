//! Port of `dash.js/src/streaming/models/MetricsModel.js`.
use crate::streaming::vo::HttpRequest;
#[derive(Clone, Debug, Default)]
pub struct MetricsModel {
    pub http_list: Vec<HttpRequest>,
    pub buffer_level: f64,
    pub buffer_state: String,
    pub dropped_frames: u32,
}
impl MetricsModel {
    pub fn new() -> Self { Self::default() }
    pub fn add_http_request(&mut self, req: HttpRequest) { self.http_list.push(req); }
    pub fn reset(&mut self) { *self = Self::default(); }
}
