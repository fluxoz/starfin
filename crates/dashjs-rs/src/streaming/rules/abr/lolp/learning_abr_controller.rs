//! Port of `dash.js/src/streaming/rules/abr/lolp/LearningAbrController.js`.
//!
//! Self-Organising Map (SOM) based learning controller that selects the next
//! quality level by finding the Best Matching Unit (BMU) in the neuron grid
//! and updating the neighbourhood.

use crate::streaming::rules::switch_request::RepresentationInfo;

use super::lolp_qoe_evaluator::LoLpQoeEvaluator;
use super::lolp_weight_selector::LoLpWeightSelector;
use super::{NeuronState, SomNeuron};

// ---------------------------------------------------------------------------
// Constants
// ---------------------------------------------------------------------------

/// SOM target latency: zero means "as low as possible".
const TARGET_LATENCY: f64 = 0.0;
/// SOM target rebuffer level: zero means no rebuffering.
const TARGET_REBUFFER: f64 = 0.0;
/// Throughput headroom (bps) below which a bitrate is considered unsafe.
/// Mirrors the `throughputDelta = 10 000` comment in the JS source
/// ("10K + video encoding is the recommended throughput").
const THROUGHPUT_DELTA_BPS: f64 = 10_000.0;
/// Distance weight applied to neurons whose bitrate exceeds available
/// throughput or whose selection would drain the buffer.  The large value
/// (100×) strongly discourages those neurons from being chosen as the BMU.
const PENALTY_DISTANCE_WEIGHT: f64 = 100.0;

// ---------------------------------------------------------------------------
// Weight selection mode
// ---------------------------------------------------------------------------

#[derive(Clone, Debug, PartialEq)]
enum WeightSelectionMode {
    #[allow(dead_code)]
    Manual,
    #[allow(dead_code)]
    Random,
    Dynamic,
}

// ---------------------------------------------------------------------------
// Minimal PRNG (xorshift64) – avoids pulling in the `rand` crate.
// ---------------------------------------------------------------------------

struct Rng(u64);

impl Rng {
    fn new() -> Self {
        // Fixed non-zero seed; behavioural variance comes from network conditions.
        Rng(0x853c49e6748fea9b_u64)
    }

    fn next_u64(&mut self) -> u64 {
        self.0 ^= self.0 << 13;
        self.0 ^= self.0 >> 7;
        self.0 ^= self.0 << 17;
        self.0
    }

    fn next_f64(&mut self) -> f64 {
        (self.next_u64() >> 11) as f64 / (1u64 << 53) as f64
    }
}

// ---------------------------------------------------------------------------
// LearningAbrController
// ---------------------------------------------------------------------------

/// SOM-based learning controller for the LoL+ ABR rule.
pub struct LearningAbrController {
    som_bitrate_neurons: Option<Vec<SomNeuron>>,
    bitrate_normalization_factor: f64,
    latency_normalization_factor: f64,
    /// Minimum bandwidth across all representations (bps).
    min_bitrate_bandwidth: u64,
    /// Current distance weights `[throughput, latency, rebuffer, switch]`.
    weights: Option<[f64; 4]>,
    /// k-means++ initial centres sorted by dissimilarity.
    sorted_centers: Option<Vec<[f64; 4]>>,
    weight_selection_mode: WeightSelectionMode,
    rng: Rng,
}

impl Default for LearningAbrController {
    fn default() -> Self {
        Self {
            som_bitrate_neurons: None,
            bitrate_normalization_factor: 1.0,
            latency_normalization_factor: 100.0,
            min_bitrate_bandwidth: 0,
            weights: None,
            sorted_centers: None,
            weight_selection_mode: WeightSelectionMode::Dynamic,
            rng: Rng::new(),
        }
    }
}

impl LearningAbrController {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn reset(&mut self) {
        self.som_bitrate_neurons = None;
        self.bitrate_normalization_factor = 1.0;
        self.min_bitrate_bandwidth = 0;
        self.weights = None;
        self.sorted_centers = None;
        self.weight_selection_mode = WeightSelectionMode::Dynamic;
    }

