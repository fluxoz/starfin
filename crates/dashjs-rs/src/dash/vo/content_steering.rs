//! Port of `dash.js/src/dash/vo/ContentSteering.js`.

use serde::{Deserialize, Serialize};

/// Content Steering configuration from the MPD.
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(default)]
pub struct ContentSteering {
    pub default_service_location: Option<String>,
    pub default_service_location_array: Vec<String>,
    pub query_before_start: bool,
    pub server_url: Option<String>,
    pub client_requirement: bool,
}

impl Default for ContentSteering {
    fn default() -> Self {
        Self {
            default_service_location: None,
            default_service_location_array: Vec::new(),
            query_before_start: false,
            server_url: None,
            client_requirement: true,
        }
    }
}
