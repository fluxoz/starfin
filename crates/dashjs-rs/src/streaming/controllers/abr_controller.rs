//! Port of `dash.js/src/streaming/controllers/AbrController.js`.
//!
//! Orchestrates ABR rules to select appropriate quality levels.

use crate::streaming::rules::abr::AbrRulesCollection;
use crate::streaming::rules::rules_context::RulesContext;
use crate::streaming::rules::switch_request::SwitchRequest;

/// Adaptive Bitrate controller.
pub struct AbrController {
    rules: AbrRulesCollection,
    _initialized: bool,
}

impl Default for AbrController {
    fn default() -> Self {
        Self {
            rules: AbrRulesCollection::new_default(),
            _initialized: false,
        }
    }
}

impl AbrController {
    pub fn new() -> Self { Self::default() }

    pub fn get_quality_for(&self, context: &RulesContext) -> SwitchRequest {
        self.rules.get_max_quality(context)
    }

    pub fn should_abandon_request(&self, context: &RulesContext) -> SwitchRequest {
        self.rules.should_abandon(context)
    }

    pub fn reset(&mut self) {
        self.rules = AbrRulesCollection::new_default();
        self._initialized = false;
    }
}
