//! Port of `dash.js/src/dash/utils/TimelineConverter.js`.
//!
//! Converts between presentation time, media time, and wall clock time.

use chrono::{DateTime, FixedOffset, Utc};

use crate::dash::parser::matchers::parse_datetime;

/// Timeline converter for DASH manifest time calculations.
///
/// Handles conversions between presentation time, media time, and wall-clock
/// time. Accounts for client/server time drift and availability windows.
#[derive(Clone, Debug)]
pub struct TimelineConverter {
    /// Offset in seconds between client and server clocks.
    /// Positive means server is ahead of client.
    pub client_server_time_shift: f64,
    /// Offset used when calculating availability from timeline-based segments.
    pub timeline_anchor_availability_offset: f64,
}

impl Default for TimelineConverter {
    fn default() -> Self {
        Self::new()
    }
}

impl TimelineConverter {
    pub fn new() -> Self {
        Self {
            client_server_time_shift: 0.0,
            timeline_anchor_availability_offset: 0.0,
        }
    }

    /// Get the client/server time offset in seconds.
    pub fn get_client_time_offset(&self) -> f64 {
        self.client_server_time_shift
    }

    /// Set the client/server time offset in seconds.
    pub fn set_client_time_offset(&mut self, value: f64) {
        self.client_server_time_shift = value;
    }

    /// Returns a "now" reference time (as Unix millis) for comparing segment availability.
    pub fn get_client_reference_time(&self) -> f64 {
        let now_ms = Utc::now().timestamp_millis() as f64;
        now_ms - (self.timeline_anchor_availability_offset * 1000.0)
            + (self.client_server_time_shift * 1000.0)
    }

    /// Calculate availability start time from presentation end time.
    ///
    /// For dynamic manifests: ASAST = AST + (presentationEndTime - ATO)
    /// For static manifests: returns availability start time of the MPD.
    pub fn calc_availability_start_time_from_presentation_time(
        &self,
        presentation_end_time: f64,
        availability_start_time: Option<&str>,
        availability_time_offset: f64,
        is_dynamic: bool,
    ) -> Option<f64> {
        if is_dynamic {
            let ast = self.parse_ast(availability_start_time)?;
            let ast_ms = ast.timestamp_millis() as f64;
            Some(ast_ms + (presentation_end_time - availability_time_offset) * 1000.0)
        } else {
            let ast = self.parse_ast(availability_start_time)?;
            Some(ast.timestamp_millis() as f64)
        }
    }

    /// Calculate availability end time from presentation end time.
    ///
    /// For dynamic manifests with TSBD: SAET = AST + presentationEndTime + TSBD + duration
    /// For static manifests or infinite TSBD: returns f64::INFINITY.
    pub fn calc_availability_end_time_from_presentation_time(
        &self,
        presentation_end_time: f64,
        availability_start_time: Option<&str>,
        time_shift_buffer_depth: Option<f64>,
        is_dynamic: bool,
    ) -> f64 {
        if is_dynamic {
            if let Some(tsbd) = time_shift_buffer_depth {
                if tsbd.is_finite() {
                    if let Some(ast) = self.parse_ast(availability_start_time) {
                        let ast_ms = ast.timestamp_millis() as f64;
                        return ast_ms + (presentation_end_time + tsbd) * 1000.0;
                    }
                }
            }
        }
        f64::INFINITY
    }

    /// Convert wall-clock time to presentation time.
    pub fn calc_presentation_time_from_wall_time(
        &self,
        wall_time_ms: f64,
        availability_start_time: Option<&str>,
    ) -> f64 {
        if let Some(ast) = self.parse_ast(availability_start_time) {
            let ast_ms = ast.timestamp_millis() as f64;
            (wall_time_ms - ast_ms + self.client_server_time_shift * 1000.0) / 1000.0
        } else {
            0.0
        }
    }

    /// Convert media time to presentation time.
    ///
    /// presentationTime = mediaTime + (periodStart - presentationTimeOffset)
    pub fn calc_presentation_time_from_media_time(
        &self,
        media_time: f64,
        period_start: f64,
        presentation_time_offset: f64,
    ) -> f64 {
        media_time + (period_start - presentation_time_offset)
    }

    /// Convert presentation time to media time.
    ///
    /// mediaTime = presentationTime - periodStart + presentationTimeOffset
    pub fn calc_media_time_from_presentation_time(
        &self,
        presentation_time: f64,
        period_start: f64,
        presentation_time_offset: f64,
    ) -> f64 {
        presentation_time - period_start + presentation_time_offset
    }

