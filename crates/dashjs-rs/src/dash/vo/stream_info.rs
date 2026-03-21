//! Port of `dash.js/src/dash/vo/StreamInfo.js`.

use serde::{Deserialize, Serialize};

use super::manifest_info::ManifestInfo;

/// Information about a single stream (period) from the player's perspective.
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(default)]
pub struct StreamInfo {
    pub id: Option<String>,
    pub index: Option<u32>,
    pub start: Option<f64>,
    pub duration: Option<f64>,
    pub manifest_info: Option<Box<ManifestInfo>>,
    pub is_last: bool,
    pub is_encrypted: bool,
}

impl Default for StreamInfo {
    fn default() -> Self {
        Self {
            id: None,
            index: None,
            start: None,
            duration: None,
            manifest_info: None,
            is_last: true,
            is_encrypted: false,
        }
    }
}
