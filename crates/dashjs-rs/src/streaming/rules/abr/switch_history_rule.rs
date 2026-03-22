//! Port of `dash.js/src/streaming/rules/abr/SwitchHistoryRule.js`.
//!
//! Prevents rapid quality oscillation by tracking switch history and
//! recommending a lower quality when the drop ratio exceeds a threshold.

use std::cell::RefCell;

use crate::streaming::rules::rules_context::RulesContext;
use crate::streaming::rules::switch_request::{Priority, SwitchReason, SwitchRequest};
use crate::streaming::rules::switch_request_history::SwitchRequestHistory;
use crate::streaming::rules::AbrRule;

const DEFAULT_SAMPLE_SIZE: u32 = 8;
const DEFAULT_SWITCH_PERCENTAGE_THRESHOLD: f64 = 0.075;

/// Prevents rapid quality oscillation by analysing switch history.
///
/// When the ratio of quality drops to total switches exceeds
/// [`SWITCH_PERCENTAGE_THRESHOLD`](DEFAULT_SWITCH_PERCENTAGE_THRESHOLD) over a
/// sliding window of [`SAMPLE_SIZE`](DEFAULT_SAMPLE_SIZE) events, the rule
/// recommends dropping one quality level.
#[derive(Clone, Debug)]
pub struct SwitchHistoryRule {
    sample_size: u32,
    switch_percentage_threshold: f64,
    history: RefCell<SwitchRequestHistory>,
}

impl Default for SwitchHistoryRule {
    fn default() -> Self {
        Self::new()
    }
}

impl SwitchHistoryRule {
    /// Create a new rule with default parameters from dash.js settings.
    pub fn new() -> Self {
        Self {
            sample_size: DEFAULT_SAMPLE_SIZE,
            switch_percentage_threshold: DEFAULT_SWITCH_PERCENTAGE_THRESHOLD,
            history: RefCell::new(SwitchRequestHistory::new()),
        }
    }

    /// Create a rule with custom parameters.
    pub fn with_params(sample_size: u32, switch_percentage_threshold: f64) -> Self {
        Self {
            sample_size,
            switch_percentage_threshold,
            history: RefCell::new(SwitchRequestHistory::new()),
        }
    }

    /// Access the underlying switch-request history (e.g. to push events).
    pub fn history(&self) -> &RefCell<SwitchRequestHistory> {
        &self.history
    }

    /// Convenience: record a quality switch for external callers.
    pub fn push_switch(
        &self,
        stream_id: &str,
        media_type: &str,
        representation_id: &str,
        was_dropped: bool,
    ) {
        self.history
            .borrow_mut()
            .push(stream_id, media_type, representation_id, was_dropped);
    }
}

impl AbrRule for SwitchHistoryRule {
    fn get_max_index(&self, context: &RulesContext) -> SwitchRequest {
        let reps = &context.available_representations;
        if reps.is_empty() {
            return SwitchRequest::no_change();
        }

        let history = self.history.borrow();
        let switch_requests = match history
            .get_switch_requests(&context.stream_id, context.media_type.as_str())
        {
            Some(sr) => sr,
            None => return SwitchRequest::no_change(),
        };

        let mut total_drops: u32 = 0;
        let mut total_no_drops: u32 = 0;

        for (i, rep) in reps.iter().enumerate() {
            let rep_id = match &rep.id {
                Some(id) => id.as_str(),
                None => continue,
            };

            if let Some(entry) = switch_requests.get(rep_id) {
                total_drops += entry.drops;
                total_no_drops += entry.no_drops;

                if total_drops + total_no_drops >= self.sample_size
                    && total_no_drops > 0
                    && (total_drops as f64 / total_no_drops as f64)
                        > self.switch_percentage_threshold
                {
                    // If this representation itself had drops and it's not the
                    // lowest quality, recommend one level below.
                    let chosen = if entry.drops > 0 && i > 0 {
                        &reps[i - 1]
                    } else {
                        &reps[i]
                    };

                    return SwitchRequest {
                        representation: Some(chosen.clone()),
                        priority: Priority::Strong,
                        reason: Some(SwitchReason {
                            message: format!(
                                "SwitchHistoryRule: drops={total_drops} noDrops={total_no_drops} \
                                 ratio={:.3}",
                                total_drops as f64 / total_no_drops as f64
                            ),
                            ..Default::default()
                        }),
                        rule: Some("SwitchHistoryRule".into()),
                    };
                }
            }
        }

        SwitchRequest::no_change()
    }

    fn name(&self) -> &str {
        "SwitchHistoryRule"
    }

