//! Port of `dash.js/src/streaming/rules/abr/BolaRule.js`.
//!
//! BOLA (Buffer Occupancy based Lyapunov Algorithm) for adaptive bitrate selection.
//! See <http://arxiv.org/abs/1601.06748> for the algorithm description.

use crate::streaming::rules::rules_context::{BufferState, MediaType, RulesContext};
use crate::streaming::rules::switch_request::{
    Priority, RepresentationInfo, SwitchReason, SwitchRequest,
};
use crate::streaming::rules::AbrRule;

// BOLA states
const BOLA_STATE_ONE_BITRATE: u8 = 0;
const BOLA_STATE_STARTUP: u8 = 1;
const BOLA_STATE_STEADY: u8 = 2;

const MINIMUM_BUFFER_S: f64 = 10.0;
const MINIMUM_BUFFER_PER_BITRATE_LEVEL_S: f64 = 2.0;
const PLACEHOLDER_BUFFER_DECAY: f64 = 0.99;

/// Internal BOLA parameters computed from representations.
#[derive(Clone, Debug)]
struct BolaParams {
    gp: f64,
    vp: f64,
    utilities: Vec<f64>,
}

/// Per-stream BOLA state.
#[derive(Clone, Debug)]
struct BolaState {
    state: u8,
    params: Option<BolaParams>,
    placeholder_buffer: f64,
    last_call_time_ms: Option<f64>,
    current_representation: Option<RepresentationInfo>,
}

/// BOLA ABR rule.
#[derive(Debug)]
pub struct BolaRule {
    buffer_time_default: f64,
}

impl BolaRule {
    pub fn new(buffer_time_default: f64) -> Self {
        Self { buffer_time_default }
    }

    /// Calculate BOLA parameters (Vp and gp) from representations and utilities.
    fn calculate_bola_parameters(
        buffer_time_default: f64,
        representations: &[RepresentationInfo],
        utilities: &[f64],
    ) -> Option<BolaParams> {
        if representations.is_empty() || utilities.is_empty() {
            return None;
        }

        let highest_utility_index = utilities
            .iter()
            .enumerate()
            .max_by(|(_, a), (_, b)| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal))
            .map(|(i, _)| i)
            .unwrap_or(0);

        // If only one effective bitrate level, no parameters needed.
        if highest_utility_index == 0 {
            return None;
        }

        let buffer_time = buffer_time_default
            .max(MINIMUM_BUFFER_S + MINIMUM_BUFFER_PER_BITRATE_LEVEL_S * representations.len() as f64);

        // Vp * (utilities[0] + gp - 1) === MINIMUM_BUFFER_S
        // Vp * (utilities[max] + gp - 1) === buffer_time
        let gp = (utilities[highest_utility_index] - 1.0) / (buffer_time / MINIMUM_BUFFER_S - 1.0);
        let vp = MINIMUM_BUFFER_S / gp;

        Some(BolaParams {
            gp,
            vp,
            utilities: utilities.to_vec(),
        })
    }

    /// Compute normalized log-utilities from bandwidths.
    fn compute_utilities(representations: &[RepresentationInfo]) -> Vec<f64> {
        if representations.is_empty() {
            return Vec::new();
        }
        let mut utilities: Vec<f64> = representations.iter().map(|r| (r.bandwidth as f64).ln()).collect();
        let base = utilities[0];
        for u in utilities.iter_mut() {
            *u = *u - base + 1.0;
        }
        utilities
    }

    /// The core BOLA quality selection from buffer level.
    fn get_quality_from_buffer_level(
        params: &BolaParams,
        representations: &[RepresentationInfo],
        buffer_level: f64,
    ) -> usize {
        let mut best_index = 0;
        let mut best_score = f64::NEG_INFINITY;

        for (i, rep) in representations.iter().enumerate() {
            let score = (params.vp * (params.utilities[i] - 1.0 + params.gp) - buffer_level)
                / rep.bandwidth as f64;
            if score >= best_score {
                best_score = score;
                best_index = i;
            }
        }
        best_index
    }

    /// Max buffer level for which downloading the given representation is preferred over waiting.
    fn max_buffer_level_for_quality(params: &BolaParams, quality_index: usize) -> f64 {
        params.vp * (params.utilities[quality_index] + params.gp)
    }

    /// Min buffer level for which BOLA prefers `quality_index` over any lower quality.
    fn min_buffer_level_for_quality(
        params: &BolaParams,
        representations: &[RepresentationInfo],
        quality_index: usize,
    ) -> f64 {
        let q_bitrate = representations[quality_index].bandwidth as f64;
        let q_utility = params.utilities[quality_index];
        let mut min = 0.0_f64;

        for i in (0..quality_index).rev() {
            if params.utilities[i] < q_utility {
                let i_bitrate = representations[i].bandwidth as f64;
                let i_utility = params.utilities[i];
                let level = params.vp * (params.gp + (q_bitrate * i_utility - i_bitrate * q_utility) / (q_bitrate - i_bitrate));
                min = min.max(level);
            }
        }
        min
    }

    fn get_optimal_for_bitrate<'a>(representations: &'a [RepresentationInfo], bitrate_kbps: f64) -> Option<&'a RepresentationInfo> {
        let mut best: Option<&RepresentationInfo> = None;
        for rep in representations {
            if rep.bitrate_in_kbit <= bitrate_kbps {
                match best {
                    Some(b) if b.bitrate_in_kbit >= rep.bitrate_in_kbit => {}
                    _ => best = Some(rep),
                }
            }
        }
        if best.is_none() {
            best = representations.first();
        }
        best
    }
}

