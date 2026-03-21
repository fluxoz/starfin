//! Port of `dash.js/src/dash/vo/Segment.js`, `FullSegment.js` and `PartialSegment.js`.

use serde::{Deserialize, Serialize};

/// A media segment – the base type shared by full and partial segments.
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(default)]
pub struct Segment {
    /// The index of the segment in the segment list (starts at 0).
    pub index: Option<u32>,
    pub media: Option<String>,
    pub media_range: Option<String>,
    pub index_range: Option<String>,
    pub media_url: Option<String>,

    /// Do not schedule this segment until this wall-clock time.
    pub availability_start_time: Option<f64>,
    /// Ignore and discard this segment after this wall-clock time.
    pub availability_end_time: Option<f64>,

    pub duration: Option<f64>,
    /// For dynamic MPDs, the wall-clock time the video element should display.
    pub wall_start_time: Option<f64>,

    /// Time encoded in the media segment.
    pub media_start_time: Option<f64>,
    /// Time matching seekTarget / video.currentTime when mseTimeOffset is applied.
    pub presentation_start_time: Option<f64>,

    /// Representation index (avoids circular reference).
    pub representation_index: Option<usize>,
    /// Number inserted into the media URL template.
    pub replacement_number: Option<u64>,
    /// Time value inserted into the media URL template.
    pub replacement_time: Option<u64>,

    pub is_partial_segment: bool,
}

impl Default for Segment {
    fn default() -> Self {
        Self {
            index: None,
            media: None,
            media_range: None,
            index_range: None,
            media_url: None,
            availability_start_time: None,
            availability_end_time: None,
            duration: None,
            wall_start_time: None,
            media_start_time: None,
            presentation_start_time: None,
            representation_index: None,
            replacement_number: None,
            replacement_time: None,
            is_partial_segment: false,
        }
    }
}

/// A full (non-partial) segment. Identical to [`Segment`] with
/// `is_partial_segment = false`.
pub type FullSegment = Segment;

/// A partial (low-latency) segment with additional fields.
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(default)]
pub struct PartialSegment {
    #[serde(flatten)]
    pub segment: Segment,
    /// Sub-number within the parent segment (first partial = 0).
    pub replacement_sub_number: Option<u64>,
    pub total_number_of_partial_segments: Option<u64>,
}

impl Default for PartialSegment {
    fn default() -> Self {
        Self {
            segment: Segment {
                is_partial_segment: true,
                ..Segment::default()
            },
            replacement_sub_number: None,
            total_number_of_partial_segments: None,
        }
    }
}
