//! Port of `dash.js/src/dash/utils/ListSegmentsGetter.js`.
//!
//! Generates segment list from SegmentList with explicit SegmentURL elements.

use crate::dash::vo::representation::{MediaFinishedInformation, Representation};
use crate::dash::vo::segment::Segment;

use super::segments_utils::{get_index_based_segment, IndexBasedSegmentData};
use super::timeline_converter::TimelineConverter;

/// A SegmentURL entry from a SegmentList.
#[derive(Clone, Debug, Default)]
pub struct SegmentUrlEntry {
    pub media: Option<String>,
    pub media_range: Option<String>,
    pub index_range: Option<String>,
}

/// Generates segments based on SegmentList addressing.
pub struct ListSegmentsGetter {
    pub timeline_converter: TimelineConverter,
    pub is_dynamic: bool,
}

impl ListSegmentsGetter {
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
        segment_urls: &[SegmentUrlEntry],
    ) -> MediaFinishedInformation {
        let start_number = if representation.start_number > 0 {
            representation.start_number
        } else {
            1
        };
        let offset = (start_number - 1).max(0);

        MediaFinishedInformation {
            number_of_segments: offset + segment_urls.len() as u64,
            media_time_of_last_signaled_segment: None,
        }
    }

    /// Get a segment by its index.
    pub fn get_segment_by_index(
        &self,
        representation: &Representation,
        index: u32,
        segment_urls: &[SegmentUrlEntry],
        period_start: f64,
        period_duration: f64,
        availability_start_time: Option<&str>,
        time_shift_buffer_depth: Option<f64>,
        suggested_presentation_delay: f64,
    ) -> Option<Segment> {
        let len = segment_urls.len();
        let start_number = if representation.start_number > 0 {
            representation.start_number
        } else {
            1
        };
        let offset = (start_number as i64 - 1).max(0) as u32;
        let relative_index = if index >= offset { index - offset } else { 0 };

        if (relative_index as usize) >= len {
            return None;
        }

        let s = &segment_urls[relative_index as usize];
        let segment_duration = representation.segment_duration.unwrap_or(0.0);

        let data = IndexBasedSegmentData {
            index,
            is_dynamic: self.is_dynamic,
            media_range: s.media_range.clone(),
            media_time: Some(((start_number + index as u64 - 1) as f64 * segment_duration) as u64),
            media_url: s.media.clone(),
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

        let mut seg = get_index_based_segment(&data)?;

        // Override media URL from SegmentURL if present
        if let Some(ref m) = s.media {
            seg.media = Some(m.clone());
        }
        if s.index_range.is_some() {
            seg.index_range = s.index_range.clone();
        }

        Some(seg)
    }

    /// Get a segment by requested presentation time.
    pub fn get_segment_by_time(
        &self,
        representation: &Representation,
        requested_time: f64,
        segment_urls: &[SegmentUrlEntry],
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

        let period_time = self
            .timeline_converter
            .calc_period_relative_time_from_mpd_relative_time(requested_time, period_start);
        let index = (period_time / duration).floor() as u32;

        self.get_segment_by_index(
            representation,
            index,
            segment_urls,
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
            bandwidth: Some(500000),
            timescale: 1000,
            segment_duration: Some(2.0),
            start_number: 1,
            ..Representation::default()
        }
    }

    fn make_segment_urls() -> Vec<SegmentUrlEntry> {
        vec![
            SegmentUrlEntry {
                media: Some("seg-0.m4s".to_string()),
                media_range: None,
                index_range: None,
            },
            SegmentUrlEntry {
                media: Some("seg-1.m4s".to_string()),
                media_range: None,
                index_range: None,
            },
            SegmentUrlEntry {
                media: Some("seg-2.m4s".to_string()),
                media_range: None,
                index_range: None,
            },
        ]
    }

    #[test]
    fn test_get_segment_by_index() {
        let getter = ListSegmentsGetter::new(TimelineConverter::new(), false);
        let rep = make_rep();
        let urls = make_segment_urls();

        let seg = getter
            .get_segment_by_index(&rep, 0, &urls, 0.0, 30.0, None, None, 0.0)
            .unwrap();
        assert_eq!(seg.index, Some(0));
        assert_eq!(seg.media.as_deref(), Some("seg-0.m4s"));
    }

    #[test]
    fn test_get_segment_by_index_out_of_bounds() {
        let getter = ListSegmentsGetter::new(TimelineConverter::new(), false);
        let rep = make_rep();
        let urls = make_segment_urls();

        let seg = getter.get_segment_by_index(&rep, 10, &urls, 0.0, 30.0, None, None, 0.0);
        assert!(seg.is_none());
    }

    #[test]
    fn test_get_segment_by_time() {
        let getter = ListSegmentsGetter::new(TimelineConverter::new(), false);
        let rep = make_rep();
        let urls = make_segment_urls();

        let seg = getter
            .get_segment_by_time(&rep, 3.0, &urls, 0.0, 30.0, None, None, 0.0)
            .unwrap();
        assert_eq!(seg.index, Some(1));
    }

    #[test]
    fn test_media_finished_information() {
        let getter = ListSegmentsGetter::new(TimelineConverter::new(), false);
        let rep = make_rep();
        let urls = make_segment_urls();

        let info = getter.get_media_finished_information(&rep, &urls);
        // start_number=1, offset=0, 3 URLs → 3 segments
        assert_eq!(info.number_of_segments, 3);
    }
}
