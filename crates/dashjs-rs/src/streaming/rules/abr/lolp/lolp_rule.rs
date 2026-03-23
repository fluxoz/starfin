//! Port of `dash.js/src/streaming/rules/abr/lolp/LoLpRule.js`.
//!
//! Main ABR rule for LoL+ (Low-on-Latency Plus).  Combines a Self-Organising
//! Map learning controller with a dynamic weight selector to pick the best
//! quality for low-latency live streaming.

use std::cell::RefCell;

use crate::streaming::rules::rules_context::{MediaType, RulesContext};
use crate::streaming::rules::switch_request::{Priority, SwitchReason, SwitchRequest};
use crate::streaming::rules::AbrRule;

use super::learning_abr_controller::LearningAbrController;
use super::lolp_qoe_evaluator::LoLpQoeEvaluator;
use super::lolp_weight_selector::LoLpWeightSelector;

/// Target live latency (seconds) for the Dynamic Weight Selector.
const DWS_TARGET_LATENCY: f64 = 1.5;
/// Minimum buffer level (seconds) to maintain.
const DWS_BUFFER_MIN: f64 = 0.3;

/// LoL+ ABR rule.
///
/// Port of `dash.js/src/streaming/rules/abr/lolp/LoLpRule.js`.
pub struct LolpRule {
    learning_controller: RefCell<LearningAbrController>,
    qoe_evaluator: RefCell<LoLpQoeEvaluator>,
}

impl Default for LolpRule {
    fn default() -> Self {
        Self {
            learning_controller: RefCell::new(LearningAbrController::new()),
            qoe_evaluator: RefCell::new(LoLpQoeEvaluator::new()),
        }
    }
}

impl LolpRule {
    pub fn new() -> Self {
        Self::default()
    }
}

impl AbrRule for LolpRule {
    fn get_max_index(&self, context: &RulesContext) -> SwitchRequest {
        let reps = &context.available_representations;

        // Only operate on video, and only when representations are available.
        if context.media_type != MediaType::Video || reps.is_empty() {
            return SwitchRequest::no_change();
        }

        // Need a valid throughput measurement.
        if context.throughput <= 0.0 || context.throughput.is_nan() {
            return SwitchRequest::no_change();
        }

        let current_rep = match &context.current_representation {
            Some(r) => r.clone(),
            None => reps[0].clone(),
        };

        // QoE setup parameters.
        let bandwidths: Vec<f64> = reps.iter().map(|r| r.bandwidth as f64).collect();
        let max_bitrate_kbps = bandwidths.iter().cloned().fold(0.0_f64, f64::max) / 1000.0;
        let min_bitrate_kbps = bandwidths.iter().cloned().fold(f64::INFINITY, f64::min) / 1000.0;
        let segment_duration = context.fragment_duration;

        // Estimate the rebuffer time for the segment just downloaded.
        // Without actual HTTP timing we approximate from throughput.
        let download_time =
            (current_rep.bandwidth as f64 * segment_duration) / context.throughput.max(1.0);
        let segment_rebuffer_time = f64::max(0.0, download_time - segment_duration);

        // Step 1: update the QoE evaluator.
        {
            let mut qoe = self.qoe_evaluator.borrow_mut();
            qoe.setup_per_segment_qoe(segment_duration, max_bitrate_kbps, min_bitrate_kbps);
            qoe.log_segment_metrics(
                current_rep.bandwidth as f64 / 1000.0,
                segment_rebuffer_time,
                context.latency,
                context.playback_rate,
            );
        }

        // Step 2: create the Dynamic Weight Selector (fresh per invocation,
        // matching the JS pattern of constructing it inside getSwitchRequest).
        let mut weight_selector =
            LoLpWeightSelector::new(DWS_TARGET_LATENCY, DWS_BUFFER_MIN, segment_duration);

        // Step 3: select quality via the learning controller.
        let qoe = self.qoe_evaluator.borrow();
        let mut lc = self.learning_controller.borrow_mut();
        let chosen_rep = lc.get_next_quality(
            reps,
            context.throughput,
            context.latency,
            context.buffer_level,
            context.playback_rate,
            &current_rep,
            &mut weight_selector,
            &*qoe,
        );

        match chosen_rep {
            Some(rep) => SwitchRequest {
                representation: Some(rep.clone()),
                priority: Priority::Default,
                reason: Some(SwitchReason {
                    throughput: Some(context.throughput),
                    latency: Some(context.latency),
                    buffer_level: Some(context.buffer_level),
                    message: format!(
                        "LolpRule: latency={:.3}s buffer={:.2}s chose bandwidth={}",
                        context.latency, context.buffer_level, rep.bandwidth,
                    ),
                    ..Default::default()
                }),
                rule: Some("LolpRule".into()),
            },
            None => SwitchRequest::no_change(),
        }
    }

    fn name(&self) -> &str {
        "LolpRule"
    }

    fn reset(&mut self) {
        self.learning_controller.borrow_mut().reset();
        self.qoe_evaluator.borrow_mut().reset();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::streaming::rules::rules_context::BufferState;
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

    fn make_context(throughput: f64, latency: f64, buffer: f64) -> RulesContext {
        let reps = make_reps();
        RulesContext {
            media_type: MediaType::Video,
            available_representations: reps.clone(),
            current_representation: Some(reps[1].clone()),
            throughput,
            latency,
            buffer_level: buffer,
            fragment_duration: 2.0,
            playback_rate: 1.0,
            schedule_controller_state: BufferState::Loaded,
            is_dynamic: true,
            low_latency_enabled: true,
            ..Default::default()
        }
    }

    #[test]
    fn returns_representation_for_good_conditions() {
        let rule = LolpRule::new();
        let ctx = make_context(3_000_000.0, 1.0, 8.0);
        let req = rule.get_max_index(&ctx);
        assert!(req.representation.is_some());
        assert_eq!(req.rule.as_deref(), Some("LolpRule"));
    }

    #[test]
    fn no_change_for_audio() {
        let rule = LolpRule::new();
        let mut ctx = make_context(3_000_000.0, 1.0, 8.0);
        ctx.media_type = MediaType::Audio;
        let req = rule.get_max_index(&ctx);
        assert!(req.representation.is_none());
    }

    #[test]
    fn no_change_for_zero_throughput() {
        let rule = LolpRule::new();
        let ctx = make_context(0.0, 1.0, 8.0);
        let req = rule.get_max_index(&ctx);
        assert!(req.representation.is_none());
    }

    #[test]
    fn multiple_calls_produce_valid_results() {
        let rule = LolpRule::new();
        for _ in 0..5 {
            let ctx = make_context(2_000_000.0, 1.2, 5.0);
            let req = rule.get_max_index(&ctx);
            assert!(req.representation.is_some());
        }
    }
}
