//! Port of `dash.js/src/dash/utils/SegmentBaseGetter.js`.
//!
//! Handles SegmentBase addressing using pre-parsed segment arrays.

use crate::dash::vo::representation::{MediaFinishedInformation, Representation};
use crate::dash::vo::segment::Segment;

use super::timeline_converter::TimelineConverter;

/// Retrieves segments from a pre-parsed segment array (SegmentBase / sidx).
pub struct SegmentBaseGetter {
    pub timeline_converter: TimelineConverter,
}

impl SegmentBaseGetter {
    pub fn new(timeline_converter: TimelineConverter) -> Self {
        Self { timeline_converter }
    }

    /// Returns information about the last media segment.
    pub fn get_media_finished_information(
        &self,
        representation: &Representation,
    ) -> MediaFinishedInformation {
        let count = representation
            .segments
            .as_ref()
            .map_or(0, |s| s.len() as u64);

        MediaFinishedInformation {
            number_of_segments: count,
            media_time_of_last_signaled_segment: None,
        }
    }

    /// Get a segment by its index from the pre-parsed segment array.
    pub fn get_segment_by_index(
        &self,
        representation: &Representation,
        index: u32,
    ) -> Option<Segment> {
        let segments = representation.segments.as_ref()?;
        let len = segments.len();

        // Fast path: direct index access
        if (index as usize) < len {
            let seg = &segments[index as usize];
            if seg.index == Some(index) {
                return Some(seg.clone());
            }
        }

        // Linear search fallback
        segments.iter().find(|s| s.index == Some(index)).cloned()
    }

    /// Get a segment by requested presentation time.
    pub fn get_segment_by_time(
        &self,
        representation: &Representation,
        requested_time: f64,
    ) -> Option<Segment> {
        let index = self.get_index_by_time(representation, requested_time)?;
        self.get_segment_by_index(representation, index)
    }

    /// Find the segment index that contains the given presentation time.
    fn get_index_by_time(&self, representation: &Representation, time: f64) -> Option<u32> {
        let segments = representation.segments.as_ref()?;

        for seg in segments {
            let ft = seg.presentation_start_time.unwrap_or(0.0);
            let fd = seg.duration.unwrap_or(0.0);

            if fd <= 0.0 {
                continue;
            }

            let epsilon = fd / 2.0;
            if (time + epsilon) >= ft && (time - epsilon) < (ft + fd) {
                return seg.index;
            }
        }

        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn make_rep_with_segments() -> Representation {
        let segments = vec![
            Segment {
                index: Some(0),
                presentation_start_time: Some(0.0),
                duration: Some(2.0),
                media: Some("seg-0.m4s".to_string()),
                ..Segment::default()
            },
            Segment {
                index: Some(1),
                presentation_start_time: Some(2.0),
                duration: Some(2.0),
                media: Some("seg-1.m4s".to_string()),
                ..Segment::default()
            },
            Segment {
                index: Some(2),
                presentation_start_time: Some(4.0),
                duration: Some(2.0),
                media: Some("seg-2.m4s".to_string()),
                ..Segment::default()
            },
        ];

        Representation {
            id: Some("1".to_string()),
            segments: Some(segments),
            ..Representation::default()
        }
    }

    #[test]
    fn test_get_segment_by_index() {
        let getter = SegmentBaseGetter::new(TimelineConverter::new());
        let rep = make_rep_with_segments();

        let seg = getter.get_segment_by_index(&rep, 1).unwrap();
        assert_eq!(seg.index, Some(1));
        assert_eq!(seg.media.as_deref(), Some("seg-1.m4s"));
    }

    #[test]
    fn test_get_segment_by_index_out_of_bounds() {
        let getter = SegmentBaseGetter::new(TimelineConverter::new());
        let rep = make_rep_with_segments();

        let seg = getter.get_segment_by_index(&rep, 10);
        assert!(seg.is_none());
    }

    #[test]
    fn test_get_segment_by_time() {
        let getter = SegmentBaseGetter::new(TimelineConverter::new());
        let rep = make_rep_with_segments();

        let seg = getter.get_segment_by_time(&rep, 3.0).unwrap();
        assert_eq!(seg.index, Some(1));
    }

    #[test]
    fn test_get_segment_by_time_edge() {
        let getter = SegmentBaseGetter::new(TimelineConverter::new());
        let rep = make_rep_with_segments();

        // At time 2.0, with epsilon=fd/2=1.0:
        // seg 0: (2.0+1.0) >= 0.0 && (2.0-1.0) < 2.0 → true, so seg 0 matches first
        let seg = getter.get_segment_by_time(&rep, 2.0).unwrap();
        assert_eq!(seg.index, Some(0));
    }

    #[test]
    fn test_media_finished_information() {
        let getter = SegmentBaseGetter::new(TimelineConverter::new());
        let rep = make_rep_with_segments();

        let info = getter.get_media_finished_information(&rep);
        assert_eq!(info.number_of_segments, 3);
    }

    #[test]
    fn test_empty_segments() {
        let getter = SegmentBaseGetter::new(TimelineConverter::new());
        let rep = Representation::default();

        let seg = getter.get_segment_by_index(&rep, 0);
        assert!(seg.is_none());

        let info = getter.get_media_finished_information(&rep);
        assert_eq!(info.number_of_segments, 0);
    }
}
