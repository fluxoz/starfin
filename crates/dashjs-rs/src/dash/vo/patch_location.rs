//! Port of `dash.js/src/dash/vo/PatchLocation.js`.

use serde::{Deserialize, Serialize};
use std::collections::HashMap;

/// PatchLocation element from the MPD.
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(default)]
pub struct PatchLocation {
    pub url: String,
    pub service_location: Option<String>,
    pub ttl: Option<f64>,
    /// Query parameters synthesized during content steering.
    pub query_params: HashMap<String, String>,
}

impl Default for PatchLocation {
    fn default() -> Self {
        Self {
            url: String::new(),
            service_location: None,
            ttl: None,
            query_params: HashMap::new(),
        }
    }
}
