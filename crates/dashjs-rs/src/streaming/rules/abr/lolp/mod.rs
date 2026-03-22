//! LoL+ (Low-on-Latency Plus) ABR rule implementation.
//!
//! Port of `dash.js/src/streaming/rules/abr/lolp/`.
//!
//! The algorithm combines a Self-Organising Map (SOM) learning controller with
//! a Dynamic Weight Selector (DWS) to pick the best quality level for
//! low-latency live streaming.

pub mod learning_abr_controller;
pub mod lolp_qoe_evaluator;
pub mod lolp_rule;
pub mod lolp_weight_selector;

pub use lolp_rule::LolpRule;

// ---------------------------------------------------------------------------
// Shared data types used by both the learning controller and weight selector.
// ---------------------------------------------------------------------------

use crate::streaming::rules::switch_request::RepresentationInfo;

/// Per-neuron network / QoE state maintained by the SOM.
#[derive(Clone, Debug, Default)]
pub struct NeuronState {
    /// Normalised throughput (bandwidth / L2-norm of all bandwidths).
    pub throughput: f64,
    /// Normalised latency.
    pub latency: f64,
    /// Rebuffer duration (seconds).
    pub rebuffer: f64,
    /// Normalised absolute bitrate switch magnitude.
    pub switch: f64,
}

/// A single SOM neuron — one per available representation.
#[derive(Clone, Debug)]
pub struct SomNeuron {
    pub representation: RepresentationInfo,
    pub state: NeuronState,
}
