//! Port of `dash.js/src/dash/utils/SegmentsUtils.js`.
//!
//! Utility functions for segment creation and URI template processing.

use crate::dash::parser::process_uri_template;
use crate::dash::vo::segment::Segment;

use super::timeline_converter::TimelineConverter;

/// Data for building an index-based segment.
pub struct IndexBasedSegmentData<'a> {
    pub index: u32,
    pub is_dynamic: bool,
    pub media_range: Option<String>,
    pub media_time: Option<u64>,
    pub media_url: Option<String>,
    pub representation_id: Option<String>,
    pub bandwidth: Option<u64>,
    pub start_number: u64,
    pub timescale: u64,
    pub segment_duration: f64,
    pub period_start: f64,
    pub period_duration: f64,
    pub presentation_time_offset: f64,
    pub availability_start_time: Option<String>,
    pub availability_time_offset: f64,
    pub time_shift_buffer_depth: Option<f64>,
    pub suggested_presentation_delay: f64,
    pub timeline_converter: &'a TimelineConverter,
}

/// Data for building a time-based segment.
pub struct TimeBasedSegmentData<'a> {
    pub duration_in_timescale: u64,
    pub timescale: u64,
    pub index: u32,
    pub is_dynamic: bool,
    pub media_range: Option<String>,
    pub media_time: u64,
    pub media_url: Option<String>,
    pub representation_id: Option<String>,
    pub bandwidth: Option<u64>,
    pub start_number: u64,
    pub period_start: f64,
    pub presentation_time_offset: f64,
    pub availability_start_time: Option<String>,
    pub availability_time_offset: f64,
    pub time_shift_buffer_depth: Option<f64>,
    pub suggested_presentation_delay: f64,
    pub t_manifest: Option<u64>,
    pub timeline_converter: &'a TimelineConverter,
}

/// Build an index-based segment (used by TemplateSegmentsGetter and ListSegmentsGetter).
pub fn get_index_based_segment(data: &IndexBasedSegmentData) -> Option<Segment> {
    let segment_duration = if data.segment_duration.is_nan() || data.segment_duration <= 0.0 {
        data.period_duration
    } else {
        data.segment_duration
    };

    let presentation_start_time =
        round_to_5((data.period_start + (data.index as f64) * segment_duration) as f64);
    let presentation_end_time = round_to_5(presentation_start_time + segment_duration);

    let media_time_in_seconds = data.timeline_converter.calc_media_time_from_presentation_time(
        presentation_start_time,
        data.period_start,
        data.presentation_time_offset,
    );

    let replacement_number = data.start_number + data.index as u64;

    let availability_start_time =
        data.timeline_converter
            .calc_availability_start_time_from_presentation_time(
                presentation_end_time,
                data.availability_start_time.as_deref(),
                data.availability_time_offset,
                data.is_dynamic,
            );

    let availability_end_time =
        data.timeline_converter
            .calc_availability_end_time_from_presentation_time(
                presentation_end_time + segment_duration,
                data.availability_start_time.as_deref(),
                data.time_shift_buffer_depth,
                data.is_dynamic,
            );

    let wall_start_time = data.timeline_converter.calc_wall_time_for_segment(
        availability_start_time.unwrap_or(0.0),
        presentation_start_time,
        data.suggested_presentation_delay,
        data.is_dynamic,
    );

    let replacement_time = data.media_time.or(Some(
        (media_time_in_seconds * data.timescale as f64) as u64,
    ));

    let media = data.media_url.as_ref().map(|url| {
        process_uri_template(
            url,
            data.representation_id.as_deref(),
            Some(replacement_number),
            None,
            data.bandwidth,
            replacement_time,
        )
    });

    // Check segment availability for period boundary
    if data.period_duration.is_finite()
        && data.period_start + data.period_duration <= presentation_start_time
    {
        return None;
    }

    Some(Segment {
        index: Some(data.index),
        media,
        media_range: data.media_range.clone(),
        media_url: data.media_url.clone(),
        availability_start_time,
        availability_end_time: Some(availability_end_time),
        duration: Some(segment_duration),
        wall_start_time,
        media_start_time: Some(media_time_in_seconds),
        presentation_start_time: Some(presentation_start_time),
        representation_index: None,
        replacement_number: Some(replacement_number),
        replacement_time,
        is_partial_segment: false,
        ..Segment::default()
    })
}

/// Build a time-based segment (used by TimelineSegmentsGetter).
pub fn get_time_based_segment(data: &TimeBasedSegmentData) -> Option<Segment> {
    let timescale = data.timescale as f64;
    let media_time_in_seconds = data.media_time as f64 / timescale;
    let segment_duration_in_seconds = data.duration_in_timescale as f64 / timescale;

    let presentation_start_time = data
        .timeline_converter
        .calc_presentation_time_from_media_time(
            media_time_in_seconds,
            data.period_start,
            data.presentation_time_offset,
        );
    let presentation_end_time = presentation_start_time + segment_duration_in_seconds;

    let replacement_number = data.start_number + data.index as u64;

    let availability_start_time =
        data.timeline_converter
            .calc_availability_start_time_from_presentation_time(
                presentation_end_time,
                data.availability_start_time.as_deref(),
                data.availability_time_offset,
                data.is_dynamic,
            );

    let availability_end_time =
        data.timeline_converter
            .calc_availability_end_time_from_presentation_time(
                presentation_end_time + segment_duration_in_seconds,
                data.availability_start_time.as_deref(),
                data.time_shift_buffer_depth,
                data.is_dynamic,
            );

    let wall_start_time = data.timeline_converter.calc_wall_time_for_segment(
        availability_start_time.unwrap_or(0.0),
        presentation_start_time,
        data.suggested_presentation_delay,
        data.is_dynamic,
    );

    let replacement_time = data.t_manifest.or(Some(data.media_time));

    let media = data.media_url.as_ref().map(|url| {
        process_uri_template(
            url,
            data.representation_id.as_deref(),
            Some(replacement_number),
            None,
            data.bandwidth,
            replacement_time,
        )
    });

    Some(Segment {
        index: Some(data.index),
        media,
        media_range: data.media_range.clone(),
        media_url: data.media_url.clone(),
        availability_start_time,
        availability_end_time: Some(availability_end_time),
        duration: Some(segment_duration_in_seconds),
        wall_start_time,
        media_start_time: Some(media_time_in_seconds),
        presentation_start_time: Some(presentation_start_time),
        representation_index: None,
        replacement_number: Some(replacement_number),
        replacement_time,
        is_partial_segment: false,
        ..Segment::default()
    })
}

