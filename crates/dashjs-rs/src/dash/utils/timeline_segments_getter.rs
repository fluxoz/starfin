//! Port of `dash.js/src/dash/utils/TimelineSegmentsGetter.js`.
//!
//! Generates segment lists from SegmentTimeline (S elements with @t, @d, @r).

use crate::dash::parser::SElement;
use crate::dash::vo::representation::{MediaFinishedInformation, Representation};
use crate::dash::vo::segment::Segment;

use super::segments_utils::{get_time_based_segment, TimeBasedSegmentData};
use super::timeline_converter::TimelineConverter;

/// Generates segments based on SegmentTimeline addressing.
pub struct TimelineSegmentsGetter {
    pub timeline_converter: TimelineConverter,
    pub is_dynamic: bool,
}

impl TimelineSegmentsGetter {
    pub fn new(timeline_converter: TimelineConverter, is_dynamic: bool) -> Self {
        Self {
            timeline_converter,
            is_dynamic,
        }
    }

    /// Get a segment by time, iterating over the timeline S elements.
    pub fn get_segment_by_time(
        &self,
        representation: &Representation,
        requested_presentation_time: f64,
        timeline: &[SElement],
        media_url: Option<&str>,
        period_start: f64,
        period_duration: f64,
        availability_start_time: Option<&str>,
        time_shift_buffer_depth: Option<f64>,
        suggested_presentation_delay: f64,
    ) -> Option<Segment> {
        let timescale = representation.timescale as f64;
        let required_media_time = self
            .timeline_converter
            .calc_media_time_from_presentation_time(
                requested_presentation_time,
                period_start,
                representation.presentation_time_offset,
            );

        let required_media_time_in_timescale = precision_round(required_media_time * timescale);

        let mut media_time: f64 = 0.0;
        let mut s_counter_including_repeats: i64 = -1;

        for (_s_idx, s_element) in timeline.iter().enumerate() {
            let mut repeat = s_element.r.unwrap_or(0);

            if let Some(t) = s_element.t {
                media_time = t as f64;
            }

            // Handle negative r: repeat until next S element or end of period
            if repeat < 0 {
                repeat = self.calculate_repeat_count_for_negative_r(
                    representation,
                    timeline,
                    _s_idx,
                    s_element,
                    timescale,
                    media_time / timescale,
                    period_start,
                    period_duration,
                );
            }

            for _j in 0..=repeat {
                s_counter_including_repeats += 1;

                let d = s_element.d as f64;
                let media_start = media_time;
                let media_end = media_time + d;

                if required_media_time_in_timescale < media_end
                    && required_media_time_in_timescale >= media_start
                {
                    let data = TimeBasedSegmentData {
                        duration_in_timescale: s_element.d,
                        timescale: representation.timescale,
                        index: s_counter_including_repeats as u32,
                        is_dynamic: self.is_dynamic,
                        media_range: None,
                        media_time: media_start as u64,
                        media_url: media_url.map(String::from),
                        representation_id: representation.id.clone(),
                        bandwidth: representation.bandwidth,
                        start_number: representation.start_number,
                        period_start,
                        presentation_time_offset: representation.presentation_time_offset,
                        availability_start_time: availability_start_time.map(String::from),
                        availability_time_offset: representation.availability_time_offset,
                        time_shift_buffer_depth,
                        suggested_presentation_delay,
                        t_manifest: s_element.t,
                        timeline_converter: &self.timeline_converter,
                    };
                    return get_time_based_segment(&data);
                }

                media_time += d;
            }
        }

        None
    }

    /// Get a segment by index, using last segment to determine the next time.
    pub fn get_segment_by_index(
        &self,
        representation: &Representation,
        last_segment: Option<&Segment>,
        timeline: &[SElement],
        media_url: Option<&str>,
        period_start: f64,
        period_duration: f64,
        availability_start_time: Option<&str>,
        time_shift_buffer_depth: Option<f64>,
        suggested_presentation_delay: f64,
    ) -> Option<Segment> {
        let safety_offset = 0.01;
        let requested_time = match last_segment {
            Some(seg) => {
                let pst = seg.presentation_start_time.unwrap_or(0.0);
                let dur = seg.duration.unwrap_or(0.0);
                pst + dur + safety_offset
            }
            None => 0.0,
        };

        self.get_segment_by_time(
            representation,
            requested_time,
            timeline,
            media_url,
            period_start,
            period_duration,
            availability_start_time,
            time_shift_buffer_depth,
            suggested_presentation_delay,
        )
    }

    /// Calculate finished information from the timeline.
    pub fn get_media_finished_information(
        &self,
        representation: &Representation,
        timeline: &[SElement],
    ) -> MediaFinishedInformation {
        let timescale = representation.timescale as f64;
        let mut media_time: f64 = 0.0;
        let mut media_time_in_seconds: f64 = 0.0;
        let mut available_segments: u64 = 0;

        for (i, frag) in timeline.iter().enumerate() {
            let mut repeat = frag.r.unwrap_or(0);

            if let Some(t) = frag.t {
                media_time = t as f64;
                media_time_in_seconds = media_time / timescale;
            }

            if repeat < 0 {
                repeat = self.calculate_repeat_count_for_negative_r(
                    representation,
                    timeline,
                    i,
                    frag,
                    timescale,
                    media_time_in_seconds,
                    0.0,
                    f64::INFINITY,
                );
            }

            for _j in 0..=repeat {
                available_segments += 1;
                media_time += frag.d as f64;
                media_time_in_seconds = media_time / timescale;
            }
        }

        MediaFinishedInformation {
            number_of_segments: available_segments,
            media_time_of_last_signaled_segment: Some(media_time_in_seconds),
        }
    }

