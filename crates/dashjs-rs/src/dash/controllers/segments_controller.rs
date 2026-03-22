//! Port of `dash.js/src/dash/controllers/SegmentsController.js` (stub).
//!
//! Manages segment generation across different addressing modes.

use crate::dash::vo::representation::Representation;
use crate::dash::vo::segment::Segment;

/// Addressing mode for segment generation.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum SegmentAddressingMode {
    Template,
    Timeline,
    List,
    Base,
}

/// Controls segment generation, dispatching to the appropriate getter
/// based on the representation's segment info type.
pub struct SegmentsController {
    is_dynamic: bool,
}

impl SegmentsController {
    pub fn new() -> Self {
        Self { is_dynamic: false }
    }

    /// Initialize with dynamic/static mode.
    pub fn initialize(&mut self, is_dynamic: bool) {
        self.is_dynamic = is_dynamic;
    }

    /// Determine the addressing mode for a representation.
    pub fn get_addressing_mode(representation: &Representation) -> Option<SegmentAddressingMode> {
        match representation.segment_info_type.as_deref() {
            Some("SegmentTemplate") => Some(SegmentAddressingMode::Template),
            Some("SegmentTimeline") => Some(SegmentAddressingMode::Timeline),
            Some("SegmentList") => Some(SegmentAddressingMode::List),
            Some("SegmentBase") | Some("BaseURL") => Some(SegmentAddressingMode::Base),
            _ => None,
        }
    }

    /// Get a segment by its index from the representation's pre-parsed segments
    /// (for SegmentBase addressing).
    pub fn get_segment_by_index_from_segments(
        representation: &Representation,
        index: u32,
    ) -> Option<Segment> {
        let segments = representation.segments.as_ref()?;
        if (index as usize) < segments.len() {
            let seg = &segments[index as usize];
            if seg.index == Some(index) {
                return Some(seg.clone());
            }
        }
        segments.iter().find(|s| s.index == Some(index)).cloned()
    }

    /// Whether this is a dynamic manifest.
    pub fn is_dynamic(&self) -> bool {
        self.is_dynamic
    }
}

impl Default for SegmentsController {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_addressing_mode() {
        let mut rep = Representation::default();
        rep.segment_info_type = Some("SegmentTemplate".to_string());
        assert_eq!(
            SegmentsController::get_addressing_mode(&rep),
            Some(SegmentAddressingMode::Template)
        );

        rep.segment_info_type = Some("SegmentTimeline".to_string());
        assert_eq!(
            SegmentsController::get_addressing_mode(&rep),
            Some(SegmentAddressingMode::Timeline)
        );

        rep.segment_info_type = Some("SegmentList".to_string());
        assert_eq!(
            SegmentsController::get_addressing_mode(&rep),
            Some(SegmentAddressingMode::List)
        );

        rep.segment_info_type = Some("SegmentBase".to_string());
        assert_eq!(
            SegmentsController::get_addressing_mode(&rep),
            Some(SegmentAddressingMode::Base)
        );

        rep.segment_info_type = None;
        assert_eq!(SegmentsController::get_addressing_mode(&rep), None);
    }

    #[test]
    fn test_initialize() {
        let mut ctrl = SegmentsController::new();
        assert!(!ctrl.is_dynamic());
        ctrl.initialize(true);
        assert!(ctrl.is_dynamic());
    }

    #[test]
    fn test_get_segment_by_index_from_segments() {
        let segments = vec![
            Segment {
                index: Some(0),
                media: Some("seg-0.m4s".to_string()),
                ..Segment::default()
            },
            Segment {
                index: Some(1),
                media: Some("seg-1.m4s".to_string()),
                ..Segment::default()
            },
        ];
        let rep = Representation {
            segments: Some(segments),
            ..Representation::default()
        };

        let seg = SegmentsController::get_segment_by_index_from_segments(&rep, 0).unwrap();
        assert_eq!(seg.media.as_deref(), Some("seg-0.m4s"));

        let seg = SegmentsController::get_segment_by_index_from_segments(&rep, 1).unwrap();
        assert_eq!(seg.media.as_deref(), Some("seg-1.m4s"));

        assert!(SegmentsController::get_segment_by_index_from_segments(&rep, 5).is_none());
    }
}
