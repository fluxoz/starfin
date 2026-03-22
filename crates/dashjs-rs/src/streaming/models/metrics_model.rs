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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn new_model_defaults() {
        let model = MetricsModel::new();
        assert!(model.http_list.is_empty());
        assert_eq!(model.buffer_level, 0.0);
        assert!(model.buffer_state.is_empty());
        assert_eq!(model.dropped_frames, 0);
    }

    #[test]
    fn add_http_request() {
        let mut model = MetricsModel::new();
        let req = HttpRequest { url: "http://example.com/seg.m4s".into(), responsecode: Some(200), ..Default::default() };
        model.add_http_request(req);
        assert_eq!(model.http_list.len(), 1);
        assert_eq!(model.http_list[0].url, "http://example.com/seg.m4s");
    }

    #[test]
    fn add_multiple_http_requests() {
        let mut model = MetricsModel::new();
        for _ in 0..3 {
            model.add_http_request(HttpRequest::default());
        }
        assert_eq!(model.http_list.len(), 3);
    }

    #[test]
    fn reset_restores_defaults() {
        let mut model = MetricsModel::new();
        model.add_http_request(HttpRequest::default());
        model.buffer_level = 10.0;
        model.buffer_state = "bufferLoaded".into();
        model.dropped_frames = 5;
        model.reset();
        assert!(model.http_list.is_empty());
        assert_eq!(model.buffer_level, 0.0);
        assert!(model.buffer_state.is_empty());
        assert_eq!(model.dropped_frames, 0);
    }

    #[test]
    fn reset_after_reset_is_idempotent() {
        let mut model = MetricsModel::new();
        model.add_http_request(HttpRequest::default());
        model.reset();
        model.reset();
        assert!(model.http_list.is_empty());
    }
}
