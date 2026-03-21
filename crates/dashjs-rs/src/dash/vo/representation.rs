//! Port of `dash.js/src/dash/vo/Representation.js`.

use serde::{Deserialize, Serialize};

use super::base_url::BaseUrl;
use super::descriptor_type::DescriptorType;
use super::segment::Segment;
use crate::dash::constants;

/// Information about the last media segment that has been signalled.
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct MediaFinishedInformation {
    pub number_of_segments: u64,
    pub media_time_of_last_signaled_segment: Option<f64>,
}

/// A single Representation within an AdaptationSet.
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(default)]
pub struct Representation {
    pub id: Option<String>,
    pub index: Option<u32>,
    /// Absolute index across all adaptation sets.
    pub absolute_index: Option<u32>,
    /// Index of the parent AdaptationSet.
    pub adaptation_index: Option<usize>,

    pub bandwidth: Option<u64>,
    pub bitrate_in_kbit: Option<f64>,
    pub bits_per_pixel: Option<f64>,
    pub width: Option<u32>,
    pub height: Option<u32>,
    pub codecs: Option<String>,
    pub codec_family: Option<String>,
    pub codec_private_data: Option<String>,
    pub mime_type: Option<String>,
    pub scan_type: Option<String>,
    pub frame_rate: Option<String>,
    pub sar: Option<String>,
    pub audio_sampling_rate: Option<String>,
    pub max_playout_rate: Option<f64>,
    pub quality_ranking: Option<u32>,
    pub coding_dependency: Option<String>,
    pub dependency_id: Option<String>,

    // Segment addressing
    pub segment_info_type: Option<String>,
    pub initialization: Option<String>,
    pub media: Option<String>,
    pub start_number: u64,
    pub end_number: Option<u64>,
    pub timescale: u64,
    pub presentation_time_offset: f64,
    pub segment_duration: Option<f64>,
    pub availability_time_offset: f64,
    pub availability_time_complete: bool,
    pub media_finished_information: MediaFinishedInformation,
    pub mse_time_offset: Option<f64>,
    pub pixels_per_second: Option<f64>,
    pub k: u32,
    pub fragment_duration: Option<f64>,

    // Ranges
    pub range: Option<String>,
    pub index_range: Option<String>,

    // Child elements
    pub base_urls: Vec<BaseUrl>,
    pub segments: Option<Vec<Segment>>,
    pub essential_properties: Vec<DescriptorType>,
    pub supplemental_properties: Vec<DescriptorType>,
}

impl Default for Representation {
    fn default() -> Self {
        Self {
            id: None,
            index: None,
            absolute_index: None,
            adaptation_index: None,
            bandwidth: None,
            bitrate_in_kbit: None,
            bits_per_pixel: None,
            width: None,
            height: None,
            codecs: None,
            codec_family: None,
            codec_private_data: None,
            mime_type: None,
            scan_type: None,
            frame_rate: None,
            sar: None,
            audio_sampling_rate: None,
            max_playout_rate: None,
            quality_ranking: None,
            coding_dependency: None,
            dependency_id: None,
            segment_info_type: None,
            initialization: None,
            media: None,
            start_number: 1,
            end_number: None,
            timescale: 1,
            presentation_time_offset: 0.0,
            segment_duration: None,
            availability_time_offset: 0.0,
            availability_time_complete: true,
            media_finished_information: MediaFinishedInformation {
                number_of_segments: 0,
                media_time_of_last_signaled_segment: None,
            },
            mse_time_offset: None,
            pixels_per_second: None,
            k: 1,
            fragment_duration: None,
            range: None,
            index_range: None,
            base_urls: Vec::new(),
            segments: None,
            essential_properties: Vec::new(),
            supplemental_properties: Vec::new(),
        }
    }
}

impl Representation {
    /// Returns `true` when the representation carries an initialisation segment
    /// (either an explicit URL or a byte-range on the base URL).
    pub fn has_initialization(&self) -> bool {
        self.initialization.is_some() || self.range.is_some()
    }

    /// Returns `true` when the segment info type implies individually
    /// addressable media segments (i.e. not BaseURL / SegmentBase with only an
    /// index range).
    pub fn has_segments(&self) -> bool {
        match self.segment_info_type.as_deref() {
            Some(constants::BASE_URL) | Some(constants::SEGMENT_BASE) => false,
            _ => self.index_range.is_none(),
        }
    }
}
