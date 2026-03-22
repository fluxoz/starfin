//! Port of `dash.js/src/dash/vo/Period.js`.

use serde::{Deserialize, Serialize};

use super::adaptation_set::AdaptationSet;
use super::base_url::BaseUrl;

/// Default period id when none is specified in the manifest.
pub const DEFAULT_ID: &str = "defaultId";

/// A Period within an MPD.
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(default)]
pub struct Period {
    pub id: Option<String>,
    pub index: i32,
    pub start: Option<f64>,
    pub duration: Option<f64>,

    /// Index of the owning MPD (avoids circular references).
    pub mpd_index: Option<usize>,
    pub next_period_id: Option<String>,
    pub is_encrypted: bool,

    // Child elements
    pub adaptation_sets: Vec<AdaptationSet>,
    pub base_urls: Vec<BaseUrl>,
}

impl Default for Period {
    fn default() -> Self {
        Self {
            id: None,
            index: -1,
            start: None,
            duration: None,
            mpd_index: None,
            next_period_id: None,
            is_encrypted: false,
            adaptation_sets: Vec::new(),
            base_urls: Vec::new(),
        }
    }
}
