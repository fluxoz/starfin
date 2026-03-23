//! Port of `dash.js/src/streaming/rules/abr/`.
//!
//! Adaptive bitrate rule implementations and the rules collection orchestrator.

pub mod abandon_requests_rule;
pub mod abr_rules_collection;
pub mod bola_rule;
pub mod dropped_frames_rule;
pub mod insufficient_buffer_rule;
pub mod l2a_rule;
pub mod lolp;
pub mod switch_history_rule;
pub mod throughput_rule;

pub use abandon_requests_rule::AbandonRequestsRule;
pub use abr_rules_collection::AbrRulesCollection;
pub use bola_rule::BolaRule;
pub use dropped_frames_rule::DroppedFramesRule;
pub use insufficient_buffer_rule::InsufficientBufferRule;
pub use l2a_rule::L2ARule;
pub use lolp::LolpRule;
pub use switch_history_rule::SwitchHistoryRule;
pub use throughput_rule::ThroughputRule;
