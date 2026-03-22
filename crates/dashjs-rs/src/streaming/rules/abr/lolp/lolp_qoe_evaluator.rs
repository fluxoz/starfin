//! Port of `dash.js/src/streaming/rules/abr/lolp/LoLpQoEEvaluator.js` and `QoeInfo.js`.
//!
//! QoE metric calculation for the LoL+ ABR algorithm.

/// A single (threshold, penalty) pair for latency-based QoE penalties.
#[derive(Clone, Debug)]
pub struct LatencyPenaltyPoint {
    pub threshold: f64,
    pub penalty: f64,
}

/// Weights controlling how each factor contributes to the QoE score.
#[derive(Clone, Debug, Default)]
pub struct QoeWeights {
    /// Multiplicative reward per kbps of bitrate (set to segment duration).
    pub bitrate_reward: f64,
    /// Multiplicative penalty per kbps of absolute bitrate switch.
    pub bitrate_switch_penalty: f64,
    /// Multiplicative penalty per second of rebuffering (set to max bitrate kbps).
    pub rebuffer_penalty: f64,
    /// Piecewise latency penalties: list of (threshold_s, penalty_per_s).
    pub latency_penalty: Vec<LatencyPenaltyPoint>,
    /// Multiplicative penalty per unit of |1 - playbackSpeed| (set to min bitrate kbps).
    pub playback_speed_penalty: f64,
}

/// Accumulated QoE state for a single segment or stream fragment.
///
/// Port of `dash.js/src/streaming/rules/abr/lolp/QoeInfo.js`.
#[derive(Clone, Debug, Default)]
pub struct QoeInfo {
    pub weights: QoeWeights,
    /// Last logged bitrate used to compute switch penalties.
    pub last_bitrate: Option<f64>,
    pub bitrate_w_sum: f64,
    pub bitrate_switch_w_sum: f64,
    pub rebuffer_w_sum: f64,
    pub latency_w_sum: f64,
    pub playback_speed_w_sum: f64,
    /// Running total QoE = rewards − penalties.
    pub total_qoe: f64,
}

impl QoeInfo {
    pub fn new() -> Self {
        Self::default()
    }
}

/// QoE evaluator that tracks per-segment metrics and computes QoE scores.
///
/// Port of `dash.js/src/streaming/rules/abr/lolp/LoLpQoEEvaluator.js`.
#[derive(Clone, Debug, Default)]
pub struct LoLpQoeEvaluator {
    pub vo_per_segment_qoe_info: Option<QoeInfo>,
    pub segment_duration: Option<f64>,
    pub max_bitrate_kbps: Option<f64>,
    pub min_bitrate_kbps: Option<f64>,
}

impl LoLpQoeEvaluator {
    pub fn new() -> Self {
        Self::default()
    }

    /// Initialise the per-segment QoE accumulator.
    pub fn setup_per_segment_qoe(
        &mut self,
        segment_duration: f64,
        max_bitrate_kbps: f64,
        min_bitrate_kbps: f64,
    ) {
        self.vo_per_segment_qoe_info =
            Some(Self::create_qoe_info(segment_duration, max_bitrate_kbps, min_bitrate_kbps));
        self.segment_duration = Some(segment_duration);
        self.max_bitrate_kbps = Some(max_bitrate_kbps);
        self.min_bitrate_kbps = Some(min_bitrate_kbps);
    }

    /// Create a fresh `QoeInfo` with weights derived from the given stream parameters.
    fn create_qoe_info(
        segment_duration: f64,
        max_bitrate_kbps: f64,
        min_bitrate_kbps: f64,
    ) -> QoeInfo {
        // Weights as per Abdelhak Bentaleb, 2020 – see JS source for rationale.
        let bitrate_reward = if segment_duration == 0.0 { 1.0 } else { segment_duration };
        let bitrate_switch_penalty = 1.0;
        let rebuffer_penalty = if max_bitrate_kbps == 0.0 { 1000.0 } else { max_bitrate_kbps };
        let latency_penalty = vec![
            LatencyPenaltyPoint { threshold: 1.1, penalty: min_bitrate_kbps * 0.05 },
            LatencyPenaltyPoint { threshold: 1.0e8, penalty: max_bitrate_kbps * 0.1 },
        ];
        let playback_speed_penalty =
            if min_bitrate_kbps == 0.0 { 200.0 } else { min_bitrate_kbps };

        QoeInfo {
            weights: QoeWeights {
                bitrate_reward,
                bitrate_switch_penalty,
                rebuffer_penalty,
                latency_penalty,
                playback_speed_penalty,
            },
            ..Default::default()
        }
    }