impl AbrRule for BolaRule {
    fn get_max_index(&self, context: &RulesContext) -> SwitchRequest {
        let mut sr = SwitchRequest::new();
        sr.rule = Some("BolaRule".into());

        let representations = &context.available_representations;
        if representations.len() <= 1 {
            // ONE_BITRATE state: no switching possible
            return sr;
        }

        let utilities = Self::compute_utilities(representations);
        let params = match Self::calculate_bola_parameters(self.buffer_time_default, representations, &utilities) {
            Some(p) => p,
            None => return sr,
        };

        // Startup: use throughput-based selection
        if context.schedule_controller_state == BufferState::Empty
            || context.buffer_level < context.fragment_duration
        {
            if context.safe_throughput.is_nan() || context.safe_throughput <= 0.0 {
                return sr;
            }
            let rep = Self::get_optimal_for_bitrate(representations, context.safe_throughput);
            if let Some(rep) = rep {
                sr.representation = Some(rep.clone());
                sr.reason = Some(SwitchReason {
                    throughput: Some(context.safe_throughput),
                    message: format!(
                        "[BolaRule]: Startup - selecting bitrate {} kbit/s based on throughput {}",
                        rep.bitrate_in_kbit, context.safe_throughput
                    ),
                    ..Default::default()
                });
            }
            sr.priority = Priority::Default;
            return sr;
        }

        // Steady state: use buffer-based selection
        let effective_buffer = context.buffer_level;
        let quality_index = Self::get_quality_from_buffer_level(&params, representations, effective_buffer);

        // BOLA-O: avoid unsustainable oscillations
        let mut final_index = quality_index;
        if let Some(current) = &context.current_representation {
            if quality_index > current.quality_index {
                // Only increase if throughput supports it
                if let Some(tp_rep) = Self::get_optimal_for_bitrate(representations, context.safe_throughput) {
                    if quality_index > tp_rep.quality_index {
                        final_index = tp_rep.quality_index.max(current.quality_index);
                    }
                }
            }
        }

        if let Some(rep) = representations.get(final_index) {
            sr.representation = Some(rep.clone());
            sr.reason = Some(SwitchReason {
                throughput: Some(context.safe_throughput),
                buffer_level: Some(context.buffer_level),
                message: format!(
                    "[BolaRule]: Steady state - buffer {:.1}s -> quality index {}",
                    context.buffer_level, final_index
                ),
                ..Default::default()
            });
        }

        sr.priority = Priority::Default;
        sr
    }

