//! Port of `dash.js/src/streaming/rules/`.
//!
//! ABR (Adaptive Bitrate) rules and supporting types.

pub mod abr;
pub mod dropped_frames_history;
pub mod rules_context;
pub mod switch_request;
pub mod switch_request_history;

pub use dropped_frames_history::DroppedFramesHistory;
pub use rules_context::{BufferState, MediaType, RulesContext};
pub use switch_request::{Priority, RepresentationInfo, SwitchReason, SwitchRequest};
pub use switch_request_history::SwitchRequestHistory;

/// Trait implemented by all ABR quality-switch rules.
pub trait AbrRule {
    /// Return the recommended quality switch, or `SwitchRequest::no_change()`.
    fn get_max_index(&self, context: &RulesContext) -> SwitchRequest;

    /// Rule name for logging and identification.
    fn name(&self) -> &str;

    /// Reset internal state.
    fn reset(&mut self) {}
}

/// Trait for abandon-fragment rules (e.g. AbandonRequestsRule).
pub trait AbandonRule {
    /// Determine if the current download should be abandoned.
    fn should_abandon(&self, context: &RulesContext) -> SwitchRequest;

    /// Rule name for logging.
    fn name(&self) -> &str;

    /// Reset internal state.
    fn reset(&mut self) {}
}
