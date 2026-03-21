//! Port of `dash.js/src/dash/vo/MpdLocation.js`.

use serde::{Deserialize, Serialize};
use std::collections::HashMap;

/// Location element from the MPD.
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(default)]
pub struct MpdLocation {
    pub url: String,
    pub service_location: Option<String>,
    /// Query parameters synthesized during content steering.
    pub query_params: HashMap<String, String>,
}

impl Default for MpdLocation {
    fn default() -> Self {
        Self {
            url: String::new(),
            service_location: None,
            query_params: HashMap::new(),
        }
    }
}
