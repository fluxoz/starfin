//! Port of `dash.js/src/streaming/models/FragmentModel.js`.
use crate::streaming::vo::FragmentRequest;
#[derive(Clone, Debug, Default)]
pub struct FragmentModel {
    pub executed_requests: Vec<FragmentRequest>,
    pub loading_requests: Vec<FragmentRequest>,
}
impl FragmentModel {
    pub fn new() -> Self { Self::default() }
    pub fn add_executed_request(&mut self, req: FragmentRequest) { self.executed_requests.push(req); }
    pub fn get_loading_requests(&self) -> &[FragmentRequest] { &self.loading_requests }
    pub fn reset(&mut self) { self.executed_requests.clear(); self.loading_requests.clear(); }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn new_model_is_empty() {
        let model = FragmentModel::new();
        assert!(model.executed_requests.is_empty());
        assert!(model.get_loading_requests().is_empty());
    }

    #[test]
    fn add_executed_request() {
        let mut model = FragmentModel::new();
        let req = FragmentRequest { action: "download".into(), media_type: "video".into(), ..Default::default() };
        model.add_executed_request(req);
        assert_eq!(model.executed_requests.len(), 1);
        assert_eq!(model.executed_requests[0].action, "download");
    }

    #[test]
    fn add_multiple_executed_requests() {
        let mut model = FragmentModel::new();
        for i in 0..5 {
            model.add_executed_request(FragmentRequest { quality: i, ..Default::default() });
        }
        assert_eq!(model.executed_requests.len(), 5);
        assert_eq!(model.executed_requests[4].quality, 4);
    }

    #[test]
    fn get_loading_requests_empty() {
        let model = FragmentModel::new();
        assert!(model.get_loading_requests().is_empty());
    }

    #[test]
    fn reset_clears_all() {
        let mut model = FragmentModel::new();
        model.add_executed_request(FragmentRequest::default());
        model.loading_requests.push(FragmentRequest::default());
        model.reset();
        assert!(model.executed_requests.is_empty());
        assert!(model.get_loading_requests().is_empty());
    }
}
