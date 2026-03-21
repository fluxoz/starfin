//! Port of `dash.js/src/dash/utils/TemplateSegmentsGetter.js`.
//!
//! Generates segment lists from SegmentTemplate with fixed duration.

use crate::dash::vo::representation::{MediaFinishedInformation, Representation};
use crate::dash::vo::segment::Segment;

use super::segments_utils::{get_index_based_segment, IndexBasedSegmentData};
use super::timeline_converter::TimelineConverter;

/// Generates segments based on SegmentTemplate addressing (index-based).
pub struct TemplateSegmentsGetter {
    pub timeline_converter: TimelineConverter,
    pub is_dynamic: bool,
}

impl TemplateSegmentsGetter {
    pub fn new(timeline_converter: TimelineConverter, is_dynamic: bool) -> Self {
        Self {
            timeline_converter,
            is_dynamic,
        }
    }

    /// Returns information about the last media segment.
    pub fn get_media_finished_information(
        &self,
        representation: &Representation,
    ) -> MediaFinishedInformation {
        let mut info = MediaFinishedInformation {
            number_of_segments: 0,
            media_time_of_last_signaled_segment: None,
        };

        let duration = match representation.segment_duration {
            Some(d) if !d.is_nan() && d > 0.0 => d,
            _ => {
                info.number_of_segments = 1;
                return info;
            }
        };

        let period_duration = representation
            .segment_duration
            .map(|_| {
                // In a full port, we'd access period.duration through the hierarchy.
                // For now we use segment_duration as a proxy.
                f64::INFINITY
            })
            .unwrap_or(f64::INFINITY);

        if period_duration.is_finite() {
            info.number_of_segments = (period_duration / duration).ceil() as u64;
        } else {
            info.number_of_segments = 1;
        }

        info
    }

    /// Get a segment by its index (without startNumber offset).
    pub fn get_segment_by_index(
        &self,
        representation: &Representation,
        index: u32,
        period_start: f64,
        period_duration: f64,
        availability_start_time: Option<&str>,
        time_shift_buffer_depth: Option<f64>,
        suggested_presentation_delay: f64,
    ) -> Option<Segment> {
        let segment_duration = match representation.segment_duration {
            Some(d) if !d.is_nan() && d > 0.0 => d,
            _ => return None,
        };

        let media_time = (index as f64 * segment_duration * representation.timescale as f64).round() as u64;

        let data = IndexBasedSegmentData {
            index,
            is_dynamic: self.is_dynamic,
            media_range: None,
            media_time: Some(media_time),
            media_url: representation.media.clone(),
            representation_id: representation.id.clone(),
            bandwidth: representation.bandwidth,
            start_number: representation.start_number,
            timescale: representation.timescale,
            segment_duration,
            period_start,
            period_duration,
            presentation_time_offset: representation.presentation_time_offset,
            availability_start_time: availability_start_time.map(String::from),
            availability_time_offset: representation.availability_time_offset,
            time_shift_buffer_depth,
            suggested_presentation_delay,
            timeline_converter: &self.timeline_converter,
        };

        let seg = get_index_based_segment(&data)?;

        // Check endNumber constraint
        if let Some(end_num) = representation.end_number {
            if seg.replacement_number.unwrap_or(0) > end_num {
                return None;
            }
        }

        Some(seg)
    }

    /// Get a segment by requested presentation time.
    pub fn get_segment_by_time(
        &self,
        representation: &Representation,
        requested_time: f64,
        period_start: f64,
        period_duration: f64,
        availability_start_time: Option<&str>,
        time_shift_buffer_depth: Option<f64>,
        suggested_presentation_delay: f64,
    ) -> Option<Segment> {
        let duration = match representation.segment_duration {
            Some(d) if !d.is_nan() && d > 0.0 => d,
            _ => return None,
        };

        let period_relative_time = self
            .timeline_converter
            .calc_period_relative_time_from_mpd_relative_time(requested_time, period_start);

        let index = (period_relative_time / duration).floor() as u32;

        self.get_segment_by_index(
            representation,
            index,
            period_start,
            period_duration,
            availability_start_time,
            time_shift_buffer_depth,
            suggested_presentation_delay,
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn make_rep() -> Representation {
        Representation {
            id: Some("1".to_string()),
            bandwidth: Some(1000000),
            timescale: 90000,
            segment_duration: Some(2.0),
            start_number: 1,
            media: Some("seg-$RepresentationID$-$Number$.m4s".to_string()),
            initialization: Some("init-$RepresentationID$.mp4".to_string()),
            ..Representation::default()
        }
    }

    #[test]
    fn test_get_segment_by_index() {
        let getter = TemplateSegmentsGetter::new(TimelineConverter::new(), false);
        let rep = make_rep();

        let seg = getter
            .get_segment_by_index(&rep, 0, 0.0, 30.0, None, None, 0.0)
            .unwrap();
        assert_eq!(seg.index, Some(0));
        assert!((seg.duration.unwrap() - 2.0).abs() < 0.01);
        assert!((seg.presentation_start_time.unwrap() - 0.0).abs() < 0.01);
    }

    #[test]
    fn test_get_segment_by_index_sequence() {
        let getter = TemplateSegmentsGetter::new(TimelineConverter::new(), false);
        let rep = make_rep();

        for i in 0..5 {
            let seg = getter
                .get_segment_by_index(&rep, i, 0.0, 30.0, None, None, 0.0)
                .unwrap();
            let expected_start = i as f64 * 2.0;
            assert!((seg.presentation_start_time.unwrap() - expected_start).abs() < 0.01);
            assert_eq!(seg.replacement_number, Some(1 + i as u64));
        }
    }

    #[test]
    fn test_get_segment_by_time() {
        let getter = TemplateSegmentsGetter::new(TimelineConverter::new(), false);
        let rep = make_rep();

        let seg = getter
            .get_segment_by_time(&rep, 5.0, 0.0, 30.0, None, None, 0.0)
            .unwrap();
        // 5.0 / 2.0 = index 2
        assert_eq!(seg.index, Some(2));
        assert!((seg.presentation_start_time.unwrap() - 4.0).abs() < 0.01);
    }

    #[test]
    fn test_no_segment_beyond_period() {
        let getter = TemplateSegmentsGetter::new(TimelineConverter::new(), false);
        let rep = make_rep();

        let seg = getter.get_segment_by_index(&rep, 20, 0.0, 30.0, None, None, 0.0);
        // 20 * 2.0 = 40.0 > 30.0
        assert!(seg.is_none());
    }

    #[test]
    fn test_end_number_constraint() {
        let getter = TemplateSegmentsGetter::new(TimelineConverter::new(), false);
        let mut rep = make_rep();
        rep.end_number = Some(3);

        let seg = getter.get_segment_by_index(&rep, 2, 0.0, 30.0, None, None, 0.0);
        // replacement_number = 1 + 2 = 3, which equals end_number → allowed
        assert!(seg.is_some());

        let seg = getter.get_segment_by_index(&rep, 3, 0.0, 30.0, None, None, 0.0);
        // replacement_number = 1 + 3 = 4 > end_number 3 → blocked
        assert!(seg.is_none());
    }
}
