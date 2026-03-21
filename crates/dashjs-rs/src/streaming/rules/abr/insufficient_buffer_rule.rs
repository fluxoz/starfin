//! Port of `dash.js/src/streaming/rules/abr/InsufficientBufferRule.js`.
use crate::streaming::rules::rules_context::RulesContext;
use crate::streaming::rules::switch_request::{Priority, SwitchReason, SwitchRequest};
use crate::streaming::rules::AbrRule;
const INSUFFICIENT_BUFFER_SAFETY_FACTOR: f64 = 0.5;
#[derive(Clone, Debug, Default)]
pub struct InsufficientBufferRule;
impl AbrRule for InsufficientBufferRule {
    fn get_max_index(&self, context: &RulesContext) -> SwitchRequest {
        if context.buffer_level > INSUFFICIENT_BUFFER_SAFETY_FACTOR { return SwitchRequest::no_change(); }
        if let Some(first) = context.available_representations.first() {
            SwitchRequest { representation: Some(first.clone()), priority: Priority::Strong,
                reason: Some(SwitchReason { throughput: None, latency: None, buffer_level: Some(context.buffer_level),
                    message: format!("InsufficientBufferRule: buffer {:.2}s", context.buffer_level) }),
                rule: Some("InsufficientBufferRule".into()) }
        } else { SwitchRequest::no_change() }
    }
    fn name(&self) -> &str { "InsufficientBufferRule" }
}