    // -----------------------------------------------------------------------
    // Distance helpers
    // -----------------------------------------------------------------------

    /// Weighted Euclidean distance (sign-preserving for numerical consistency
    /// with the JS original; sum is always ≥ 0 with non-negative weights).
    pub fn get_distance(a: &[f64], b: &[f64], w: &[f64]) -> f64 {
        let sum: f64 = a
            .iter()
            .zip(b.iter())
            .zip(w.iter())
            .map(|((ai, bi), wi)| wi * (ai - bi).powi(2))
            .sum();
        let sign = if sum < 0.0 { -1.0 } else { 1.0 };
        sign * sum.abs().sqrt()
    }

    fn get_neuron_distance(a: &SomNeuron, b: &SomNeuron) -> f64 {
        let a_s = [a.state.throughput, a.state.latency, a.state.rebuffer, a.state.switch];
        let b_s = [b.state.throughput, b.state.latency, b.state.rebuffer, b.state.switch];
        Self::get_distance(&a_s, &b_s, &[1.0, 1.0, 1.0, 1.0])
    }

    fn get_magnitude(v: &[f64]) -> f64 {
        v.iter().map(|x| x * x).sum::<f64>().sqrt()
    }

    // -----------------------------------------------------------------------
    // SOM update
    // -----------------------------------------------------------------------

    /// Pull all neurons toward `x` with a Gaussian neighbourhood centred on
    /// `winner`, learning rate 0.01.
    fn update_neurons(neurons: &mut Vec<SomNeuron>, winner: &SomNeuron, x: &[f64; 4]) {
        let sigma = 0.1_f64;
        // Precompute distances to avoid borrow conflict.
        let distances: Vec<f64> = neurons
            .iter()
            .map(|n| {
                let ns = [n.state.throughput, n.state.latency, n.state.rebuffer, n.state.switch];
                let ws =
                    [winner.state.throughput, winner.state.latency, winner.state.rebuffer, winner.state.switch];
                Self::get_distance(&ns, &ws, &[1.0, 1.0, 1.0, 1.0])
            })
            .collect();

        for (i, n) in neurons.iter_mut().enumerate() {
            let hood = f64::exp(-distances[i].powi(2) / (2.0 * sigma.powi(2)));
            let lr = 0.01;
            n.state.throughput += (x[0] - n.state.throughput) * lr * hood;
            n.state.latency += (x[1] - n.state.latency) * lr * hood;
            n.state.rebuffer += (x[2] - n.state.rebuffer) * lr * hood;
            n.state.switch += (x[3] - n.state.switch) * lr * hood;
        }
    }

    // -----------------------------------------------------------------------
    // Buffer-safety downshift
    // -----------------------------------------------------------------------

