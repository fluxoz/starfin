//! Port of `dash.js/src/streaming/models/ThroughputModel.js`.
#[derive(Clone, Debug, Default)]
pub struct ThroughputModel { samples: Vec<f64> }
impl ThroughputModel {
    pub fn new() -> Self { Self::default() }
    pub fn push(&mut self, bps: f64) { self.samples.push(bps); }
    pub fn average(&self) -> f64 {
        if self.samples.is_empty() { 0.0 } else { self.samples.iter().sum::<f64>() / self.samples.len() as f64 }
    }
    pub fn reset(&mut self) { self.samples.clear(); }
}
