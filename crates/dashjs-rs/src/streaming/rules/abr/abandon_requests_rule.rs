//! Port of `dash.js/src/streaming/rules/abr/AbandonRequestsRule.js`.
//!
//! Monitors in-progress segment downloads and recommends abandoning a request
//! when the measured throughput is too low to finish before a buffer underrun.

use crate::streaming::rules::rules_context::RulesContext;
use crate::streaming::rules::switch_request::{Priority, SwitchReason, SwitchRequest};
use crate::streaming::rules::AbandonRule;
use std::cell::RefCell;
use std::collections::HashSet;

// ---------------------------------------------------------------------------
// Download-progress types
// ---------------------------------------------------------------------------

/// A single trace sample recorded during a segment download.
#[derive(Clone, Debug, Default)]
pub struct TraceEntry {
    /// Bytes downloaded in this trace interval.
    pub bytes: u64,
    /// Duration of this trace interval in milliseconds.
    pub duration_ms: f64,
}

/// Snapshot of an in-progress segment download, fed to the abandon rule.
#[derive(Clone, Debug, Default)]
pub struct DownloadProgress {
    /// Request / segment index (used to deduplicate abandon decisions).
    pub request_index: u64,
    /// Total expected size of the segment in bytes.
    pub bytes_total: u64,
    /// Bytes received so far.
    pub bytes_loaded: u64,
    /// Wall-clock time since the request started, in milliseconds.
    pub elapsed_ms: f64,
    /// Per-interval download traces collected by the XHR/fetch layer.
    pub traces: Vec<TraceEntry>,
    /// Duration of the segment in seconds (e.g. 4.0).
    pub segment_duration: f64,
    /// Bitrate of the representation currently being downloaded, in kbit/s.
    pub current_bitrate_kbit: f64,
}

// ---------------------------------------------------------------------------
// Rule
// ---------------------------------------------------------------------------

/// Adaptive-bitrate rule that abandons slow segment downloads.
///
/// Mirrors the logic of `AbandonRequestsRule` in dash.js:
/// * skips evaluation when the buffer is healthy or too few samples exist;
/// * estimates throughput from download traces (excluding the first sample to
///   account for connection-setup latency);
/// * abandons the download when the estimated completion time exceeds a
///   configurable multiple of the segment duration **and** switching to a
///   lower quality would save bytes.
#[derive(Clone, Debug)]
pub struct AbandonRequestsRule {
    /// Request indices already marked as abandoned (interior-mutable so that
    /// `should_abandon_download` can work through `&self`).
    abandon_dict: RefCell<HashSet<u64>>,
    /// Buffer level (in seconds) above which we never abandon.
    stable_buffer_time: f64,
    /// Multiplier applied to `segment_duration` — if estimated download time
    /// is below `segment_duration * abandon_duration_multiplier` the download
    /// is considered fast enough.
    abandon_duration_multiplier: f64,
    /// Minimum number of trace entries required before the rule activates.
    min_throughput_samples: usize,
    /// Minimum elapsed download time (ms) before the rule activates.
    min_segment_download_time_ms: f64,
}

impl Default for AbandonRequestsRule {
    fn default() -> Self {
        Self {
            abandon_dict: RefCell::new(HashSet::new()),
            stable_buffer_time: 12.0,
            abandon_duration_multiplier: 1.8,
            min_throughput_samples: 5,
            min_segment_download_time_ms: 500.0,
        }
    }
}

impl AbandonRequestsRule {
    pub fn new() -> Self {
        Self::default()
    }