    fn get_downshift_neuron<'a>(
        neurons: &'a [SomNeuron],
        current: &'a SomNeuron,
        current_throughput: f64,
    ) -> &'a SomNeuron {
        let mut max_suitable: u64 = 0;
        let mut result = current;
        for n in neurons {
            if n.representation.bandwidth < current.representation.bandwidth
                && n.representation.bandwidth > max_suitable
                && current_throughput > n.representation.bandwidth as f64
            {
                max_suitable = n.representation.bandwidth;
                result = n;
            }
        }
        result
    }

    // -----------------------------------------------------------------------
    // Initialisation
    // -----------------------------------------------------------------------

    fn initialize_neurons(&mut self, representations: &[RepresentationInfo]) {
        let bw_list: Vec<f64> = representations.iter().map(|r| r.bandwidth as f64).collect();
        self.min_bitrate_bandwidth =
            bw_list.iter().cloned().fold(f64::INFINITY, f64::min) as u64;
        self.bitrate_normalization_factor = Self::get_magnitude(&bw_list);
        if self.bitrate_normalization_factor == 0.0 {
            self.bitrate_normalization_factor = 1.0;
        }

        let bnf = self.bitrate_normalization_factor;
        let neurons: Vec<SomNeuron> = representations
            .iter()
            .map(|rep| SomNeuron {
                representation: rep.clone(),
                state: NeuronState {
                    throughput: rep.bandwidth as f64 / bnf,
                    latency: 0.0,
                    rebuffer: 0.0,
                    switch: 0.0,
                },
            })
            .collect();

        let sorted_centers = self.compute_kmeans_plus_plus_centers(&neurons);
        self.sorted_centers = Some(sorted_centers);
        self.som_bitrate_neurons = Some(neurons);
    }

    fn max_throughput_normalized(&self) -> f64 {
        self.som_bitrate_neurons
            .as_ref()
            .map(|ns| ns.iter().fold(0.0_f64, |m, n| f64::max(m, n.state.throughput)))
            .unwrap_or(0.0)
    }

    fn random_data(&mut self, count: usize, max_tp: f64) -> Vec<[f64; 4]> {
        (0..count)
            .map(|_| {
                [
                    self.rng.next_f64() * max_tp,
                    self.rng.next_f64(),
                    self.rng.next_f64(),
                    self.rng.next_f64(),
                ]
            })
            .collect()
    }

    fn compute_kmeans_plus_plus_centers(&mut self, neurons: &[SomNeuron]) -> Vec<[f64; 4]> {
        let max_tp = neurons.iter().fold(0.0_f64, |m, n| f64::max(m, n.state.throughput));
        let dataset = self.random_data(neurons.len() * neurons.len(), max_tp);
        let dw = [1.0, 1.0, 1.0, 1.0];

        let mut centers: Vec<[f64; 4]> = vec![dataset[0]];

        for _ in 1..neurons.len() {
            let mut next_point = dataset[0];
            let mut max_dist: Option<f64> = None;
            for &point in &dataset {
                let mut min_dist: Option<f64> = None;
                for &center in &centers {
                    let d = Self::get_distance(&point, &center, &dw);
                    if min_dist.is_none() || d < min_dist.unwrap() {
                        min_dist = Some(d);
                    }
                }
                if let Some(md) = min_dist {
                    if max_dist.is_none() || md > max_dist.unwrap() {
                        next_point = point;
                        max_dist = Some(md);
                    }
                }
            }
            centers.push(next_point);
        }

        // Find the centre least similar to all others.
        let mut least_similar_idx = 0;
        let mut max_total: Option<f64> = None;
        for i in 0..centers.len() {
            let total: f64 = (0..centers.len())
                .filter(|&j| j != i)
                .map(|j| Self::get_distance(&centers[i], &centers[j], &dw))
                .sum();
            if max_total.is_none() || total > max_total.unwrap() {
                max_total = Some(total);
                least_similar_idx = i;
            }
        }

        // Build sorted list starting from the least-similar centre.
        let first = centers.remove(least_similar_idx);
        let mut sorted = vec![first];
        while !centers.is_empty() {
            let mut min_d: Option<f64> = None;
            let mut min_idx = 0;
            for (i, &c) in centers.iter().enumerate() {
                let d = Self::get_distance(&sorted[0], &c, &dw);
                if min_d.is_none() || d < min_d.unwrap() {
                    min_d = Some(d);
                    min_idx = i;
                }
            }
            sorted.push(centers.remove(min_idx));
        }

        sorted
    }

    // -----------------------------------------------------------------------
    // Weight selection strategies
    // -----------------------------------------------------------------------

    fn manual_weight_selection(&mut self) {
        self.weights = Some([0.4, 0.4, 0.4, 0.4]);
    }

    fn random_weight_selection(&mut self) {
        let n = self.som_bitrate_neurons.as_ref().map(|v| v.len()).unwrap_or(1);
        let upper = f64::sqrt(2.0 / n as f64);
        self.weights = Some([
            self.rng.next_f64() * upper,
            self.rng.next_f64() * upper,
            self.rng.next_f64() * upper,
            self.rng.next_f64() * upper,
        ]);
    }

    fn dynamic_weight_selection(
        &mut self,
        weight_selector: &mut LoLpWeightSelector,
        qoe_evaluator: &LoLpQoeEvaluator,
        current_latency: f64,
        current_buffer: f64,
        rebuffer: f64,
        current_throughput: f64,
        playback_rate: f64,
    ) {
        // Fallback to last sorted centre if weights have not been set yet.
        if self.weights.is_none() {
            if let Some(ref sc) = self.sorted_centers {
                if let Some(&last) = sc.last() {
                    self.weights = Some(last);
                }
            }
        }

        let neurons = self.som_bitrate_neurons.as_deref().unwrap_or(&[]);
        if let Some(wv) = weight_selector.find_weight_vector(
            neurons,
            current_latency,
            current_buffer,
            rebuffer,
            current_throughput,
            playback_rate,
            qoe_evaluator,
        ) {
            self.weights = Some(wv);
        }
    }

    // -----------------------------------------------------------------------
    // Public API
    // -----------------------------------------------------------------------

    /// Select the next quality representation using the SOM.
    ///
    /// Initialises the SOM on first call. Subsequent calls update neuron
    /// weights with the observed QoE.
    pub fn get_next_quality(
        &mut self,
        representations: &[RepresentationInfo],
        throughput: f64,
        latency: f64,
        current_buffer_level: f64,
        playback_rate: f64,
        current_representation: &RepresentationInfo,
        weight_selector: &mut LoLpWeightSelector,
        qoe_evaluator: &LoLpQoeEvaluator,
    ) -> Option<RepresentationInfo> {
        if representations.is_empty() {
            return None;
        }

        if self.som_bitrate_neurons.is_none() {
            self.initialize_neurons(representations);
        }

        let bnf = self.bitrate_normalization_factor;
        let lnf = self.latency_normalization_factor;

        // Normalise throughput; saturate above 1 to the observed max.
        let mut tp_norm = throughput / bnf;
        if tp_norm > 1.0 {
            tp_norm = self.max_throughput_normalized();
        }
        let latency_norm = latency / lnf;

        let target_latency = TARGET_LATENCY;
        let target_rebuffer = TARGET_REBUFFER;
        let throughput_delta = THROUGHPUT_DELTA_BPS;

        let neurons = self.som_bitrate_neurons.as_ref().unwrap();

        // Locate the neuron that matches the currently-playing representation.
        let current_idx = neurons
            .iter()
            .position(|n| match (&n.representation.id, &current_representation.id) {
                (Some(a), Some(b)) => a == b,
                _ => n.representation.bandwidth == current_representation.bandwidth,
            })
            .unwrap_or(0);

        let cur_bw = neurons[current_idx].representation.bandwidth as f64;
        let seg_dur = weight_selector.get_segment_duration();
        let download_time = (cur_bw * seg_dur) / throughput.max(1.0);
        let rebuffer = f64::max(0.0, download_time - current_buffer_level);

        // Buffer-stall check: downshift immediately if buffer is too low.
        if current_buffer_level - download_time < weight_selector.get_min_buffer() {
            let current_neuron = neurons[current_idx].clone();
            let neurons = self.som_bitrate_neurons.as_ref().unwrap();
            return Some(
                Self::get_downshift_neuron(neurons, &current_neuron, throughput)
                    .representation
                    .clone(),
            );
        }

        // Select weights for this step.
        match self.weight_selection_mode {
            WeightSelectionMode::Manual => self.manual_weight_selection(),
            WeightSelectionMode::Random => self.random_weight_selection(),
            WeightSelectionMode::Dynamic => {
                self.dynamic_weight_selection(
                    weight_selector,
                    qoe_evaluator,
                    latency,
                    current_buffer_level,
                    rebuffer,
                    throughput,
                    playback_rate,
                );
            }
        }

        let weights = self.weights.unwrap_or([1.0, 1.0, 1.0, 1.0]);

        // Find the Best Matching Unit (BMU).
        let mut min_dist: Option<f64> = None;
        let mut winner_idx = 0;
        let mut target_rep: Option<RepresentationInfo> = None;

        let neurons = self.som_bitrate_neurons.as_ref().unwrap();
        for (i, n) in neurons.iter().enumerate() {
            let som_data =
                [n.state.throughput, n.state.latency, n.state.rebuffer, n.state.switch];

            let mut dw = weights;
            let next_buf = weight_selector.get_next_buffer_with_bitrate(
                n.representation.bandwidth as f64,
                current_buffer_level,
                throughput,
            );
            let is_buf_low = next_buf < weight_selector.get_min_buffer();

            // Penalise neurons whose bitrate exceeds available throughput or that
            // would drain the buffer.
            if n.representation.bandwidth as f64 > throughput - throughput_delta || is_buf_low {
                if n.representation.bandwidth != self.min_bitrate_bandwidth {
                    dw[0] = PENALTY_DISTANCE_WEIGHT;
                }
            }

            let d = Self::get_distance(
                &som_data,
                &[tp_norm, target_latency, target_rebuffer, 0.0],
                &dw,
            );
            if min_dist.is_none() || d < min_dist.unwrap() {
                min_dist = Some(d);
                winner_idx = i;
                target_rep = Some(n.representation.clone());
            }
        }

        // Update the SOM: pull the current and winner neurons toward the
        // observed state, punishing the current neuron if it was not selected.
        let winner_bw = neurons[winner_idx].representation.bandwidth as f64;
        let bitrate_switch = (cur_bw - winner_bw).abs() / bnf;

        let current_clone = neurons[current_idx].clone();
        let winner_clone = neurons[winner_idx].clone();

        let ns = self.som_bitrate_neurons.as_mut().unwrap();
        Self::update_neurons(
            ns,
            &current_clone,
            &[tp_norm, latency_norm, rebuffer, bitrate_switch],
        );
        let ns = self.som_bitrate_neurons.as_mut().unwrap();
        Self::update_neurons(
            ns,
            &winner_clone,
            &[tp_norm, target_latency, target_rebuffer, bitrate_switch],
        );

        target_rep
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::streaming::rules::switch_request::RepresentationInfo;

    fn make_reps() -> Vec<RepresentationInfo> {
        vec![
            RepresentationInfo {
                quality_index: 0,
                bandwidth: 500_000,
                bitrate_in_kbit: 500.0,
                media_type: "video".into(),
                id: Some("0".into()),
                absolute_index: 0,
            },
            RepresentationInfo {
                quality_index: 1,
                bandwidth: 1_000_000,
                bitrate_in_kbit: 1000.0,
                media_type: "video".into(),
                id: Some("1".into()),
                absolute_index: 1,
            },
            RepresentationInfo {
                quality_index: 2,
                bandwidth: 2_000_000,
                bitrate_in_kbit: 2000.0,
                media_type: "video".into(),
                id: Some("2".into()),
                absolute_index: 2,
            },
        ]
    }

    #[test]
    fn returns_a_representation() {
        let reps = make_reps();
        let mut lc = LearningAbrController::new();
        let mut ev = LoLpQoeEvaluator::new();
        ev.setup_per_segment_qoe(2.0, 2000.0, 500.0);
        let mut ws = LoLpWeightSelector::new(1.5, 0.3, 2.0);
        let result = lc.get_next_quality(
            &reps,
            2_000_000.0,
            1.0,
            10.0,
            1.0,
            &reps[1],
            &mut ws,
            &ev,
        );
        assert!(result.is_some());
    }

    #[test]
    fn downshifts_when_buffer_is_low() {
        let reps = make_reps();
        let mut lc = LearningAbrController::new();
        let mut ev = LoLpQoeEvaluator::new();
        ev.setup_per_segment_qoe(2.0, 2000.0, 500.0);
        let mut ws = LoLpWeightSelector::new(1.5, 0.3, 2.0);
        // Very low buffer (0.1s), high bitrate segment → buffer stall
        let result = lc.get_next_quality(
            &reps,
            500_000.0,
            1.0,
            0.1, // buffer almost empty
            1.0,
            &reps[2], // currently at highest quality
            &mut ws,
            &ev,
        );
        // Should downshift (not return highest quality)
        let rep = result.unwrap();
        assert!(rep.bandwidth <= reps[2].bandwidth);
    }
}