    /// Calculate wall time for displaying a segment (dynamic manifests).
    pub fn calc_wall_time_for_segment(
        &self,
        availability_start_time_ms: f64,
        presentation_start_time: f64,
        suggested_presentation_delay: f64,
        is_dynamic: bool,
    ) -> Option<f64> {
        if is_dynamic {
            let display_start_time = presentation_start_time + suggested_presentation_delay;
            Some(availability_start_time_ms + display_start_time * 1000.0)
        } else {
            None
        }
    }

    /// Calculate period-relative time from MPD-relative time.
    pub fn calc_period_relative_time_from_mpd_relative_time(
        &self,
        mpd_relative_time: f64,
        period_start: f64,
    ) -> f64 {
        mpd_relative_time - period_start
    }

    /// Reset the converter to initial settings.
    pub fn reset(&mut self) {
        self.client_server_time_shift = 0.0;
        self.timeline_anchor_availability_offset = 0.0;
    }

    fn parse_ast(&self, ast: Option<&str>) -> Option<DateTime<FixedOffset>> {
        ast.and_then(parse_datetime)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_new_converter() {
        let tc = TimelineConverter::new();
        assert_eq!(tc.get_client_time_offset(), 0.0);
    }

    #[test]
    fn test_media_to_presentation_time() {
        let tc = TimelineConverter::new();
        let pt = tc.calc_presentation_time_from_media_time(10.0, 5.0, 0.0);
        assert!((pt - 15.0).abs() < 0.001);
    }

    #[test]
    fn test_presentation_to_media_time() {
        let tc = TimelineConverter::new();
        let mt = tc.calc_media_time_from_presentation_time(15.0, 5.0, 0.0);
        assert!((mt - 10.0).abs() < 0.001);
    }

    #[test]
    fn test_media_presentation_round_trip() {
        let tc = TimelineConverter::new();
        let period_start = 100.0;
        let pto = 5.0;
        let original_media_time = 42.0;
        let pt = tc.calc_presentation_time_from_media_time(original_media_time, period_start, pto);
        let mt = tc.calc_media_time_from_presentation_time(pt, period_start, pto);
        assert!((mt - original_media_time).abs() < 0.001);
    }

    #[test]
    fn test_period_relative_time() {
        let tc = TimelineConverter::new();
        let rel = tc.calc_period_relative_time_from_mpd_relative_time(35.0, 10.0);
        assert!((rel - 25.0).abs() < 0.001);
    }

    #[test]
    fn test_with_presentation_time_offset() {
        let tc = TimelineConverter::new();
        let pt = tc.calc_presentation_time_from_media_time(10.0, 5.0, 3.0);
        assert!((pt - 12.0).abs() < 0.001);
        let mt = tc.calc_media_time_from_presentation_time(12.0, 5.0, 3.0);
        assert!((mt - 10.0).abs() < 0.001);
    }

    #[test]
    fn test_availability_start_time_static() {
        let tc = TimelineConverter::new();
        let ast_str = "2023-01-01T00:00:00Z";
        let result = tc.calc_availability_start_time_from_presentation_time(
            10.0,
            Some(ast_str),
            0.0,
            false,
        );
        assert!(result.is_some());
        // For static, returns AST directly
        let expected_ms = 1672531200000.0;
        assert!((result.unwrap() - expected_ms).abs() < 1.0);
    }

    #[test]
    fn test_availability_start_time_dynamic() {
        let tc = TimelineConverter::new();
        let ast_str = "2023-01-01T00:00:00Z";
        let result = tc.calc_availability_start_time_from_presentation_time(
            10.0,
            Some(ast_str),
            2.0,
            true,
        );
        assert!(result.is_some());
        // AST + (10 - 2) * 1000 = AST + 8000
        let ast_ms = 1672531200000.0;
        assert!((result.unwrap() - (ast_ms + 8000.0)).abs() < 1.0);
    }

    #[test]
    fn test_availability_end_time_static() {
        let tc = TimelineConverter::new();
        let result =
            tc.calc_availability_end_time_from_presentation_time(10.0, None, None, false);
        assert!(result.is_infinite());
    }

    #[test]
    fn test_availability_end_time_dynamic_with_tsbd() {
        let tc = TimelineConverter::new();
        let ast_str = "2023-01-01T00:00:00Z";
        let result = tc.calc_availability_end_time_from_presentation_time(
            10.0,
            Some(ast_str),
            Some(30.0),
            true,
        );
        assert!(result.is_finite());
        let ast_ms = 1672531200000.0;
        assert!((result - (ast_ms + (10.0 + 30.0) * 1000.0)).abs() < 1.0);
    }

    #[test]
    fn test_reset() {
        let mut tc = TimelineConverter::new();
        tc.set_client_time_offset(5.0);
        tc.timeline_anchor_availability_offset = 3.0;
        tc.reset();
        assert_eq!(tc.get_client_time_offset(), 0.0);
        assert_eq!(tc.timeline_anchor_availability_offset, 0.0);
    }
}
