//! Port of `dash.js/src/dash/DashHandler.js`.
//!
//! Handles segment requests and URL construction.

use crate::dash::parser::process_uri_template;
use crate::dash::vo::media_info::MediaInfo;
use crate::dash::vo::representation::Representation;
use crate::dash::vo::segment::Segment;

use super::utils::timeline_converter::TimelineConverter;

/// The type of segment request.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum SegmentRequestType {
    Init,
    Media,
}

/// Represents a request for a segment.
#[derive(Clone, Debug)]
pub struct SegmentRequest {
    pub request_type: SegmentRequestType,
    pub url: Option<String>,
    pub media_type: Option<String>,
    pub range: Option<String>,
    pub index: Option<u32>,
    pub duration: Option<f64>,
    pub start_time: Option<f64>,
    pub media_start_time: Option<f64>,
    pub presentation_start_time: Option<f64>,
    pub availability_start_time: Option<f64>,
    pub availability_end_time: Option<f64>,
    pub wall_start_time: Option<f64>,
    pub bandwidth: Option<u64>,
    pub timescale: u64,
    pub adaptation_index: Option<i32>,
    pub representation_id: Option<String>,
    pub service_location: Option<String>,
    pub query_params: std::collections::HashMap<String, String>,
}

impl Default for SegmentRequest {
    fn default() -> Self {
        Self {
            request_type: SegmentRequestType::Media,
            url: None,
            media_type: None,
            range: None,
            index: None,
            duration: None,
            start_time: None,
            media_start_time: None,
            presentation_start_time: None,
            availability_start_time: None,
            availability_end_time: None,
            wall_start_time: None,
            bandwidth: None,
            timescale: 1,
            adaptation_index: None,
            representation_id: None,
            service_location: None,
            query_params: std::collections::HashMap::new(),
        }
    }
}

/// DashHandler manages segment request generation.
pub struct DashHandler {
    pub media_type: String,
    pub is_dynamic: bool,
    pub timeline_converter: TimelineConverter,
    last_segment: Option<Segment>,
    media_has_finished: bool,
}

impl DashHandler {
    pub fn new(media_type: &str, timeline_converter: TimelineConverter) -> Self {
        Self {
            media_type: media_type.to_string(),
            is_dynamic: false,
            timeline_converter,
            last_segment: None,
            media_has_finished: false,
        }
    }

    /// Initialize with manifest type info.
    pub fn initialize(&mut self, is_dynamic: bool) {
        self.is_dynamic = is_dynamic;
        self.media_has_finished = false;
    }

    /// Generate an initialization segment request.
    pub fn get_init_request(
        &self,
        _media_info: &MediaInfo,
        representation: &Representation,
        period_start: f64,
        period_duration: f64,
        availability_start_time: Option<&str>,
    ) -> Option<SegmentRequest> {
        let init_url = representation.initialization.as_ref()?;

        let url = process_uri_template(
            init_url,
            representation.id.as_deref(),
            None,
            None,
            representation.bandwidth,
            None,
        );

        let availability_start = self
            .timeline_converter
            .calc_availability_start_time_from_presentation_time(
                period_start,
                availability_start_time,
                representation.availability_time_offset,
                self.is_dynamic,
            );

        let availability_end = self
            .timeline_converter
            .calc_availability_end_time_from_presentation_time(
                period_start + period_duration,
                availability_start_time,
                None,
                self.is_dynamic,
            );

        Some(SegmentRequest {
            request_type: SegmentRequestType::Init,
            url: Some(url),
            media_type: Some(self.media_type.clone()),
            range: representation.range.clone(),
            availability_start_time: availability_start,
            availability_end_time: Some(availability_end),
            representation_id: representation.id.clone(),
            bandwidth: representation.bandwidth,
            timescale: representation.timescale,
            adaptation_index: representation.adaptation_index.map(|i| i as i32),
            ..SegmentRequest::default()
        })
    }

    /// Generate a request for a media segment at a given time.
    pub fn get_segment_request_for_time(
        &mut self,
        _media_info: &MediaInfo,
        segment: Segment,
    ) -> Option<SegmentRequest> {
        let request = self.build_request_for_segment(&segment)?;
        self.last_segment = Some(segment);
        Some(request)
    }

    /// Generate a request for the next segment after the current one.
    pub fn get_next_segment_request(
        &mut self,
        _media_info: &MediaInfo,
        segment: Option<Segment>,
    ) -> Option<SegmentRequest> {
        let seg = segment?;
        let request = self.build_request_for_segment(&seg)?;
        self.last_segment = Some(seg);
        Some(request)
    }

    /// Check if the last requested segment was the final one.
    pub fn is_last_segment_requested(
        &self,
        representation: &Representation,
        buffering_time: f64,
    ) -> bool {
        let last = match &self.last_segment {
            Some(s) => s,
            None => return false,
        };

        if self.media_has_finished {
            return true;
        }

        let info = &representation.media_finished_information;
        if info.number_of_segments > 0 {
            if let Some(idx) = last.index {
                if idx as u64 >= info.number_of_segments - 1 {
                    if !self.is_dynamic {
                        return true;
                    }
                }
            }
        }

        // Check if we're past the buffering window
        if let (Some(pst), Some(dur)) = (last.presentation_start_time, last.duration) {
            if pst + dur <= buffering_time {
                return false;
            }
        }

        false
    }

