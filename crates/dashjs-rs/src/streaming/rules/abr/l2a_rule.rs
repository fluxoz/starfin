//! Port of `dash.js/src/streaming/rules/abr/L2ARule.js` — Learn2Adapt low-latency ABR.
//!
//! Implements the Learn2Adapt-LowLatency (L2A-LL) algorithm for adaptive bitrate selection in
//! low-latency live streaming. The algorithm uses online convex optimisation to learn per-bitrate
//! selection probabilities via Lagrangian descent and euclidean projection onto the probability
//! simplex.
//!
//! Reference: <https://github.com/unifiedstreaming/Learn2Adapt-LowLatency>

use std::cell::RefCell;
use std::collections::HashMap;
use std::fmt;

use crate::streaming::rules::rules_context::RulesContext;
use crate::streaming::rules::switch_request::{
    Priority, RepresentationInfo, SwitchReason, SwitchRequest,
};
use crate::streaming::rules::AbrRule;

// ---------------------------------------------------------------------------
// Constants (matching dash.js L2ARule.js)
// ---------------------------------------------------------------------------

/// Optimisation horizon – number of steps to achieve convergence.
const HORIZON: f64 = 4.0;

/// Cautiousness parameter: `HORIZON^0.99`.
const VL: f64 = 3.9724420632491064; // 4.0_f64.powf(0.99) pre-computed

/// Re-calibration factor for the Lagrangian when bitrate is over-estimated.
const REACT: f64 = 2.0;

/// Default target buffer level in seconds.
const B_TARGET: f64 = 1.5;

/// Minimum throughput floor (kbit/s) to avoid division by zero.
const MIN_THROUGHPUT: f64 = 1.0;

// ---------------------------------------------------------------------------
// L2A state machine
// ---------------------------------------------------------------------------

/// States of the L2A algorithm.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum L2AState {
    /// Only one bitrate available – always return no-change.
    OneBitrate,
    /// Start-up: use throughput-based selection until buffer is primed.
    Startup,
    /// Steady-state: use the online learning adaptation logic.
    Steady,
}

// ---------------------------------------------------------------------------
// Per-media-type bookkeeping
// ---------------------------------------------------------------------------

/// Mutable state tracked per media type (video, audio, …).
#[derive(Clone, Debug)]
pub struct L2AMediaState {
    pub state: L2AState,
    pub current_representation: Option<RepresentationInfo>,
    pub placeholder_buffer: f64,
    pub most_advanced_segment_start: f64,
    pub last_segment_was_replacement: bool,
    pub last_segment_start: f64,
    pub last_segment_duration_s: f64,
    pub last_segment_request_time_ms: f64,
    pub last_segment_finish_time_ms: f64,
}

impl Default for L2AMediaState {
    fn default() -> Self {
        Self {
            state: L2AState::Startup,
            current_representation: None,
            placeholder_buffer: 0.0,
            most_advanced_segment_start: f64::NAN,
            last_segment_was_replacement: false,
            last_segment_start: f64::NAN,
            last_segment_duration_s: f64::NAN,
            last_segment_request_time_ms: f64::NAN,
            last_segment_finish_time_ms: f64::NAN,
        }
    }
}

/// Algorithm parameters tracked per media type.
#[derive(Clone, Debug)]
pub struct L2AParameters {
    /// Weight / probability vector over representations.
    pub w: Vec<f64>,
    /// Previous-step weight vector.
    pub prev_w: Vec<f64>,
    /// Lagrangian multiplier tracking buffer displacement.
    pub q: f64,
    pub segment_request_start_s: f64,
    pub segment_download_finish_s: f64,
    /// Target buffer level (seconds).
    pub b_target: f64,
}

impl Default for L2AParameters {
    fn default() -> Self {
        Self {
            w: Vec::new(),
            prev_w: Vec::new(),
            q: 0.0,
            segment_request_start_s: 0.0,
            segment_download_finish_s: 0.0,
            b_target: B_TARGET,
        }
    }
}

// ---------------------------------------------------------------------------
// L2ARule
// ---------------------------------------------------------------------------

