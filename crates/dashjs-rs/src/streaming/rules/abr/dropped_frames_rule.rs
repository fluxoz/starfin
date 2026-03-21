//! Port of `dash.js/src/streaming/rules/abr/DroppedFramesRule.js`.
use crate::streaming::rules::rules_context::RulesContext;
use crate::streaming::rules::switch_request::{Priority, SwitchReason, SwitchRequest};
use crate::streaming::rules::AbrRule;
const DROPPED_PERCENTAGE_FORBID: f64 = 0.15;
#[derive(Clone, Debug, Default)]
pub struct DroppedFramesRule;
impl AbrRule for DroppedFramesRule {
    fn get_max_index(&self, context: &RulesContext) -> SwitchRequest {
        if context.total_frames == 0 { return SwitchRequest::no_change(); }
        let ratio = context.dropped_frames_total as f64 / context.total_frames as f64;
        if ratio <= DROPPED_PERCENTAGE_FORBID { return SwitchRequest::no_change(); }
        if let Some(first) = context.available_representations.first() {
            SwitchRequest { representation: Some(first.clone()), priority: Priority::Weak,
                reason: Some(SwitchReason { throughput: None, latency: None, buffer_level: None,
                    message: format!("DroppedFramesRule: {:.1}% dropped", ratio * 100.0),
                    ..Default::default() }),
                rule: Some("DroppedFramesRule".into()) }
        } else { SwitchRequest::no_change() }
    }
    fn name(&self) -> &str { "DroppedFramesRule" }
}
