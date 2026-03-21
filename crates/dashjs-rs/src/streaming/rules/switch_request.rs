//! Port of `dash.js/src/streaming/rules/SwitchRequest.js`.

/// Priority of a switch request, controlling arbitration between rules.
#[derive(Clone, Debug, PartialEq)]
pub enum Priority {
    /// Weak priority (0.0) — easily overridden.
    Weak,
    /// Default priority (0.5).
    Default,
    /// Strong priority (1.0) — overrides others.
    Strong,
}

impl Priority {
    pub fn value(&self) -> f64 {
        match self {
            Priority::Weak => 0.0,
            Priority::Default => 0.5,
            Priority::Strong => 1.0,
        }
    }
}

/// Lightweight representation info carried inside a [`SwitchRequest`].
#[derive(Clone, Debug, PartialEq)]
pub struct RepresentationInfo {
    pub quality_index: usize,
    pub bandwidth: u64,
    pub bitrate_in_kbit: f64,
    pub media_type: String,
    pub id: Option<String>,
    pub absolute_index: usize,
}

/// Reason for a quality switch.
#[derive(Clone, Debug, Default)]
pub struct SwitchReason {
    pub throughput: Option<f64>,
    pub latency: Option<f64>,
    pub buffer_level: Option<f64>,
    pub dropped_frames: Option<u32>,
    pub message: String,
    pub force_abandon: bool,
}

/// A request to switch quality, produced by an ABR rule.
#[derive(Clone, Debug)]
pub struct SwitchRequest {
    pub representation: Option<RepresentationInfo>,
    pub priority: Priority,
    pub reason: Option<SwitchReason>,
    pub rule: Option<String>,
}

impl Default for SwitchRequest {
    fn default() -> Self {
        Self {
            representation: None,
            priority: Priority::Default,
            reason: None,
            rule: None,
        }
    }
}

impl SwitchRequest {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn no_change() -> Self {
        Self::default()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_switch_request_has_no_change() {
        let sr = SwitchRequest::default();
        assert!(sr.representation.is_none());
        assert_eq!(sr.priority, Priority::Default);
    }

    #[test]
    fn priority_values() {
        assert_eq!(Priority::Weak.value(), 0.0);
        assert_eq!(Priority::Default.value(), 0.5);
        assert_eq!(Priority::Strong.value(), 1.0);
    }
}
