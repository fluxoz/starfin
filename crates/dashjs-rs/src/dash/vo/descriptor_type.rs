//! Port of `dash.js/src/dash/vo/DescriptorType.js`.

use serde::{Deserialize, Serialize};

/// Generic DASH descriptor (schemeIdUri + value + id).
///
/// Used for Role, Accessibility, SupplementalProperty, EssentialProperty,
/// AudioChannelConfiguration, and many other descriptor elements.
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct DescriptorType {
    pub scheme_id_uri: Option<String>,
    pub value: Option<String>,
    pub id: Option<String>,

    // DVB extensions — only present when the corresponding attributes exist.
    pub dvb_url: Option<String>,
    pub dvb_mime_type: Option<String>,
    pub dvb_font_family: Option<String>,
}

impl DescriptorType {
    /// Initialise from raw attribute values (mirrors `init(data)` in dash.js).
    pub fn init(
        scheme_id_uri: Option<&str>,
        value: Option<&str>,
        id: Option<&str>,
    ) -> Self {
        Self {
            scheme_id_uri: scheme_id_uri.map(String::from),
            value: value.map(String::from),
            id: id.map(String::from),
            dvb_url: None,
            dvb_mime_type: None,
            dvb_font_family: None,
        }
    }

    /// Returns `true` when this descriptor's `schemeIdUri` + `value` matches
    /// any entry in `arr` (where each entry's `value` is treated as a regex
    /// pattern, mirroring the JS `inArray` method).
    pub fn in_array(&self, arr: &[DescriptorType]) -> bool {
        arr.iter().any(|entry| {
            if self.scheme_id_uri != entry.scheme_id_uri {
                return false;
            }
            match (&self.value, &entry.value) {
                (Some(v), Some(pattern)) => v.contains(pattern.as_str()),
                (None, Some(pattern)) => pattern.is_empty(),
                _ => true,
            }
        })
    }
}
