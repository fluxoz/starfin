//! Port of `dash.js/src/dash/vo/ContentProtection.js`.

use serde::{Deserialize, Serialize};

use super::descriptor_type::DescriptorType;

/// Certificate URL descriptor.
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct CertUrl {
    pub url: String,
    pub cert_type: Option<String>,
}

/// A ContentProtection descriptor, extending [`DescriptorType`].
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct ContentProtection {
    // Inherited from DescriptorType
    pub scheme_id_uri: Option<String>,
    pub value: Option<String>,
    pub id: Option<String>,

    // ContentProtection-specific fields
    #[serde(rename = "ref")]
    pub ref_: Option<String>,
    pub ref_id: Option<String>,
    pub robustness: Option<String>,
    pub key_id: Option<String>,
    pub cenc_default_kid: Option<String>,
    pub pssh: Option<String>,
    pub pro: Option<String>,
    pub la_url: Option<String>,
    pub cert_urls: Vec<CertUrl>,
}

impl ContentProtection {
    /// Merge attributes from a referenced ContentProtection, filling in any
    /// `None` fields from the reference (mirrors `mergeAttributesFromReference`
    /// in dash.js).
    pub fn merge_attributes_from_reference(&mut self, reference: &ContentProtection) {
        macro_rules! merge_opt {
            ($field:ident) => {
                if self.$field.is_none() {
                    self.$field = reference.$field.clone();
                }
            };
        }
        merge_opt!(scheme_id_uri);
        merge_opt!(value);
        merge_opt!(id);
        merge_opt!(robustness);
        merge_opt!(cenc_default_kid);
        merge_opt!(pro);
        merge_opt!(pssh);
        merge_opt!(la_url);

        // Merge cert_urls: append any from reference not already present.
        for rc in &reference.cert_urls {
            let key = format!("{}||{}", rc.url, rc.cert_type.as_deref().unwrap_or(""));
            let already = self.cert_urls.iter().any(|c| {
                format!("{}||{}", c.url, c.cert_type.as_deref().unwrap_or("")) == key
            });
            if !already {
                self.cert_urls.push(rc.clone());
            }
        }
    }

    /// Convert to a plain [`DescriptorType`].
    pub fn as_descriptor_type(&self) -> DescriptorType {
        DescriptorType {
            scheme_id_uri: self.scheme_id_uri.clone(),
            value: self.value.clone(),
            id: self.id.clone(),
            ..DescriptorType::default()
        }
    }
}