    /// Get the current segment index.
    pub fn get_current_index(&self) -> i32 {
        self.last_segment
            .as_ref()
            .and_then(|s| s.index)
            .map_or(-1, |i| i as i32)
    }

    /// Get the last segment.
    pub fn get_last_segment(&self) -> Option<&Segment> {
        self.last_segment.as_ref()
    }

    /// Mark the stream as finished (e.g. dynamic→static transition).
    pub fn set_media_finished(&mut self) {
        self.media_has_finished = true;
    }

    /// Reset internal state.
    pub fn reset(&mut self) {
        self.last_segment = None;
        self.media_has_finished = false;
    }

    fn build_request_for_segment(&self, segment: &Segment) -> Option<SegmentRequest> {
        let url = segment.media.clone()?;

        Some(SegmentRequest {
            request_type: SegmentRequestType::Media,
            url: Some(url),
            media_type: Some(self.media_type.clone()),
            range: segment.media_range.clone(),
            index: segment.index,
            duration: segment.duration,
            start_time: segment.presentation_start_time,
            media_start_time: segment.media_start_time,
            presentation_start_time: segment.presentation_start_time,
            availability_start_time: segment.availability_start_time,
            availability_end_time: segment.availability_end_time,
            wall_start_time: segment.wall_start_time,
            ..SegmentRequest::default()
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::dash::utils::timeline_converter::TimelineConverter;

    fn make_representation() -> Representation {
        Representation {
            id: Some("1".to_string()),
            bandwidth: Some(1000000),
            timescale: 90000,
            segment_duration: Some(2.0),
            start_number: 1,
            initialization: Some("init-$RepresentationID$.mp4".to_string()),
            media: Some("seg-$RepresentationID$-$Number$.m4s".to_string()),
            ..Representation::default()
        }
    }

    fn make_segment(index: u32) -> Segment {
        Segment {
            index: Some(index),
            media: Some(format!("seg-1-{}.m4s", index + 1)),
            duration: Some(2.0),
            presentation_start_time: Some(index as f64 * 2.0),
            media_start_time: Some(index as f64 * 2.0),
            replacement_number: Some(index as u64 + 1),
            ..Segment::default()
        }
    }

    #[test]
    fn test_get_init_request() {
        let handler = DashHandler::new("video", TimelineConverter::new());
        let rep = make_representation();
        let mi = MediaInfo::default();

        let req = handler
            .get_init_request(&mi, &rep, 0.0, 30.0, None)
            .unwrap();
        assert_eq!(req.request_type, SegmentRequestType::Init);
        assert_eq!(req.url.as_deref(), Some("init-1.mp4"));
        assert_eq!(req.media_type.as_deref(), Some("video"));
    }

    #[test]
    fn test_get_segment_request() {
        let mut handler = DashHandler::new("video", TimelineConverter::new());
        let mi = MediaInfo::default();
        let seg = make_segment(0);

        let req = handler
            .get_segment_request_for_time(&mi, seg)
            .unwrap();
        assert_eq!(req.request_type, SegmentRequestType::Media);
        assert_eq!(req.url.as_deref(), Some("seg-1-1.m4s"));
        assert_eq!(req.index, Some(0));
        assert!((req.duration.unwrap() - 2.0).abs() < 0.01);
    }

    #[test]
    fn test_get_next_segment_request() {
        let mut handler = DashHandler::new("video", TimelineConverter::new());
        let mi = MediaInfo::default();
        let seg0 = make_segment(0);
        let seg1 = make_segment(1);

        handler.get_segment_request_for_time(&mi, seg0);
        let req = handler
            .get_next_segment_request(&mi, Some(seg1))
            .unwrap();
        assert_eq!(req.index, Some(1));
    }

    #[test]
    fn test_get_current_index() {
        let mut handler = DashHandler::new("video", TimelineConverter::new());
        assert_eq!(handler.get_current_index(), -1);

        let mi = MediaInfo::default();
        let seg = make_segment(5);
        handler.get_segment_request_for_time(&mi, seg);
        assert_eq!(handler.get_current_index(), 5);
    }

    #[test]
    fn test_reset() {
        let mut handler = DashHandler::new("video", TimelineConverter::new());
        let mi = MediaInfo::default();
        handler.get_segment_request_for_time(&mi, make_segment(0));
        handler.set_media_finished();
        handler.reset();
        assert_eq!(handler.get_current_index(), -1);
        assert!(!handler.media_has_finished);
    }

    #[test]
    fn test_init_request_without_initialization() {
        let handler = DashHandler::new("video", TimelineConverter::new());
        let rep = Representation::default();
        let mi = MediaInfo::default();

        let req = handler.get_init_request(&mi, &rep, 0.0, 30.0, None);
        assert!(req.is_none());
    }
}
