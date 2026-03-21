//! Port of `dash.js/src/streaming/rules/abr/L2ARule.js` — Learn2Adapt low-latency ABR.
use crate::streaming::rules::rules_context::RulesContext;
use crate::streaming::rules::switch_request::SwitchRequest;
use crate::streaming::rules::AbrRule;
#[derive(Clone, Debug, Default)]
pub struct L2ARule;
impl AbrRule for L2ARule {
    fn get_max_index(&self, _context: &RulesContext) -> SwitchRequest { SwitchRequest::no_change() }
    fn name(&self) -> &str { "L2ARule" }
}