    fn calculate_repeat_count_for_negative_r(
        &self,
        representation: &Representation,
        timeline: &[SElement],
        current_index: usize,
        frag: &SElement,
        timescale: f64,
        scaled_time: f64,
        period_start: f64,
        period_duration: f64,
    ) -> i64 {
        let repeat_end_time = if current_index + 1 < timeline.len() {
            if let Some(t) = timeline[current_index + 1].t {
                t as f64 / timescale
            } else {
                self.get_availability_end(representation, period_start, period_duration)
            }
        } else {
            self.get_availability_end(representation, period_start, period_duration)
        };

        let frag_duration = frag.d as f64 / timescale;
        if frag_duration > 0.0 {
            ((repeat_end_time - scaled_time) / frag_duration).ceil() as i64 - 1
        } else {
            0
        }
        .max(0)
    }

    fn get_availability_end(
        &self,
        _representation: &Representation,
        period_start: f64,
        period_duration: f64,
    ) -> f64 {
        if period_duration.is_finite() && !period_duration.is_nan() {
            period_start + period_duration
        } else {
            0.0
        }
    }
}

fn precision_round(number: f64) -> f64 {
    // Use 15 significant digits of precision (matches JS toPrecision(15))
    let s = format!("{:.15e}", number);
    s.parse::<f64>().unwrap_or(number)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn make_rep() -> Representation {
        Representation {
            id: Some("1".to_string()),
            bandwidth: Some(1000000),
            timescale: 90000,
            start_number: 1,
            media: Some("seg-$Time$.m4s".to_string()),
            initialization: Some("init.mp4".to_string()),
            ..Representation::default()
        }
    }

    fn make_simple_timeline() -> Vec<SElement> {
        vec![
            SElement {
                t: Some(0),
                d: 180000,
                r: Some(4),
                k: None,
            },
            SElement {
                t: None,
                d: 90000,
                r: None,
                k: None,
            },
        ]
    }

    #[test]
    fn test_get_segment_by_time_first() {
        let getter = TimelineSegmentsGetter::new(TimelineConverter::new(), false);
        let rep = make_rep();
        let timeline = make_simple_timeline();

        let seg = getter
            .get_segment_by_time(&rep, 0.0, &timeline, Some("seg-$Time$.m4s"), 0.0, 30.0, None, None, 0.0)
            .unwrap();
        assert_eq!(seg.index, Some(0));
        assert!((seg.duration.unwrap() - 2.0).abs() < 0.01);
    }

    #[test]
    fn test_get_segment_by_time_middle() {
        let getter = TimelineSegmentsGetter::new(TimelineConverter::new(), false);
        let rep = make_rep();
        let timeline = make_simple_timeline();

        // Request at 4.0s = 360000 in timescale, second S element repeat
        let seg = getter
            .get_segment_by_time(&rep, 4.0, &timeline, Some("seg-$Time$.m4s"), 0.0, 30.0, None, None, 0.0)
            .unwrap();
        assert_eq!(seg.index, Some(2));
    }

    #[test]
    fn test_get_segment_by_index_with_last_segment() {
        let getter = TimelineSegmentsGetter::new(TimelineConverter::new(), false);
        let rep = make_rep();
        let timeline = make_simple_timeline();

        // First segment
        let seg0 = getter
            .get_segment_by_time(&rep, 0.0, &timeline, Some("seg-$Time$.m4s"), 0.0, 30.0, None, None, 0.0)
            .unwrap();

        // Next segment using last segment
        let seg1 = getter
            .get_segment_by_index(&rep, Some(&seg0), &timeline, Some("seg-$Time$.m4s"), 0.0, 30.0, None, None, 0.0)
            .unwrap();
        assert_eq!(seg1.index, Some(1));
    }

    #[test]
    fn test_media_finished_information() {
        let getter = TimelineSegmentsGetter::new(TimelineConverter::new(), false);
        let rep = make_rep();
        let timeline = make_simple_timeline();

        let info = getter.get_media_finished_information(&rep, &timeline);
        // 5 repeats of first S + 1 of second = 6 segments
        assert_eq!(info.number_of_segments, 6);
    }

    #[test]
    fn test_negative_r() {
        let getter = TimelineSegmentsGetter::new(TimelineConverter::new(), false);
        let mut rep = make_rep();
        rep.timescale = 1000;
        let timeline = vec![
            SElement {
                t: Some(0),
                d: 2000,
                r: Some(-1),
                k: None,
            },
        ];

        let info = getter.get_media_finished_information(&rep, &timeline);
        // With negative r and no period info, the repeat count calculation
        // depends on availability end. With default period_duration infinity from getter,
        // it returns 0, so 1 segment (the initial one).
        assert!(info.number_of_segments >= 1);
    }
}
