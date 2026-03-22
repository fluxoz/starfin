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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn average_empty() {
        let model = ThroughputModel::new();
        assert_eq!(model.average(), 0.0);
    }

    #[test]
    fn average_single_sample() {
        let mut model = ThroughputModel::new();
        model.push(5000.0);
        assert_eq!(model.average(), 5000.0);
    }

    #[test]
    fn average_multiple_samples() {
        let mut model = ThroughputModel::new();
        model.push(1000.0);
        model.push(2000.0);
        model.push(3000.0);
        assert!((model.average() - 2000.0).abs() < f64::EPSILON);
    }

    #[test]
    fn push_accumulates() {
        let mut model = ThroughputModel::new();
        for i in 1..=10 {
            model.push(i as f64 * 100.0);
        }
        let expected = (1..=10).map(|i| i as f64 * 100.0).sum::<f64>() / 10.0;
        assert!((model.average() - expected).abs() < f64::EPSILON);
    }

    #[test]
    fn reset_clears_samples() {
        let mut model = ThroughputModel::new();
        model.push(1000.0);
        model.push(2000.0);
        model.reset();
        assert_eq!(model.average(), 0.0);
    }
}
