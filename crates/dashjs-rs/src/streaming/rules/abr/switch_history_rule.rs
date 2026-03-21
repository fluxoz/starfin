//! Port of `dash.js/src/streaming/rules/abr/SwitchHistoryRule.js`.
use crate::streaming::rules::rules_context::RulesContext;
use crate::streaming::rules::switch_request::SwitchRequest;
use crate::streaming::rules::AbrRule;
#[derive(Clone, Debug, Default)]
pub struct SwitchHistoryRule;
impl AbrRule for SwitchHistoryRule {
    fn get_max_index(&self, _context: &RulesContext) -> SwitchRequest { SwitchRequest::no_change() }
    fn name(&self) -> &str { "SwitchHistoryRule" }
}