/// Learn2Adapt low-latency ABR rule.
///
/// Interior mutability via `RefCell` is used because the `AbrRule` trait passes `&self`.
pub struct L2ARule {
    state_dict: RefCell<HashMap<String, L2AMediaState>>,
    param_dict: RefCell<HashMap<String, L2AParameters>>,
}

impl fmt::Debug for L2ARule {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("L2ARule")
            .field("state_dict", &self.state_dict)
            .field("param_dict", &self.param_dict)
            .finish()
    }
}

impl Default for L2ARule {
    fn default() -> Self {
        Self::new()
    }
}

impl L2ARule {
    /// Create a new L2A rule with empty state.
    pub fn new() -> Self {
        Self {
            state_dict: RefCell::new(HashMap::new()),
            param_dict: RefCell::new(HashMap::new()),
        }
    }

    // -- helpers ------------------------------------------------------------

    /// Ensure state and parameters exist for the given media type, returning clones.
    fn ensure_state(
        &self,
        media_type: &str,
        representations: &[RepresentationInfo],
    ) -> (L2AMediaState, L2AParameters) {
        let mut states = self.state_dict.borrow_mut();
        let mut params = self.param_dict.borrow_mut();

        if !states.contains_key(media_type) {
            let num_reps = representations.len();
            let initial_state = if num_reps <= 1 {
                L2AMediaState {
                    state: L2AState::OneBitrate,
                    ..Default::default()
                }
            } else {
                L2AMediaState::default()
            };
            states.insert(media_type.to_owned(), initial_state);

            let mut p = L2AParameters::default();
            p.w = vec![0.0; num_reps];
            p.prev_w = vec![0.0; num_reps];
            params.insert(media_type.to_owned(), p);
        }

        let st = states.get(media_type).unwrap().clone();
        let pm = params.get(media_type).unwrap().clone();
        (st, pm)
    }

    /// Write back state and parameters after computation.
    fn commit(
        &self,
        media_type: &str,
        state: L2AMediaState,
        params: L2AParameters,
    ) {
        self.state_dict
            .borrow_mut()
            .insert(media_type.to_owned(), state);
        self.param_dict
            .borrow_mut()
            .insert(media_type.to_owned(), params);
    }

    // -- startup ------------------------------------------------------------

    fn handle_startup(
        &self,
        context: &RulesContext,
        l2a_state: &mut L2AMediaState,
        l2a_params: &mut L2AParameters,
    ) -> SwitchRequest {
        let safe_tp = context.safe_throughput;
        if !(safe_tp > 0.0) {
            return SwitchRequest::no_change();
        }

        let rep = match context.get_optimal_representation_for_bitrate(safe_tp) {
            Some(r) => r.clone(),
            None => return SwitchRequest::no_change(),
        };

        l2a_state.current_representation = Some(rep.clone());

        // Transition to steady when buffer >= B_target and we know segment duration
        if !l2a_state.last_segment_duration_s.is_nan()
            && context.buffer_level >= l2a_params.b_target
        {
            l2a_state.state = L2AState::Steady;
            l2a_params.q = VL;

            // Initialise prev_w as one-hot for current representation
            let reps = &context.available_representations;
            l2a_params.prev_w = vec![0.0; reps.len()];
            l2a_params.w = vec![0.0; reps.len()];
            for (i, r) in reps.iter().enumerate() {
                if r.quality_index == rep.quality_index {
                    l2a_params.prev_w[i] = 1.0;
                }
            }
        }

        SwitchRequest {
            representation: Some(rep),
            priority: Priority::Default,
            reason: Some(SwitchReason {
                throughput: Some(safe_tp),
                message: "L2A startup: throughput-based".into(),
                ..Default::default()
            }),
            rule: Some("L2ARule".into()),
        }
    }

    // -- steady -------------------------------------------------------------