    fn name(&self) -> &str {
        "BolaRule"
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::streaming::rules::rules_context::BufferState;

    fn make_reps() -> Vec<RepresentationInfo> {
        vec![
            RepresentationInfo { quality_index: 0, bandwidth: 500_000, bitrate_in_kbit: 500.0, media_type: "video".into(), id: Some("0".into()), absolute_index: 0 },
            RepresentationInfo { quality_index: 1, bandwidth: 1_000_000, bitrate_in_kbit: 1000.0, media_type: "video".into(), id: Some("1".into()), absolute_index: 1 },
            RepresentationInfo { quality_index: 2, bandwidth: 2_000_000, bitrate_in_kbit: 2000.0, media_type: "video".into(), id: Some("2".into()), absolute_index: 2 },
            RepresentationInfo { quality_index: 3, bandwidth: 4_000_000, bitrate_in_kbit: 4000.0, media_type: "video".into(), id: Some("3".into()), absolute_index: 3 },
            RepresentationInfo { quality_index: 4, bandwidth: 6_000_000, bitrate_in_kbit: 6000.0, media_type: "video".into(), id: Some("4".into()), absolute_index: 4 },
        ]
    }

    #[test]
    fn compute_utilities_normalized() {
        let reps = make_reps();
        let utils = BolaRule::compute_utilities(&reps);
        assert!((utils[0] - 1.0).abs() < 1e-9);
        assert!(utils[4] > utils[3]);
        assert!(utils[3] > utils[2]);
    }

    #[test]
    fn bola_parameters_calculated() {
        let reps = make_reps();
        let utils = BolaRule::compute_utilities(&reps);
        let params = BolaRule::calculate_bola_parameters(18.0, &reps, &utils);
        assert!(params.is_some());
        let p = params.unwrap();
        assert!(p.gp > 0.0);
        assert!(p.vp > 0.0);
    }

    #[test]
    fn single_bitrate_returns_none_params() {
        let reps = vec![
            RepresentationInfo { quality_index: 0, bandwidth: 500_000, bitrate_in_kbit: 500.0, media_type: "video".into(), id: Some("0".into()), absolute_index: 0 },
        ];
        let utils = BolaRule::compute_utilities(&reps);
        let params = BolaRule::calculate_bola_parameters(18.0, &reps, &utils);
        assert!(params.is_none());
    }

    #[test]
    fn quality_from_buffer_level_low_buffer_picks_low_quality() {
        let reps = make_reps();
        let utils = BolaRule::compute_utilities(&reps);
        let params = BolaRule::calculate_bola_parameters(18.0, &reps, &utils).unwrap();
        let q = BolaRule::get_quality_from_buffer_level(&params, &reps, 1.0);
        assert_eq!(q, 0, "At very low buffer, should pick lowest quality");
    }

    #[test]
    fn quality_from_buffer_level_high_buffer_picks_high_quality() {
        let reps = make_reps();
        let utils = BolaRule::compute_utilities(&reps);
        let params = BolaRule::calculate_bola_parameters(18.0, &reps, &utils).unwrap();
        let q = BolaRule::get_quality_from_buffer_level(&params, &reps, 30.0);
        assert!(q >= 3, "At high buffer, should pick high quality, got {}", q);
    }

    #[test]
    fn quality_increases_with_buffer() {
        let reps = make_reps();
        let utils = BolaRule::compute_utilities(&reps);
        let params = BolaRule::calculate_bola_parameters(18.0, &reps, &utils).unwrap();
        let q_low = BolaRule::get_quality_from_buffer_level(&params, &reps, 5.0);
        let q_high = BolaRule::get_quality_from_buffer_level(&params, &reps, 25.0);
        assert!(q_high >= q_low, "Higher buffer should give equal or higher quality");
    }

    #[test]
    fn steady_state_selects_based_on_buffer() {
        let reps = make_reps();
        let rule = BolaRule::new(18.0);
        let context = RulesContext {
            media_type: MediaType::Video,
            available_representations: reps.clone(),
            current_representation: Some(reps[2].clone()),
            buffer_level: 20.0,
            safe_throughput: 5000.0,
            throughput: 5000.0,
            fragment_duration: 4.0,
            schedule_controller_state: BufferState::Loaded,
            ..Default::default()
        };
        let sr = rule.get_max_index(&context);
        assert!(sr.representation.is_some());
        assert!(sr.representation.as_ref().unwrap().quality_index >= 2);
    }

    #[test]
    fn startup_uses_throughput() {
        let reps = make_reps();
        let rule = BolaRule::new(18.0);
        let context = RulesContext {
            media_type: MediaType::Video,
            available_representations: reps.clone(),
            buffer_level: 0.5,
            safe_throughput: 1500.0,
            throughput: 1500.0,
            fragment_duration: 4.0,
            schedule_controller_state: BufferState::Empty,
            ..Default::default()
        };
        let sr = rule.get_max_index(&context);
        assert!(sr.representation.is_some());
        let rep = sr.representation.unwrap();
        assert!(rep.bitrate_in_kbit <= 1500.0);
    }

    #[test]
    fn min_buffer_level_increases_with_quality() {
        let reps = make_reps();
        let utils = BolaRule::compute_utilities(&reps);
        let params = BolaRule::calculate_bola_parameters(18.0, &reps, &utils).unwrap();
        let min1 = BolaRule::min_buffer_level_for_quality(&params, &reps, 1);
        let min3 = BolaRule::min_buffer_level_for_quality(&params, &reps, 3);
        assert!(min3 > min1, "Higher quality needs higher min buffer");
    }

    #[test]
    fn max_buffer_level_increases_with_quality() {
        let reps = make_reps();
        let utils = BolaRule::compute_utilities(&reps);
        let params = BolaRule::calculate_bola_parameters(18.0, &reps, &utils).unwrap();
        let max0 = BolaRule::max_buffer_level_for_quality(&params, 0);
        let max4 = BolaRule::max_buffer_level_for_quality(&params, 4);
        assert!(max4 > max0, "Higher quality should have higher max buffer level");
    }
}