fn round_to_5(v: f64) -> f64 {
    (v * 100000.0).round() / 100000.0
}

#[cfg(test)]
mod tests {
    use super::*;

    fn make_converter() -> TimelineConverter {
        TimelineConverter::new()
    }

    #[test]
    fn test_index_based_segment_static() {
        let tc = make_converter();
        let data = IndexBasedSegmentData {
            index: 0,
            is_dynamic: false,
            media_range: None,
            media_time: None,
            media_url: Some("seg-$Number$.m4s".to_string()),
            representation_id: Some("1".to_string()),
            bandwidth: Some(1000000),
            start_number: 1,
            timescale: 1000,
            segment_duration: 2.0,
            period_start: 0.0,
            period_duration: 30.0,
            presentation_time_offset: 0.0,
            availability_start_time: None,
            availability_time_offset: 0.0,
            time_shift_buffer_depth: None,
            suggested_presentation_delay: 0.0,
            timeline_converter: &tc,
        };

        let seg = get_index_based_segment(&data).unwrap();
        assert_eq!(seg.index, Some(0));
        assert!((seg.presentation_start_time.unwrap() - 0.0).abs() < 0.01);
        assert!((seg.duration.unwrap() - 2.0).abs() < 0.01);
        assert_eq!(seg.replacement_number, Some(1));
        assert_eq!(seg.media.as_deref(), Some("seg-1.m4s"));
    }

    #[test]
    fn test_index_based_segment_beyond_period() {
        let tc = make_converter();
        let data = IndexBasedSegmentData {
            index: 20,
            is_dynamic: false,
            media_range: None,
            media_time: None,
            media_url: Some("seg-$Number$.m4s".to_string()),
            representation_id: Some("1".to_string()),
            bandwidth: Some(1000000),
            start_number: 1,
            timescale: 1000,
            segment_duration: 2.0,
            period_start: 0.0,
            period_duration: 30.0,
            presentation_time_offset: 0.0,
            availability_start_time: None,
            availability_time_offset: 0.0,
            time_shift_buffer_depth: None,
            suggested_presentation_delay: 0.0,
            timeline_converter: &tc,
        };

        let seg = get_index_based_segment(&data);
        // index 20 * 2.0s = 40s > period_duration 30s
        assert!(seg.is_none());
    }

    #[test]
    fn test_time_based_segment() {
        let tc = make_converter();
        let data = TimeBasedSegmentData {
            duration_in_timescale: 180000,
            timescale: 90000,
            index: 0,
            is_dynamic: false,
            media_range: None,
            media_time: 0,
            media_url: Some("seg-$Time$.m4s".to_string()),
            representation_id: Some("1".to_string()),
            bandwidth: Some(500000),
            start_number: 1,
            period_start: 0.0,
            presentation_time_offset: 0.0,
            availability_start_time: None,
            availability_time_offset: 0.0,
            time_shift_buffer_depth: None,
            suggested_presentation_delay: 0.0,
            t_manifest: Some(0),
            timeline_converter: &tc,
        };

        let seg = get_time_based_segment(&data).unwrap();
        assert_eq!(seg.index, Some(0));
        assert!((seg.duration.unwrap() - 2.0).abs() < 0.01);
        assert!((seg.presentation_start_time.unwrap() - 0.0).abs() < 0.01);
        assert_eq!(seg.media.as_deref(), Some("seg-0.m4s"));
    }

    #[test]
    fn test_multiple_segments() {
        let tc = make_converter();
        for i in 0..5 {
            let data = IndexBasedSegmentData {
                index: i,
                is_dynamic: false,
                media_range: None,
                media_time: None,
                media_url: Some("seg-$Number$.m4s".to_string()),
                representation_id: Some("v1".to_string()),
                bandwidth: Some(500000),
                start_number: 0,
                timescale: 1000,
                segment_duration: 4.0,
                period_start: 0.0,
                period_duration: 60.0,
                presentation_time_offset: 0.0,
                availability_start_time: None,
                availability_time_offset: 0.0,
                time_shift_buffer_depth: None,
                suggested_presentation_delay: 0.0,
                timeline_converter: &tc,
            };
            let seg = get_index_based_segment(&data).unwrap();
            let expected_start = i as f64 * 4.0;
            assert!((seg.presentation_start_time.unwrap() - expected_start).abs() < 0.01);
            assert_eq!(seg.replacement_number, Some(i as u64));
        }
    }
}
