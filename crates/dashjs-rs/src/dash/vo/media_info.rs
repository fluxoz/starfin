//! Port of `dash.js/src/dash/vo/MediaInfo.js`.

use serde::{Deserialize, Serialize};
use std::collections::HashSet;

use super::content_protection::ContentProtection;
use super::descriptor_type::DescriptorType;
use super::stream_info::StreamInfo;

/// Bitrate entry for a representation.
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct BitrateListEntry {
    pub bandwidth: Option<u64>,
    pub width: Option<u32>,
    pub height: Option<u32>,
    pub scan_type: Option<String>,
    pub id: Option<String>,
}

/// Aggregated media information for a track / adaptation set.
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(default)]
pub struct MediaInfo {
    pub id: Option<String>,
    pub index: Option<u32>,
    #[serde(rename = "type")]
    pub type_: Option<String>,
    pub stream_info: Option<StreamInfo>,

    pub representation_count: u32,
    pub lang: Option<String>,
    pub labels: Option<Vec<String>>,
    pub roles: Option<Vec<DescriptorType>>,
    pub accessibility: Option<Vec<DescriptorType>>,
    pub audio_channel_configuration: Option<Vec<DescriptorType>>,
    pub supplemental_properties: Vec<DescriptorType>,
    pub essential_properties: Vec<DescriptorType>,
    pub viewpoint: Option<Vec<DescriptorType>>,

    pub is_text: bool,
    pub is_embedded: Option<bool>,
    pub is_fragmented: Option<bool>,
    pub is_preselection: bool,

    pub codec: Option<String>,
    pub mime_type: Option<String>,
    pub content_protection: Option<Vec<ContentProtection>>,
    pub bitrate_list: Option<Vec<BitrateListEntry>>,

    pub segment_alignment: bool,
    pub sub_segment_alignment: bool,
    pub selection_priority: u32,

    pub adaptation_set_switching_compatible_ids: Vec<String>,
    pub segment_sequence_properties: Vec<serde_json::Value>,

    /// Normalised key IDs encountered across content-protection descriptors.
    #[serde(skip)]
    pub normalized_key_ids: HashSet<String>,
}

impl Default for MediaInfo {
    fn default() -> Self {
        Self {
            id: None,
            index: None,
            type_: None,
            stream_info: None,
            representation_count: 0,
            lang: None,
            labels: None,
            roles: None,
            accessibility: None,
            audio_channel_configuration: None,
            supplemental_properties: Vec::new(),
            essential_properties: Vec::new(),
            viewpoint: None,
            is_text: false,
            is_embedded: None,
            is_fragmented: None,
            is_preselection: false,
            codec: None,
            mime_type: None,
            content_protection: None,
            bitrate_list: None,
            segment_alignment: false,
            sub_segment_alignment: false,
            selection_priority: 1,
            adaptation_set_switching_compatible_ids: Vec::new(),
            segment_sequence_properties: Vec::new(),
            normalized_key_ids: HashSet::new(),
        }
    }
}
