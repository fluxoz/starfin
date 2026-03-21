//! Port of `dash.js/src/dash/vo/AdaptationSet.js`.

use serde::{Deserialize, Serialize};

use super::base_url::BaseUrl;
use super::content_protection::ContentProtection;
use super::descriptor_type::DescriptorType;
use super::representation::Representation;

/// An AdaptationSet within a Period.
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(default)]
pub struct AdaptationSet {
    pub id: Option<String>,
    pub index: i32,
    /// Index of the parent Period.
    pub period_index: Option<usize>,
    #[serde(rename = "type")]
    pub type_: Option<String>,

    // Common attributes
    pub content_type: Option<String>,
    pub mime_type: Option<String>,
    pub codecs: Option<String>,
    pub lang: Option<String>,
    pub group: Option<u32>,
    pub par: Option<String>,
    pub max_width: Option<u32>,
    pub max_height: Option<u32>,
    pub max_frame_rate: Option<String>,
    pub segment_alignment: bool,
    pub subsegment_alignment: bool,
    pub bitstream_switching: bool,

    // Descriptor elements
    pub content_protection: Vec<ContentProtection>,
    pub role: Vec<DescriptorType>,
    pub accessibility: Vec<DescriptorType>,
    pub supplemental_property: Vec<DescriptorType>,
    pub essential_property: Vec<DescriptorType>,
    pub audio_channel_configuration: Vec<DescriptorType>,

    // Children
    pub representations: Vec<Representation>,
    pub base_urls: Vec<BaseUrl>,

    // Segment information (at most one of these is present)
    pub segment_template: Option<serde_json::Value>,
    pub segment_base: Option<serde_json::Value>,
    pub segment_list: Option<serde_json::Value>,
}

impl Default for AdaptationSet {
    fn default() -> Self {
        Self {
            id: None,
            index: -1,
            period_index: None,
            type_: None,
            content_type: None,
            mime_type: None,
            codecs: None,
            lang: None,
            group: None,
            par: None,
            max_width: None,
            max_height: None,
            max_frame_rate: None,
            segment_alignment: false,
            subsegment_alignment: false,
            bitstream_switching: false,
            content_protection: Vec::new(),
            role: Vec::new(),
            accessibility: Vec::new(),
            supplemental_property: Vec::new(),
            essential_property: Vec::new(),
            audio_channel_configuration: Vec::new(),
            representations: Vec::new(),
            base_urls: Vec::new(),
            segment_template: None,
            segment_base: None,
            segment_list: None,
        }
    }
}