    /// Accumulate one segment's metrics into the per-segment QoE state.
    pub fn log_segment_metrics(
        &mut self,
        segment_bitrate: f64,
        segment_rebuffer_time: f64,
        current_latency: f64,
        current_playback_speed: f64,
    ) {
        if let Some(ref mut info) = self.vo_per_segment_qoe_info {
            Self::log_metrics_in_qoe_info(
                segment_bitrate,
                segment_rebuffer_time,
                current_latency,
                current_playback_speed,
                info,
            );
        }
    }

    fn log_metrics_in_qoe_info(
        bitrate: f64,
        rebuffer_time: f64,
        latency: f64,
        playback_speed: f64,
        info: &mut QoeInfo,
    ) {
        info.bitrate_w_sum += info.weights.bitrate_reward * bitrate;

        if let Some(last) = info.last_bitrate {
            info.bitrate_switch_w_sum +=
                info.weights.bitrate_switch_penalty * (bitrate - last).abs();
        }
        info.last_bitrate = Some(bitrate);

        info.rebuffer_w_sum += info.weights.rebuffer_penalty * rebuffer_time;

        for lp in &info.weights.latency_penalty {
            if latency <= lp.threshold {
                info.latency_w_sum += lp.penalty * latency;
                break;
            }
        }

        info.playback_speed_w_sum +=
            info.weights.playback_speed_penalty * (1.0 - playback_speed).abs();

        info.total_qoe = info.bitrate_w_sum
            - info.bitrate_switch_w_sum
            - info.rebuffer_w_sum
            - info.latency_w_sum
            - info.playback_speed_w_sum;
    }

    /// Return the current per-segment QoE accumulator.
    pub fn get_per_segment_qoe(&self) -> Option<&QoeInfo> {
        self.vo_per_segment_qoe_info.as_ref()
    }

    /// Compute a one-shot QoE value without mutating persistent state.
    ///
    /// Used by the weight selector for each (neuron, weight-vector) candidate.
    pub fn calculate_single_use_qoe(
        &self,
        segment_bitrate: f64,
        segment_rebuffer_time: f64,
        current_latency: f64,
        current_playback_speed: f64,
    ) -> f64 {
        match (self.segment_duration, self.max_bitrate_kbps, self.min_bitrate_kbps) {
            (Some(sd), Some(max_br), Some(min_br)) => {
                let mut info = Self::create_qoe_info(sd, max_br, min_br);
                Self::log_metrics_in_qoe_info(
                    segment_bitrate,
                    segment_rebuffer_time,
                    current_latency,
                    current_playback_speed,
                    &mut info,
                );
                info.total_qoe
            }
            _ => 0.0,
        }
    }

    pub fn reset(&mut self) {
        *self = Self::default();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn setup_and_log_increases_total_qoe() {
        let mut ev = LoLpQoeEvaluator::new();
        ev.setup_per_segment_qoe(2.0, 4000.0, 500.0);
        ev.log_segment_metrics(2000.0, 0.0, 1.0, 1.0);
        let qoe = ev.get_per_segment_qoe().unwrap();
        // bitrateWSum = 2.0 * 2000 = 4000; no rebuffer; total should be positive
        assert!(qoe.total_qoe > 0.0);
    }

    #[test]
    fn single_use_qoe_without_setup_returns_zero() {
        let ev = LoLpQoeEvaluator::new();
        assert_eq!(ev.calculate_single_use_qoe(1000.0, 0.0, 1.0, 1.0), 0.0);
    }

    #[test]
    fn single_use_qoe_after_setup() {
        let mut ev = LoLpQoeEvaluator::new();
        ev.setup_per_segment_qoe(2.0, 4000.0, 500.0);
        let qoe = ev.calculate_single_use_qoe(2000.0, 0.0, 1.0, 1.0);
        assert!(qoe > 0.0);
    }
}