    fn handle_steady(
        &self,
        context: &RulesContext,
        l2a_state: &mut L2AMediaState,
        l2a_params: &mut L2AParameters,
    ) -> SwitchRequest {
        let reps = &context.available_representations;
        let num_reps = reps.len();
        if num_reps == 0 {
            return SwitchRequest::no_change();
        }

        let throughput = context.throughput.max(MIN_THROUGHPUT);
        let playback_rate = if context.playback_rate > 0.0 {
            context.playback_rate
        } else {
            1.0
        };

        // Segment duration V
        let v = if context.fragment_duration > 0.0 {
            context.fragment_duration
        } else if !l2a_state.last_segment_duration_s.is_nan()
            && l2a_state.last_segment_duration_s > 0.0
        {
            l2a_state.last_segment_duration_s
        } else {
            4.0
        };

        // Step size: alpha = max(HORIZON^1, VL * sqrt(HORIZON))
        let alpha = HORIZON.max(VL * HORIZON.sqrt());

        // Ensure w/prev_w vectors are properly sized
        if l2a_params.w.len() != num_reps {
            l2a_params.w.resize(num_reps, 0.0);
        }
        if l2a_params.prev_w.len() != num_reps {
            l2a_params.prev_w.resize(num_reps, 0.0);
        }

        // Main adaptation: Lagrangian descent
        for i in 0..num_reps {
            let bitrate_kbit = reps[i].bitrate_in_kbit;
            let sign: f64 = if playback_rate * bitrate_kbit > throughput {
                -1.0
            } else {
                1.0
            };
            l2a_params.w[i] = l2a_params.prev_w[i]
                + sign * (v / (2.0 * alpha))
                    * ((l2a_params.q + VL) * (playback_rate * bitrate_kbit / throughput));
        }

        // Euclidean projection onto probability simplex
        l2a_params.w = euclidean_projection(&l2a_params.w);

        // diff = w - prev_w; then prev_w = w
        let mut diff = vec![0.0; num_reps];
        for i in 0..num_reps {
            diff[i] = l2a_params.w[i] - l2a_params.prev_w[i];
            l2a_params.prev_w[i] = l2a_params.w[i];
        }

        // Update Lagrangian multiplier Q
        let bandwidths: Vec<f64> = reps.iter().map(|r| r.bandwidth as f64).collect();
        let q_update = l2a_params.q - v
            + v * playback_rate
                * (dot(&bandwidths, &l2a_params.prev_w) + dot(&bandwidths, &diff))
                / throughput;
        l2a_params.q = q_update.max(0.0);

        // Quality = argmin |bandwidth[i] - dot(w, bandwidths)|
        let weighted_bw = dot(&l2a_params.w, &bandwidths);
        let mut best_idx = 0usize;
        let mut best_diff = f64::MAX;
        for (i, bw) in bandwidths.iter().enumerate() {
            let d = (*bw - weighted_bw).abs();
            if d < best_diff {
                best_diff = d;
                best_idx = i;
            }
        }

        let mut selected_idx = best_idx;

        // Cautious stepwise ascent: only step up by 1 if throughput supports it
        if let Some(ref cur_rep) = l2a_state.current_representation {
            if selected_idx > cur_rep.quality_index {
                let next_idx = cur_rep.quality_index + 1;
                if next_idx < num_reps && reps[next_idx].bitrate_in_kbit <= throughput {
                    selected_idx = next_idx;
                } else {
                    // Can't go up – stay at current
                    selected_idx = cur_rep.quality_index;
                }
            }
        }

        let selected_rep = reps
            .get(selected_idx)
            .cloned()
            .unwrap_or_else(|| reps[0].clone());

        // Anti over-estimation: if selected bitrate >= throughput, re-calibrate Q
        if selected_rep.bitrate_in_kbit >= throughput {
            l2a_params.q = REACT * VL.max(l2a_params.q);
        }

        l2a_state.current_representation = Some(selected_rep.clone());

        SwitchRequest {
            representation: Some(selected_rep),
            priority: Priority::Default,
            reason: Some(SwitchReason {
                throughput: Some(throughput),
                message: "L2A steady: online learning".into(),
                ..Default::default()
            }),
            rule: Some("L2ARule".into()),
        }
    }
}

// ---------------------------------------------------------------------------
// AbrRule implementation
// ---------------------------------------------------------------------------

impl AbrRule for L2ARule {
    fn get_max_index(&self, context: &RulesContext) -> SwitchRequest {
        let media_type = context.media_type.as_str().to_owned();
        let reps = &context.available_representations;

        if reps.is_empty() {
            return SwitchRequest::no_change();
        }

        let (mut l2a_state, mut l2a_params) = self.ensure_state(&media_type, reps);

        let result = match l2a_state.state {
            L2AState::OneBitrate => SwitchRequest::no_change(),
            L2AState::Startup => {
                self.handle_startup(context, &mut l2a_state, &mut l2a_params)
            }
            L2AState::Steady => {
                self.handle_steady(context, &mut l2a_state, &mut l2a_params)
            }
        };

        self.commit(&media_type, l2a_state, l2a_params);
        result
    }

