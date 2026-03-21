//! Port of `dash.js/src/streaming/rules/abr/AbandonRequestsRule.js`.
use crate::streaming::rules::rules_context::RulesContext;
use crate::streaming::rules::switch_request::SwitchRequest;
use crate::streaming::rules::AbandonRule;
#[derive(Clone, Debug, Default)]
pub struct AbandonRequestsRule;
impl AbandonRule for AbandonRequestsRule {
    fn should_abandon(&self, _context: &RulesContext) -> SwitchRequest { SwitchRequest::no_change() }
    fn name(&self) -> &str { "AbandonRequestsRule" }
}
