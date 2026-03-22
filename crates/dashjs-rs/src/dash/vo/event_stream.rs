//! Port of `dash.js/src/dash/vo/Event.js` and `EventStream.js`.

use serde::{Deserialize, Serialize};

/// A single event within an EventStream.
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(default)]
pub struct Event {
    #[serde(rename = "type")]
    pub type_: String,
    pub duration: Option<f64>,
    pub presentation_time: Option<f64>,
    pub id: Option<u64>,
    pub message_data: String,
    /// Index of the parent EventStream (avoids circular reference).
    pub event_stream_index: Option<usize>,
    /// Specific EMSG box parameter.
    pub presentation_time_delta: Option<f64>,
    /// Parsed value of the event message.
    pub parsed_message_data: Option<serde_json::Value>,
}

impl Default for Event {
    fn default() -> Self {
        Self {
            type_: String::new(),
            duration: None,
            presentation_time: None,
            id: None,
            message_data: String::new(),
            event_stream_index: None,
            presentation_time_delta: None,
            parsed_message_data: None,
        }
    }
}

/// An EventStream element from the MPD or an inband event stream.
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(default)]
pub struct EventStream {
    /// Index of the parent AdaptationSet (if applicable).
    pub adaptation_set_index: Option<usize>,
    /// Index of the parent Representation (if applicable).
    pub representation_index: Option<usize>,
    /// Index of the parent Period.
    pub period_index: Option<usize>,

    pub timescale: u64,
    pub value: String,
    pub scheme_id_uri: String,
    pub presentation_time_offset: f64,
}

impl Default for EventStream {
    fn default() -> Self {
        Self {
            adaptation_set_index: None,
            representation_index: None,
            period_index: None,
            timescale: 1,
            value: String::new(),
            scheme_id_uri: String::new(),
            presentation_time_offset: 0.0,
        }
    }
}
