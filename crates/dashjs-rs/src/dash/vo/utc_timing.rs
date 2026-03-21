//! Port of `dash.js/src/dash/vo/UTCTiming.js`.

use serde::{Deserialize, Serialize};

/// UTCTiming descriptor element.
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(default)]
pub struct UtcTiming {
    pub scheme_id_uri: String,
    pub value: String,
}

impl Default for UtcTiming {
    fn default() -> Self {
        Self {
            scheme_id_uri: String::new(),
            value: String::new(),
        }
    }
}
