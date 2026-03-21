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