    fn reset(&mut self) {
        self.history.borrow_mut().reset();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::streaming::rules::rules_context::MediaType;
    use crate::streaming::rules::switch_request::RepresentationInfo;

    fn make_reps(n: usize) -> Vec<RepresentationInfo> {
        (0..n)
            .map(|i| RepresentationInfo {
                quality_index: i,
                bandwidth: (i as u64 + 1) * 500_000,
                bitrate_in_kbit: (i as f64 + 1.0) * 500.0,
                media_type: "video".into(),
                id: Some(format!("rep{i}")),
                absolute_index: i,
            })
            .collect()
    }

    fn make_context(reps: Vec<RepresentationInfo>) -> RulesContext {
        RulesContext {
            media_type: MediaType::Video,
            stream_id: "s1".into(),
            available_representations: reps,
            ..Default::default()
        }
    }

    #[test]
    fn no_history_returns_no_change() {
        let rule = SwitchHistoryRule::new();
        let ctx = make_context(make_reps(4));
        let result = rule.get_max_index(&ctx);
        assert!(result.representation.is_none());
    }

    #[test]
    fn below_threshold_returns_no_change() {
        let rule = SwitchHistoryRule::new();
        // Push 1 drop and 9 no-drops → ratio = 1/9 ≈ 0.111 but total = 10 ≥ 8
        // However we need the drops on a specific rep to trigger.
        // Actually: drops=1, no_drops=9, ratio=0.111 > 0.075 but let's ensure
        // it only triggers on the right rep.

        // Push history where drops ratio is low: 0 drops, 10 no-drops for rep0
        for _ in 0..10 {
            rule.push_switch("s1", "video", "rep0", false);
        }
        let ctx = make_context(make_reps(4));
        let result = rule.get_max_index(&ctx);
        // 0 drops / 10 no_drops = 0.0 which is ≤ 0.075
        assert!(result.representation.is_none());
    }

    #[test]
    fn above_threshold_returns_lower_quality() {
        let rule = SwitchHistoryRule::new();
        // Accumulate on rep2: 3 drops, 6 no_drops → total 9 ≥ 8, ratio 3/6=0.5 > 0.075
        for _ in 0..3 {
            rule.push_switch("s1", "video", "rep2", true);
        }
        for _ in 0..6 {
            rule.push_switch("s1", "video", "rep2", false);
        }

        let ctx = make_context(make_reps(4));
        let result = rule.get_max_index(&ctx);
        assert!(result.representation.is_some());
        // rep2 has drops > 0 and i=2 > 0, so it should recommend rep1 (i-1)
        let rep = result.representation.unwrap();
        assert_eq!(rep.quality_index, 1);
        assert_eq!(result.priority, Priority::Strong);
    }

    #[test]
    fn lowest_quality_with_drops_returns_same_quality() {
        let rule = SwitchHistoryRule::new();
        // Accumulate on rep0 (lowest, i=0): 5 drops, 5 no_drops
        // ratio = 5/5 = 1.0 > 0.075
        for _ in 0..5 {
            rule.push_switch("s1", "video", "rep0", true);
        }
        for _ in 0..5 {
            rule.push_switch("s1", "video", "rep0", false);
        }

        let ctx = make_context(make_reps(4));
        let result = rule.get_max_index(&ctx);
        assert!(result.representation.is_some());
        // rep0 has drops > 0 but i=0, so it returns reps[0] (same quality)
        let rep = result.representation.unwrap();
        assert_eq!(rep.quality_index, 0);
    }

    #[test]
    fn reset_clears_history() {
        let mut rule = SwitchHistoryRule::new();
        for _ in 0..10 {
            rule.push_switch("s1", "video", "rep2", true);
        }
        rule.reset();

        let ctx = make_context(make_reps(4));
        let result = rule.get_max_index(&ctx);
        assert!(result.representation.is_none());
    }

    #[test]
    fn custom_params() {
        let rule = SwitchHistoryRule::with_params(4, 0.5);
        // 2 drops, 3 no_drops on rep1 → total 5 ≥ 4, ratio 2/3 ≈ 0.667 > 0.5
        for _ in 0..2 {
            rule.push_switch("s1", "video", "rep1", true);
        }
        for _ in 0..3 {
            rule.push_switch("s1", "video", "rep1", false);
        }

        let ctx = make_context(make_reps(4));
        let result = rule.get_max_index(&ctx);
        assert!(result.representation.is_some());
        // rep1 has drops > 0, i=1 > 0 → recommend rep0
        let rep = result.representation.unwrap();
        assert_eq!(rep.quality_index, 0);
    }

    #[test]
    fn empty_representations_returns_no_change() {
        let rule = SwitchHistoryRule::new();
        let ctx = make_context(vec![]);
        let result = rule.get_max_index(&ctx);
        assert!(result.representation.is_none());
    }

    #[test]
    fn accumulated_across_representations() {
        let rule = SwitchHistoryRule::new();
        // Spread across rep0 and rep1: rep0 has 2 no_drops, rep1 has 2 drops + 5 no_drops
        // After rep0: total_drops=0, total_no_drops=2 → total=2 < 8
        // After rep1: total_drops=2, total_no_drops=7 → total=9 ≥ 8, ratio=2/7≈0.286 > 0.075
        for _ in 0..2 {
            rule.push_switch("s1", "video", "rep0", false);
        }
        for _ in 0..2 {
            rule.push_switch("s1", "video", "rep1", true);
        }
        for _ in 0..5 {
            rule.push_switch("s1", "video", "rep1", false);
        }

        let ctx = make_context(make_reps(4));
        let result = rule.get_max_index(&ctx);
        assert!(result.representation.is_some());
        // Triggered at rep1 (i=1), rep1 has drops > 0 and i > 0 → recommend rep0
        let rep = result.representation.unwrap();
        assert_eq!(rep.quality_index, 0);
    }
}
