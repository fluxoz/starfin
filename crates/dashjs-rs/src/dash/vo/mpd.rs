//! Port of `dash.js/src/dash/vo/Mpd.js`.

use serde::{Deserialize, Serialize};

use super::base_url::BaseUrl;
use super::content_steering::ContentSteering;
use super::mpd_location::MpdLocation;
use super::patch_location::PatchLocation;
use super::period::Period;
use super::utc_timing::UtcTiming;

/// Presentation type — static (on-demand) or dynamic (live).
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum PresentationType {
    #[default]
    Static,
    Dynamic,
}

/// Top-level MPD (Media Presentation Description) value object.
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(default)]
pub struct Mpd {
    pub id: Option<String>,
    pub profiles: Option<String>,
    #[serde(rename = "type")]
    pub type_: PresentationType,

    pub availability_start_time: Option<String>,
    /// Defaults to +∞ in dash.js.
    pub availability_end_time: Option<f64>,

    pub publish_time: Option<String>,
    pub media_presentation_duration: Option<f64>,
    pub min_buffer_time: Option<f64>,
    pub minimum_update_period: Option<f64>,
    /// Defaults to +∞ in dash.js.
    pub time_shift_buffer_depth: Option<f64>,
    /// Defaults to +∞ in dash.js.
    pub max_segment_duration: Option<f64>,
    pub suggested_presentation_delay: Option<f64>,

    // Child elements
    pub periods: Vec<Period>,
    pub utc_timing: Vec<UtcTiming>,
    pub base_urls: Vec<BaseUrl>,
    pub locations: Vec<MpdLocation>,
    pub service_descriptions: Vec<ServiceDescription>,
    pub patch_location: Vec<PatchLocation>,
    pub content_steering: Option<ContentSteering>,
}

impl Default for Mpd {
    fn default() -> Self {
        Self {
            id: None,
            profiles: None,
            type_: PresentationType::Static,
            availability_start_time: None,
            availability_end_time: None,
            publish_time: None,
            media_presentation_duration: None,
            min_buffer_time: None,
            minimum_update_period: None,
            time_shift_buffer_depth: None,
            max_segment_duration: None,
            suggested_presentation_delay: Some(0.0),
            periods: Vec::new(),
            utc_timing: Vec::new(),
            base_urls: Vec::new(),
            locations: Vec::new(),
            service_descriptions: Vec::new(),
            patch_location: Vec::new(),
            content_steering: None,
        }
    }
}

/// Lightweight service description that lives inside the MPD.
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct ServiceDescription {
    pub id: Option<String>,
    pub scheme_id_uri: Option<String>,
    pub latency: Option<ServiceDescriptionLatency>,
    pub playback_rate: Option<ServiceDescriptionPlaybackRate>,
    pub operating_quality: Option<ServiceDescriptionOperatingQuality>,
    pub operating_bandwidth: Option<ServiceDescriptionOperatingBandwidth>,
}

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct ServiceDescriptionLatency {
    pub target: Option<u64>,
    pub max: Option<u64>,
    pub min: Option<u64>,
    pub reference_id: Option<u64>,
}

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct ServiceDescriptionPlaybackRate {
    pub max: Option<f64>,
    pub min: Option<f64>,
}

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct ServiceDescriptionOperatingQuality {
    pub media_type: Option<String>,
    pub max: Option<u64>,
    pub min: Option<u64>,
    pub target: Option<u64>,
    #[serde(rename = "type")]
    pub type_: Option<String>,
    pub max_difference: Option<u64>,
}

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct ServiceDescriptionOperatingBandwidth {
    pub media_type: Option<String>,
    pub max: Option<u64>,
    pub min: Option<u64>,
    pub target: Option<u64>,
}
