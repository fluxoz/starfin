//! Port of `dash.js/src/streaming/rules/abr/ABRRulesCollection.js`.
use crate::streaming::rules::{AbandonRule, AbrRule};
use crate::streaming::rules::rules_context::RulesContext;
use crate::streaming::rules::switch_request::{Priority, SwitchRequest};
pub struct AbrRulesCollection {
    pub quality_rules: Vec<Box<dyn AbrRule>>,
    pub abandon_rules: Vec<Box<dyn AbandonRule>>,
}
impl AbrRulesCollection {
    pub fn new_default() -> Self {
        use super::{bola_rule::BolaRule, throughput_rule::ThroughputRule,
            insufficient_buffer_rule::InsufficientBufferRule, dropped_frames_rule::DroppedFramesRule,
            abandon_requests_rule::AbandonRequestsRule};
        Self {
            quality_rules: vec![
                Box::new(ThroughputRule::default()), Box::new(BolaRule::new(12.0)),
                Box::new(InsufficientBufferRule), Box::new(DroppedFramesRule),
            ],
            abandon_rules: vec![Box::new(AbandonRequestsRule)],
        }
    }
    pub fn get_max_quality(&self, context: &RulesContext) -> SwitchRequest {
        let mut best = SwitchRequest::no_change();
        for rule in &self.quality_rules {
            let req = rule.get_max_index(context);
            if req.representation.is_some() {
                let dominated = match (&best.representation, &req.representation) {
                    (None, Some(_)) => true,
                    (Some(_), Some(_)) => match (&req.priority, &best.priority) {
                        (Priority::Strong, Priority::Weak) | (Priority::Strong, Priority::Default) => true,
                        (Priority::Default, Priority::Weak) => true,
                        _ => req.representation.as_ref().map(|r| r.quality_index).unwrap_or(0)
                             < best.representation.as_ref().map(|r| r.quality_index).unwrap_or(0),
                    },
                    _ => false,
                };
                if dominated { best = req; }
            }
        }
        best
    }
    pub fn should_abandon(&self, context: &RulesContext) -> SwitchRequest {
        for rule in &self.abandon_rules {
            let req = rule.should_abandon(context);
            if req.representation.is_some() { return req; }
        }
        SwitchRequest::no_change()
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    use crate::streaming::rules::rules_context::{BufferState, MediaType};
    use crate::streaming::rules::switch_request::RepresentationInfo;
    #[test]
    fn default_collection_returns_result() {
        let col = AbrRulesCollection::new_default();
        let ctx = RulesContext {
            media_type: MediaType::Video,
            current_representation: None,
            available_representations: vec![
                RepresentationInfo { quality_index: 0, bandwidth: 500_000, bitrate_in_kbit: 500.0, media_type: "video".into(), id: Some("0".into()), absolute_index: 0 },
                RepresentationInfo { quality_index: 1, bandwidth: 1_000_000, bitrate_in_kbit: 1000.0, media_type: "video".into(), id: Some("1".into()), absolute_index: 1 },
            ],
            buffer_level: 15.0, throughput: 1_500_000.0, latency: 0.05,
            is_dynamic: false, dropped_frames_total: 0, total_frames: 1000,
            schedule_controller_state: BufferState::Loaded,
            ..Default::default()
        };
        let req = col.get_max_quality(&ctx);
        assert!(req.representation.is_some());
    }
}