    /// Evaluate whether an in-progress download should be abandoned.
    ///
    /// This is the main entry-point callers should use when download-progress
    /// information is available.  The trait method [`AbandonRule::should_abandon`]
    /// always returns *no-change* because [`RulesContext`] alone does not carry
    /// download progress.
    pub fn should_abandon_download(
        &self,
        context: &RulesContext,
        progress: &DownloadProgress,
    ) -> SwitchRequest {
        // 1. Already abandoned this index — nothing more to do.
        if self.abandon_dict.borrow().contains(&progress.request_index) {
            return SwitchRequest::no_change();
        }

        // 2. Buffer is healthy — no need to panic.
        if context.buffer_level > self.stable_buffer_time {
            return SwitchRequest::no_change();
        }

        // 3. Not enough data to make a reliable decision.
        if progress.traces.len() < self.min_throughput_samples
            || progress.elapsed_ms < self.min_segment_download_time_ms
            || progress.bytes_loaded >= progress.bytes_total
        {
            return SwitchRequest::no_change();
        }

        // 4. Calculate throughput, skipping the first trace to exclude
        //    connection-setup / DNS latency.
        let first = &progress.traces[0];
        let total_bytes: u64 = progress.traces.iter().map(|t| t.bytes).sum();
        let total_duration_ms: f64 = progress.traces.iter().map(|t| t.duration_ms).sum();

        let downloaded_bytes = total_bytes.saturating_sub(first.bytes);
        let download_time_ms = total_duration_ms - first.duration_ms;

        if download_time_ms <= 0.0 {
            return SwitchRequest::no_change();
        }

        let throughput_kbit = (8.0 * downloaded_bytes as f64) / download_time_ms;

        // 5. Estimate total download time for the current segment.
        if throughput_kbit <= 0.0 {
            return SwitchRequest::no_change();
        }
        let estimated_time_s =
            (progress.bytes_total as f64 * 8.0 / throughput_kbit) / 1000.0;

        // 6. Download is fast enough — keep going.
        if estimated_time_s
            < progress.segment_duration * self.abandon_duration_multiplier
        {
            return SwitchRequest::no_change();
        }

        // 7. Already at the lowest quality — nowhere to step down.
        if context.is_playing_at_lowest_quality() {
            return SwitchRequest::no_change();
        }

        // 8. Find the optimal representation for the measured throughput and
        //    check whether abandoning actually saves bytes.
        if let Some(optimal_rep) =
            context.get_optimal_representation_for_bitrate(throughput_kbit)
        {
            // Only abandon if the optimal quality is strictly lower.
            if let Some(ref current) = context.current_representation {
                if optimal_rep.quality_index >= current.quality_index {
                    return SwitchRequest::no_change();
                }
            }

            let remaining_bytes =
                progress.bytes_total.saturating_sub(progress.bytes_loaded);
            let ratio = if progress.current_bitrate_kbit > 0.0 {
                optimal_rep.bitrate_in_kbit / progress.current_bitrate_kbit
            } else {
                1.0
            };
            let estimated_bytes_at_optimal =
                (progress.bytes_total as f64 * ratio) as u64;

            if remaining_bytes > estimated_bytes_at_optimal {
                self.abandon_dict
                    .borrow_mut()
                    .insert(progress.request_index);

                let mut req = SwitchRequest::new();
                req.representation = Some(optimal_rep.clone());
                req.priority = Priority::Strong;
                req.reason = Some(SwitchReason {
                    throughput: Some(throughput_kbit),
                    message: format!(
                        "AbandonRequestsRule: estimated download time {estimated_time_s:.1}s \
                         exceeds {:.1}x segment duration {:.1}s",
                        self.abandon_duration_multiplier,
                        progress.segment_duration,
                    ),
                    force_abandon: true,
                    ..Default::default()
                });
                req.rule = Some("AbandonRequestsRule".to_string());
                return req;
            }
        }

        SwitchRequest::no_change()
    }
}

// ---------------------------------------------------------------------------
// AbandonRule trait (context-only path — always returns no-change)
// ---------------------------------------------------------------------------

impl AbandonRule for AbandonRequestsRule {
    fn should_abandon(&self, _context: &RulesContext) -> SwitchRequest {
        SwitchRequest::no_change()
    }

    fn name(&self) -> &str {
        "AbandonRequestsRule"
    }

