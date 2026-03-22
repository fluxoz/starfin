//! Port of `dash.js/src/streaming/controllers/ThroughputController.js`.
//!
//! Dual-EWMA throughput estimator.

const FAST_HALF_LIFE: f64 = 3.0;
const SLOW_HALF_LIFE: f64 = 8.0;

#[derive(Clone, Debug)]
pub struct ThroughputController {
    fast_estimate: f64,
    slow_estimate: f64,
    fast_alpha: f64,
    slow_alpha: f64,
    sample_count: u32,
}

impl Default for ThroughputController {
    fn default() -> Self {
        Self {
            fast_estimate: 0.0,
            slow_estimate: 0.0,
            fast_alpha: 1.0 - (-(1.0_f64.ln()) / FAST_HALF_LIFE).exp(),
            slow_alpha: 1.0 - (-(1.0_f64.ln()) / SLOW_HALF_LIFE).exp(),
            sample_count: 0,
        }
    }
}

impl ThroughputController {
    pub fn new() -> Self { Self::default() }

    /// Add a throughput sample (bits per second).
    pub fn add_sample(&mut self, throughput_bps: f64) {
        if self.sample_count == 0 {
            self.fast_estimate = throughput_bps;
            self.slow_estimate = throughput_bps;
        } else {
            self.fast_estimate += self.fast_alpha * (throughput_bps - self.fast_estimate);
            self.slow_estimate += self.slow_alpha * (throughput_bps - self.slow_estimate);
        }
        self.sample_count += 1;
    }

    /// Get conservative (minimum of fast/slow) throughput estimate.
    pub fn get_safe_throughput(&self) -> f64 {
        if self.sample_count == 0 { return 0.0; }
        self.fast_estimate.min(self.slow_estimate)
    }

    pub fn get_average_throughput(&self) -> f64 {
        if self.sample_count == 0 { return 0.0; }
        (self.fast_estimate + self.slow_estimate) / 2.0
    }

    pub fn reset(&mut self) { *self = Self::default(); }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn first_sample_sets_both() {
        let mut tc = ThroughputController::new();
        tc.add_sample(1_000_000.0);
        assert_eq!(tc.get_safe_throughput(), 1_000_000.0);
    }

    #[test]
    fn converges_toward_samples() {
        let mut tc = ThroughputController::new();
        for _ in 0..20 { tc.add_sample(2_000_000.0); }
        let t = tc.get_safe_throughput();
        assert!((t - 2_000_000.0).abs() < 50_000.0);
    }
}
