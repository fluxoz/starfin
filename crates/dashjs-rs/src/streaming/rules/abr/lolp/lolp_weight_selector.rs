//! Port of `dash.js/src/streaming/rules/abr/lolp/LoLpWeightSelector.js`.
//!
//! Generates permutations of candidate weight vectors and selects the one
//! that maximises QoE while satisfying latency and buffer constraints.

use super::lolp_qoe_evaluator::LoLpQoeEvaluator;
use super::SomNeuron;

/// Fallback inverse weight when a weight vector element is zero.
/// Prevents division-by-zero while still producing a high (penalising) multiplier.
const ZERO_WEIGHT_FALLBACK: f64 = 10.0;

/// Dynamic Weight Selector for the LoL+ ABR rule.
///
/// Created fresh on each ABR invocation (mirrors the JS pattern where a new
/// instance is created inside `getSwitchRequest`).
pub struct LoLpWeightSelector {
    target_latency: f64,
    buffer_min: f64,
    segment_duration: f64,
    /// All permutations of `[0.2, 0.4, 0.6, 0.8, 1.0]^4` (625 vectors).
    weight_options: Vec<[f64; 4]>,
    /// Latency observed on the previous `find_weight_vector` call (delta calc).
    pub previous_latency: f64,
}

impl LoLpWeightSelector {
    pub fn new(target_latency: f64, buffer_min: f64, segment_duration: f64) -> Self {
        let weight_options = Self::get_permutations(&[0.2, 0.4, 0.6, 0.8, 1.0], 4);
        Self {
            target_latency,
            buffer_min,
            segment_duration,
            weight_options,
            previous_latency: 0.0,
        }
    }

    /// Generate all ordered permutations with repetition of `list` of length `len`.
    fn get_permutations(list: &[f64], len: usize) -> Vec<[f64; 4]> {
        let n = list.len();
        let total = n.pow(len as u32);
        let mut result = Vec::with_capacity(total);
        for i in 0..total {
            let mut perm = [0.0f64; 4];
            let mut idx = i;
            for j in (0..len).rev() {
                perm[j] = list[idx % n];
                idx /= n;
            }
            result.push(perm);
        }
        result
    }

    pub fn get_min_buffer(&self) -> f64 {
        self.buffer_min
    }

    pub fn get_segment_duration(&self) -> f64 {
        self.segment_duration
    }

    /// Predicted buffer level after downloading the next segment.
    pub fn get_next_buffer(&self, current_buffer: f64, download_time: f64) -> f64 {
        if download_time > self.segment_duration {
            current_buffer - self.segment_duration
        } else {
            current_buffer + self.segment_duration - download_time
        }
    }

    /// Convenience: compute `get_next_buffer` directly from bitrate and throughput.
    pub fn get_next_buffer_with_bitrate(
        &self,
        bitrate_to_download: f64,
        current_buffer: f64,
        current_throughput: f64,
    ) -> f64 {
        let download_time =
            (bitrate_to_download * self.segment_duration) / current_throughput.max(1.0);
        self.get_next_buffer(current_buffer, download_time)
    }

    /// Enumerate all (neuron × weight-vector) pairs, return the weight vector
    /// that maximises QoE subject to latency and buffer constraints.
    ///
    /// Returns `None` when no candidate satisfies the constraints (caller
    /// should keep existing weights).
    pub fn find_weight_vector(
        &mut self,
        neurons: &[SomNeuron],
        current_latency: f64,
        current_buffer: f64,
        _current_rebuffer: f64,
        current_throughput: f64,
        playback_rate: f64,
        qoe_evaluator: &LoLpQoeEvaluator,
    ) -> Option<[f64; 4]> {
        let delta_latency = (current_latency - self.previous_latency).abs();
        let mut max_qoe: Option<f64> = None;
        let mut winner_weights: Option<[f64; 4]> = None;

        for neuron in neurons {
            for &weight_vector in &self.weight_options {
                // Inverse-weight the buffer and latency so higher weight = less penalty.
                let wt_buffer =
                    if weight_vector[2] == 0.0 { ZERO_WEIGHT_FALLBACK } else { 1.0 / weight_vector[2] };
                let wt_latency =
                    if weight_vector[1] == 0.0 { ZERO_WEIGHT_FALLBACK } else { 1.0 / weight_vector[1] };

                let download_time = (neuron.representation.bandwidth as f64
                    * self.segment_duration)
                    / current_throughput.max(1.0);
                let next_buffer = self.get_next_buffer(current_buffer, download_time);
                let rebuffer = f64::max(0.00001, download_time - next_buffer);
                let weighted_rebuffer = wt_buffer * rebuffer;
                let weighted_latency = wt_latency * neuron.state.latency;

                let total_qoe = qoe_evaluator.calculate_single_use_qoe(
                    neuron.representation.bandwidth as f64,
                    weighted_rebuffer,
                    weighted_latency,
                    playback_rate,
                );

                if self.check_constraints(current_latency, next_buffer, delta_latency) {
                    if max_qoe.is_none() || total_qoe > max_qoe.unwrap() {
                        max_qoe = Some(total_qoe);
                        winner_weights = Some(weight_vector);
                    }
                }
            }
        }

        self.previous_latency = current_latency;
        winner_weights
    }

    fn check_constraints(&self, next_latency: f64, next_buffer: f64, delta_latency: f64) -> bool {
        if next_latency > self.target_latency + delta_latency {
            return false;
        }
        next_buffer >= self.buffer_min
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn permutations_count() {
        let ws = LoLpWeightSelector::new(1.5, 0.3, 2.0);
        // 5 values ^ 4 positions = 625
        assert_eq!(ws.weight_options.len(), 625);
    }

    #[test]
    fn get_next_buffer_download_fast() {
        let ws = LoLpWeightSelector::new(1.5, 0.3, 2.0);
        // download_time=1s < segment_duration=2s → buffer grows
        assert!((ws.get_next_buffer(5.0, 1.0) - 6.0).abs() < 1e-9);
    }

    #[test]
    fn get_next_buffer_download_slow() {
        let ws = LoLpWeightSelector::new(1.5, 0.3, 2.0);
        // download_time=3s > segment_duration=2s → buffer shrinks
        assert!((ws.get_next_buffer(5.0, 3.0) - 3.0).abs() < 1e-9);
    }
}
