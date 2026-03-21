//! Port of `dash.js/src/dash/vo/ManifestInfo.js`.

use serde::{Deserialize, Serialize};

use super::mpd::ServiceDescription;

/// High-level information derived from the parsed manifest.
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(default)]
pub struct ManifestInfo {
    pub dvr_window_size: Option<f64>,
    pub is_dynamic: bool,
    pub min_buffer_time: Option<f64>,
    pub max_fragment_duration: Option<f64>,
    pub duration: Option<f64>,
    pub available_from: Option<String>,
    pub loaded_time: Option<String>,
    pub protocol: Option<String>,
    pub service_descriptions: Vec<ServiceDescription>,
}

impl Default for ManifestInfo {
    fn default() -> Self {
        Self {
            dvr_window_size: None,
            is_dynamic: false,
            min_buffer_time: None,
            max_fragment_duration: None,
            duration: None,
            available_from: None,
            loaded_time: None,
            protocol: None,
            service_descriptions: Vec::new(),
        }
    }
}
