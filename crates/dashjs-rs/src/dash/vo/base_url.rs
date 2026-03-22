//! Port of `dash.js/src/dash/vo/BaseURL.js`.

use serde::{Deserialize, Serialize};
use std::collections::HashMap;

pub const DEFAULT_DVB_PRIORITY: u32 = 1;
pub const DEFAULT_DVB_WEIGHT: u32 = 1;

/// A BaseURL element from the MPD.
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(default)]
pub struct BaseUrl {
    pub url: String,
    pub service_location: String,

    // DVB extensions
    pub dvb_priority: u32,
    pub dvb_weight: u32,

    pub availability_time_offset: f64,
    pub availability_time_complete: bool,

    /// Query parameters synthesized during content steering.
    pub query_params: HashMap<String, String>,
}

impl Default for BaseUrl {
    fn default() -> Self {
        Self {
            url: String::new(),
            service_location: String::new(),
            dvb_priority: DEFAULT_DVB_PRIORITY,
            dvb_weight: DEFAULT_DVB_WEIGHT,
            availability_time_offset: 0.0,
            availability_time_complete: true,
            query_params: HashMap::new(),
        }
    }
}

impl BaseUrl {
    pub fn new(url: impl Into<String>) -> Self {
        let u = url.into();
        let sl = u.clone();
        Self {
            url: u,
            service_location: sl,
            ..Self::default()
        }
    }

    pub fn with_service_location(
        url: impl Into<String>,
        service_location: impl Into<String>,
        priority: Option<u32>,
        weight: Option<u32>,
    ) -> Self {
        Self {
            url: url.into(),
            service_location: service_location.into(),
            dvb_priority: priority.unwrap_or(DEFAULT_DVB_PRIORITY),
            dvb_weight: weight.unwrap_or(DEFAULT_DVB_WEIGHT),
            ..Self::default()
        }
    }
}