    fn name(&self) -> &str {
        "L2ARule"
    }

    fn reset(&mut self) {
        self.state_dict.borrow_mut().clear();
        self.param_dict.borrow_mut().clear();
    }
}

// ---------------------------------------------------------------------------
// Maths helpers (public for testing)
// ---------------------------------------------------------------------------

/// Dot product of two equal-length slices.
pub fn dot(a: &[f64], b: &[f64]) -> f64 {
    a.iter().zip(b.iter()).map(|(x, y)| x * y).sum()
}

/// Euclidean projection of a vector onto the probability simplex
/// `Dn = { x ∈ ℝⁿ : x >= 0, Σx = 1 }`.
///
/// Algorithm: <http://arxiv.org/abs/1101.6081>
pub fn euclidean_projection(arr: &[f64]) -> Vec<f64> {
    let m = arr.len();
    if m == 0 {
        return vec![];
    }

    // Keep a copy for the final projection (sorting is on a separate vec)
    let original: Vec<f64> = arr.to_vec();
    let mut sorted: Vec<f64> = arr.to_vec();
    // NaN values sort to the end (treated as smallest) to ensure deterministic ordering.
    sorted.sort_by(|a, b| b.partial_cmp(a).unwrap_or(if a.is_nan() {
        std::cmp::Ordering::Greater
    } else {
        std::cmp::Ordering::Less
    }));

    let mut tmpsum = 0.0_f64;
    let mut tmax = 0.0_f64;
    let mut found = false;

    for ii in 0..m - 1 {
        tmpsum += sorted[ii];
        let t = (tmpsum - 1.0) / (ii as f64 + 1.0);
        if t >= sorted[ii + 1] {
            tmax = t;
            found = true;
            break;
        }
    }

    if !found {
        tmax = (tmpsum + sorted[m - 1] - 1.0) / m as f64;
    }

    original.iter().map(|&v| (v - tmax).max(0.0)).collect()
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use crate::streaming::rules::rules_context::{BufferState, MediaType, RulesContext};
    use crate::streaming::rules::switch_request::RepresentationInfo;

    fn make_rep(idx: usize, bw: u64) -> RepresentationInfo {
        RepresentationInfo {
            quality_index: idx,
            bandwidth: bw,
            bitrate_in_kbit: bw as f64 / 1000.0,
            media_type: "video".into(),
            id: Some(format!("{}", idx)),
            absolute_index: idx,
        }
    }

    fn make_reps() -> Vec<RepresentationInfo> {
        vec![
            make_rep(0, 500_000),
            make_rep(1, 1_000_000),
            make_rep(2, 2_000_000),
            make_rep(3, 4_000_000),
        ]
    }

    fn base_context() -> RulesContext {
        RulesContext {
            media_type: MediaType::Video,
            available_representations: make_reps(),
            throughput: 3000.0,
            safe_throughput: 2500.0,
            buffer_level: 0.5,
            fragment_duration: 2.0,
            playback_rate: 1.0,
            current_representation: Some(make_rep(1, 1_000_000)),
            ..Default::default()
        }
    }

    // -- ONE_BITRATE --------------------------------------------------------

    #[test]
    fn one_bitrate_returns_no_change() {
        let rule = L2ARule::new();
        let ctx = RulesContext {
            media_type: MediaType::Video,
            available_representations: vec![make_rep(0, 500_000)],
            throughput: 3000.0,
            safe_throughput: 2500.0,
            ..Default::default()
        };
        let req = rule.get_max_index(&ctx);
        assert!(req.representation.is_none(), "single bitrate → no change");
    }

    // -- STARTUP ------------------------------------------------------------

    #[test]
    fn startup_uses_throughput_based_selection() {
        let rule = L2ARule::new();
        let ctx = base_context();
        let req = rule.get_max_index(&ctx);
        let rep = req.representation.expect("should select a representation");
        // safe_throughput = 2500 kbit → should pick quality 2 (2000 kbit)
        assert_eq!(rep.quality_index, 2);
        assert_eq!(req.rule.as_deref(), Some("L2ARule"));
    }

    #[test]
    fn startup_with_zero_throughput_returns_no_change() {
        let rule = L2ARule::new();
        let mut ctx = base_context();
        ctx.safe_throughput = 0.0;
        let req = rule.get_max_index(&ctx);
        assert!(req.representation.is_none());
    }

    // -- STARTUP → STEADY transition ----------------------------------------

    #[test]
    fn startup_to_steady_transition() {
        let rule = L2ARule::new();

        // First call: startup, buffer too low for transition
        let ctx = base_context();
        let _ = rule.get_max_index(&ctx);

        // Manually prime the state for transition
        {
            let mut states = rule.state_dict.borrow_mut();
            let st = states.get_mut("video").unwrap();
            st.last_segment_duration_s = 2.0;
        }

        // Second call: buffer >= B_target → should transition
        let mut ctx2 = base_context();
        ctx2.buffer_level = 2.0; // > B_TARGET (1.5)
        let req = rule.get_max_index(&ctx2);
        assert!(req.representation.is_some());

        // State should now be Steady
        let states = rule.state_dict.borrow();
        let st = states.get("video").unwrap();
        assert_eq!(st.state, L2AState::Steady);

        // Parameters: Q should be initialised to VL
        let params = rule.param_dict.borrow();
        let pm = params.get("video").unwrap();
        assert!((pm.q - VL).abs() < 1e-9, "Q should be VL after transition");

        // prev_w should be a one-hot vector
        let sum: f64 = pm.prev_w.iter().sum();
        assert!((sum - 1.0).abs() < 1e-9, "prev_w should sum to 1");
    }

    // -- STEADY state -------------------------------------------------------

    #[test]
    fn steady_state_produces_valid_switch() {
        let rule = L2ARule::new();
        let reps = make_reps();
        let num = reps.len();

        // Force into steady state
        {
            let mut states = rule.state_dict.borrow_mut();
            states.insert(
                "video".into(),
                L2AMediaState {
                    state: L2AState::Steady,
                    current_representation: Some(reps[1].clone()),
                    last_segment_duration_s: 2.0,
                    ..Default::default()
                },
            );
            let mut params = rule.param_dict.borrow_mut();
            let mut prev_w = vec![0.0; num];
            prev_w[1] = 1.0; // one-hot at index 1
            params.insert(
                "video".into(),
                L2AParameters {
                    w: vec![0.0; num],
                    prev_w,
                    q: VL,
                    ..Default::default()
                },
            );
        }

        let ctx = base_context();
        let req = rule.get_max_index(&ctx);

        let rep = req.representation.expect("steady state should return a rep");
        assert!(
            rep.quality_index < make_reps().len(),
            "quality index should be valid"
        );

        // Weight vector should now be a valid probability distribution
        let params = rule.param_dict.borrow();
        let pm = params.get("video").unwrap();
        let sum: f64 = pm.w.iter().sum();
        assert!(
            (sum - 1.0).abs() < 1e-6,
            "w should sum to ~1 after projection, got {}",
            sum
        );
        for &wi in &pm.w {
            assert!(wi >= -1e-9, "w entries should be non-negative");
        }
    }

    #[test]
    fn steady_state_weight_updates() {
        let rule = L2ARule::new();
        let reps = make_reps();
        let num = reps.len();

        let mut prev_w = vec![0.0; num];
        prev_w[1] = 1.0;
        let initial_prev_w = prev_w.clone();

        {
            let mut states = rule.state_dict.borrow_mut();
            states.insert(
                "video".into(),
                L2AMediaState {
                    state: L2AState::Steady,
                    current_representation: Some(reps[1].clone()),
                    last_segment_duration_s: 2.0,
                    ..Default::default()
                },
            );
            let mut params = rule.param_dict.borrow_mut();
            params.insert(
                "video".into(),
                L2AParameters {
                    w: vec![0.0; num],
                    prev_w,
                    q: VL,
                    ..Default::default()
                },
            );
        }

        let ctx = base_context();
        let _ = rule.get_max_index(&ctx);

        let params = rule.param_dict.borrow();
        let pm = params.get("video").unwrap();
        // prev_w should have changed from its initial one-hot state
        assert_ne!(pm.prev_w, initial_prev_w, "weights should be updated");
    }

    // -- Euclidean projection -----------------------------------------------

    #[test]
    fn euclidean_projection_basic() {
        let input = vec![0.5, 0.3, 0.2];
        let proj = euclidean_projection(&input);
        let sum: f64 = proj.iter().sum();
        assert!(
            (sum - 1.0).abs() < 1e-9,
            "projection should sum to 1, got {}",
            sum
        );
        for &v in &proj {
            assert!(v >= 0.0, "entries should be non-negative");
        }
    }

    #[test]
    fn euclidean_projection_already_on_simplex() {
        let input = vec![0.25, 0.25, 0.25, 0.25];
        let proj = euclidean_projection(&input);
        let sum: f64 = proj.iter().sum();
        assert!((sum - 1.0).abs() < 1e-9);
        for (i, &v) in proj.iter().enumerate() {
            assert!(
                (v - 0.25).abs() < 1e-9,
                "element {} should be 0.25, got {}",
                i,
                v
            );
        }
    }

    #[test]
    fn euclidean_projection_negative_entries() {
        let input = vec![2.0, -1.0, 0.5, -0.5];
        let proj = euclidean_projection(&input);
        let sum: f64 = proj.iter().sum();
        assert!(
            (sum - 1.0).abs() < 1e-9,
            "projection should sum to 1, got {}",
            sum
        );
        for &v in &proj {
            assert!(v >= -1e-12, "entries should be non-negative");
        }
    }

    #[test]
    fn euclidean_projection_empty() {
        assert!(euclidean_projection(&[]).is_empty());
    }

    #[test]
    fn euclidean_projection_single() {
        let proj = euclidean_projection(&[5.0]);
        assert_eq!(proj.len(), 1);
        assert!((proj[0] - 1.0).abs() < 1e-9);
    }

    // -- dot product --------------------------------------------------------

    #[test]
    fn dot_product_basic() {
        assert!((dot(&[1.0, 2.0, 3.0], &[4.0, 5.0, 6.0]) - 32.0).abs() < 1e-9);
    }

    #[test]
    fn dot_product_empty() {
        assert!((dot(&[], &[])).abs() < 1e-9);
    }

    // -- cautious stepwise ascent -------------------------------------------

    #[test]
    fn cautious_stepwise_ascent() {
        // When algorithm wants to jump from index 0 to index 3 but throughput
        // only supports index 1, it should step up by exactly one.
        let rule = L2ARule::new();
        let reps = make_reps();
        let num = reps.len();

        // Start at index 0, set weights to strongly favour index 3
        let mut prev_w = vec![0.0; num];
        prev_w[3] = 1.0;

        {
            let mut states = rule.state_dict.borrow_mut();
            states.insert(
                "video".into(),
                L2AMediaState {
                    state: L2AState::Steady,
                    current_representation: Some(reps[0].clone()),
                    last_segment_duration_s: 2.0,
                    ..Default::default()
                },
            );
            let mut params = rule.param_dict.borrow_mut();
            params.insert(
                "video".into(),
                L2AParameters {
                    w: vec![0.0; num],
                    prev_w,
                    q: VL,
                    ..Default::default()
                },
            );
        }

        // throughput = 1500 kbit → supports reps 0 (500) and 1 (1000)
        let mut ctx = base_context();
        ctx.throughput = 1500.0;
        ctx.current_representation = Some(reps[0].clone());

        let req = rule.get_max_index(&ctx);
        let rep = req.representation.expect("should have a rep");
        assert!(
            rep.quality_index <= 1,
            "cautious ascent: jumped from 0 but should not go above 1, got {}",
            rep.quality_index
        );
    }

    // -- reset --------------------------------------------------------------

    #[test]
    fn reset_clears_state() {
        let mut rule = L2ARule::new();
        let ctx = base_context();
        let _ = rule.get_max_index(&ctx);

        assert!(!rule.state_dict.borrow().is_empty());
        rule.reset();
        assert!(rule.state_dict.borrow().is_empty());
        assert!(rule.param_dict.borrow().is_empty());
    }
}
