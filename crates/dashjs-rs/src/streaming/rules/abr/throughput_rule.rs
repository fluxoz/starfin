//! Port of `dash.js/src/streaming/rules/abr/ThroughputRule.js`.
//!
//! Selects the highest representation whose bitrate fits within the measured throughput
//! (applying a bandwidth safety factor).

use crate::streaming::rules::rules_context::{BufferState, RulesContext};
use crate::streaming::rules::switch_request::{Priority, SwitchReason, SwitchRequest};
use crate::streaming::rules::AbrRule;

/// Default bandwidth safety factor (from dash.js Settings.js `bandwidthSafetyFactor`).
const DEFAULT_BANDWIDTH_SAFETY_FACTOR: f64 = 0.9;

/// Throughput-based ABR rule.
///
/// Reference: `dash.js/src/streaming/rules/abr/ThroughputRule.js`
#[derive(Clone, Debug)]
pub struct ThroughputRule {
    /// Multiplicative safety margin applied to measured throughput.
    pub bandwidth_safety_factor: f64,
}

impl Default for ThroughputRule {
    fn default() -> Self {
        Self {
            bandwidth_safety_factor: DEFAULT_BANDWIDTH_SAFETY_FACTOR,
        }
    }
}

impl ThroughputRule {
    pub fn new(bandwidth_safety_factor: f64) -> Self {
        Self {
            bandwidth_safety_factor,
        }
    }
}

impl AbrRule for ThroughputRule {
    fn get_max_index(&self, context: &RulesContext) -> SwitchRequest {
        let reps = &context.available_representations;
        if reps.is_empty() {
            return SwitchRequest::no_change();
        }

        // Only run when buffer is loaded or stream is dynamic
        if context.schedule_controller_state != BufferState::Loaded && !context.is_dynamic {
            return SwitchRequest::no_change();
        }

        let safe_throughput = context.throughput * self.bandwidth_safety_factor;

        // Find highest quality whose bitrate <= safe throughput
        let mut best_idx = 0;
        for (i, rep) in reps.iter().enumerate() {
            if (rep.bandwidth as f64) <= safe_throughput {
                best_idx = i;
            }
        }

        let chosen = &reps[best_idx];

        SwitchRequest {
            representation: Some(chosen.clone()),
            priority: Priority::Strong,
            reason: Some(SwitchReason {
                throughput: Some(context.throughput),
                latency: Some(context.latency),
                buffer_level: Some(context.buffer_level),
                message: format!(
                    "ThroughputRule: safe_throughput={:.0} chose bitrate={}",
                    safe_throughput, chosen.bandwidth
                ),
            }),
            rule: Some("ThroughputRule".into()),
        }
    }

    fn name(&self) -> &str {
        "ThroughputRule"
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::streaming::rules::rules_context::MediaType;
    use crate::streaming::rules::switch_request::RepresentationInfo;

    fn make_reps() -> Vec<RepresentationInfo> {
        vec![
            RepresentationInfo { quality_index: 0, bandwidth: 500_000, media_type: "video".into() },
            RepresentationInfo { quality_index: 1, bandwidth: 1_000_000, media_type: "video".into() },
            RepresentationInfo { quality_index: 2, bandwidth: 2_000_000, media_type: "video".into() },
            RepresentationInfo { quality_index: 3, bandwidth: 4_000_000, media_type: "video".into() },
        ]
    }

    fn ctx(throughput: f64, state: BufferState) -> RulesContext {
        RulesContext {
            media_type: MediaType::Video,
            current_representation: Some(RepresentationInfo { quality_index: 0, bandwidth: 500_000, media_type: "video".into() }),
            available_representations: make_reps(),
            buffer_level: 15.0,
            throughput,
            latency: 0.05,
            is_dynamic: false,
            dropped_frames_total: 0,
            total_frames: 1000,
            schedule_controller_state: state,
        }
    }

    #[test]
    fn selects_highest_fitting_quality() {
        let rule = ThroughputRule::default();
        // 2.5 Mbps * 0.9 = 2.25 Mbps => quality 2 (2 Mbps)
        let req = rule.get_max_index(&ctx(2_500_000.0, BufferState::Loaded));
        assert_eq!(req.representation.as_ref().unwrap().quality_index, 2);
    }

    #[test]
    fn low_throughput_selects_lowest() {
        let rule = ThroughputRule::default();
        // 400kbps * 0.9 = 360 kbps < 500 kbps lowest
        let req = rule.get_max_index(&ctx(400_000.0, BufferState::Loaded));
        assert_eq!(req.representation.as_ref().unwrap().quality_index, 0);
    }

    #[test]
    fn no_change_when_buffer_empty_and_static() {
        let rule = ThroughputRule::default();
        let req = rule.get_max_index(&ctx(5_000_000.0, BufferState::Empty));
        assert!(req.representation.is_none());
    }

    #[test]
    fn runs_when_dynamic_even_if_empty() {
        let rule = ThroughputRule::default();
        let mut c = ctx(5_000_000.0, BufferState::Empty);
        c.is_dynamic = true;
        let req = rule.get_max_index(&c);
        assert!(req.representation.is_some());
    }
}