    fn reset(&mut self) {
        self.abandon_dict.borrow_mut().clear();
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use crate::streaming::rules::switch_request::RepresentationInfo;

    fn make_rep(quality_index: usize, bitrate_kbit: f64) -> RepresentationInfo {
        RepresentationInfo {
            quality_index,
            bandwidth: (bitrate_kbit * 1000.0) as u64,
            bitrate_in_kbit: bitrate_kbit,
            media_type: "video".to_string(),
            id: Some(format!("rep_{quality_index}")),
            absolute_index: quality_index,
        }
    }

    fn base_context() -> RulesContext {
        let reps = vec![
            make_rep(0, 500.0),
            make_rep(1, 1000.0),
            make_rep(2, 2000.0),
            make_rep(3, 4000.0),
        ];
        RulesContext {
            buffer_level: 5.0,
            current_representation: Some(reps[3].clone()),
            available_representations: reps,
            ..Default::default()
        }
    }

    fn slow_progress() -> DownloadProgress {
        // Simulate a very slow download at quality index 3 (4000 kbit).
        // 6 trace entries so we exceed min_throughput_samples (5).
        // Total ~600 ms elapsed, ~300 kbit/s throughput after skipping first.
        let traces: Vec<TraceEntry> = (0..6)
            .map(|_| TraceEntry {
                bytes: 5_000,
                duration_ms: 100.0,
            })
            .collect();
        DownloadProgress {
            request_index: 42,
            bytes_total: 500_000,
            bytes_loaded: 30_000,
            elapsed_ms: 600.0,
            traces,
            segment_duration: 4.0,
            current_bitrate_kbit: 4000.0,
        }
    }

    // ---- trait method always returns no-change ----

    #[test]
    fn trait_method_returns_no_change() {
        let rule = AbandonRequestsRule::new();
        let ctx = base_context();
        let req = rule.should_abandon(&ctx);
        assert!(req.representation.is_none());
    }

    // ---- buffer level high ----

    #[test]
    fn high_buffer_returns_no_change() {
        let rule = AbandonRequestsRule::new();
        let mut ctx = base_context();
        ctx.buffer_level = 20.0; // > 12.0
        let req = rule.should_abandon_download(&ctx, &slow_progress());
        assert!(req.representation.is_none());
    }

    // ---- not enough trace samples ----

    #[test]
    fn too_few_samples_returns_no_change() {
        let rule = AbandonRequestsRule::new();
        let ctx = base_context();
        let mut prog = slow_progress();
        prog.traces.truncate(3); // < 5
        let req = rule.should_abandon_download(&ctx, &prog);
        assert!(req.representation.is_none());
    }

    // ---- elapsed time too short ----

    #[test]
    fn short_elapsed_returns_no_change() {
        let rule = AbandonRequestsRule::new();
        let ctx = base_context();
        let mut prog = slow_progress();
        prog.elapsed_ms = 200.0; // < 500
        let req = rule.should_abandon_download(&ctx, &prog);
        assert!(req.representation.is_none());
    }

    // ---- download already complete ----

    #[test]
    fn complete_download_returns_no_change() {
        let rule = AbandonRequestsRule::new();
        let ctx = base_context();
        let mut prog = slow_progress();
        prog.bytes_loaded = prog.bytes_total;
        let req = rule.should_abandon_download(&ctx, &prog);
        assert!(req.representation.is_none());
    }

    // ---- download fast enough ----

    #[test]
    fn fast_download_returns_no_change() {
        let rule = AbandonRequestsRule::new();
        let ctx = base_context();
        // Make traces very fast so estimated time is short.
        let traces: Vec<TraceEntry> = (0..6)
            .map(|_| TraceEntry {
                bytes: 100_000,
                duration_ms: 10.0,
            })
            .collect();
        let prog = DownloadProgress {
            request_index: 1,
            bytes_total: 500_000,
            bytes_loaded: 100_000,
            elapsed_ms: 600.0,
            traces,
            segment_duration: 4.0,
            current_bitrate_kbit: 4000.0,
        };
        let req = rule.should_abandon_download(&ctx, &prog);
        assert!(req.representation.is_none());
    }

    // ---- already at lowest quality ----

    #[test]
    fn lowest_quality_returns_no_change() {
        let rule = AbandonRequestsRule::new();
        let mut ctx = base_context();
        // Set current to quality_index 0 (lowest).
        ctx.current_representation = Some(make_rep(0, 500.0));
        let req = rule.should_abandon_download(&ctx, &slow_progress());
        assert!(req.representation.is_none());
    }

    // ---- slow download triggers abandon ----

    #[test]
    fn slow_download_triggers_abandon() {
        let rule = AbandonRequestsRule::new();
        let ctx = base_context();
        let prog = slow_progress();
        let req = rule.should_abandon_download(&ctx, &prog);
        assert!(req.representation.is_some(), "expected abandon recommendation");
        assert_eq!(req.priority, Priority::Strong);
        assert!(req.reason.as_ref().unwrap().force_abandon);
        // Should recommend a quality lower than current (3).
        assert!(req.representation.as_ref().unwrap().quality_index < 3);
    }

    // ---- already-abandoned index returns no-change ----

    #[test]
    fn already_abandoned_returns_no_change() {
        let rule = AbandonRequestsRule::new();
        let ctx = base_context();
        let prog = slow_progress();

        // First call should trigger abandon.
        let req = rule.should_abandon_download(&ctx, &prog);
        assert!(req.representation.is_some());

        // Second call for the same index should return no-change.
        let req2 = rule.should_abandon_download(&ctx, &prog);
        assert!(req2.representation.is_none());
    }

    // ---- reset clears abandon dict ----

    #[test]
    fn reset_clears_state() {
        let mut rule = AbandonRequestsRule::new();
        let ctx = base_context();
        let prog = slow_progress();

        let req = rule.should_abandon_download(&ctx, &prog);
        assert!(req.representation.is_some());

        rule.reset();

        // After reset the same index should trigger abandon again.
        let req2 = rule.should_abandon_download(&ctx, &prog);
        assert!(req2.representation.is_some());
    }
}

