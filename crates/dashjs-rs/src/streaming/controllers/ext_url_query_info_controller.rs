//! Port of `dash.js/src/streaming/controllers/ExtUrlQueryInfoController.js`.
//!
//! Builds the final query-parameter strings that should be appended to
//! segment, MPD, and content-steering requests based on `ExtUrlQueryInfo` /
//! `UrlQueryInfo` descriptors found in the MPD.

use std::collections::HashMap;

/// Whether a descriptor comes from `UrlQueryInfo` or `ExtUrlQueryInfo`.
#[derive(Clone, Debug, PartialEq)]
pub enum QueryInfoKind {
    UrlQueryInfo,
    ExtUrlQueryInfo,
}

/// A resolved query-info descriptor from an MPD element.
#[derive(Clone, Debug, Default)]
pub struct QueryInfoDescriptor {
    pub scheme_id_uri: String,
    pub kind: Option<QueryInfoKind>,
    pub query_string: Option<String>,
    pub query_template: Option<String>,
    pub use_mpd_url_query: bool,
    pub same_origin_only: bool,
    pub include_in_requests: Vec<String>,
}

/// Request types mirrored from `HTTPRequest`.
pub const REQUEST_TYPE_MPD: &str = "MPD";
pub const REQUEST_TYPE_MEDIA_SEGMENT: &str = "MediaSegment";
pub const REQUEST_TYPE_INIT_SEGMENT: &str = "InitializationSegment";
pub const REQUEST_TYPE_CONTENT_STEERING: &str = "ContentSteering";

/// Resolved query-parameter information for a single MPD level.
#[derive(Clone, Debug, Default)]
pub struct QueryParamInfo {
    pub initial_query_string: String,
    pub final_query_string: String,
    pub query_params: HashMap<String, String>,
    pub same_origin_only: bool,
    pub include_in_requests: Vec<String>,
}

/// Builds and caches the query-string information derived from the MPD
/// hierarchy for later use when constructing network requests.
#[derive(Clone, Debug, Default)]
pub struct ExtUrlQueryInfoController {
    mpd_query_string_information: Option<MpdQueryInfo>,
}

#[derive(Clone, Debug, Default)]
pub struct MpdQueryInfo {
    pub origin: String,
    pub final_query_string: String,
    pub query_params: HashMap<String, String>,
    pub same_origin_only: bool,
    pub include_in_requests: Vec<String>,
    pub periods: Vec<PeriodQueryInfo>,
}

#[derive(Clone, Debug, Default)]
pub struct PeriodQueryInfo {
    pub final_query_string: String,
    pub query_params: HashMap<String, String>,
    pub same_origin_only: bool,
    pub include_in_requests: Vec<String>,
    pub adaptations: Vec<AdaptationQueryInfo>,
}

#[derive(Clone, Debug, Default)]
pub struct AdaptationQueryInfo {
    pub final_query_string: String,
    pub query_params: HashMap<String, String>,
    pub same_origin_only: bool,
    pub include_in_requests: Vec<String>,
    pub representations: Vec<RepresentationQueryInfo>,
}

#[derive(Clone, Debug, Default)]
pub struct RepresentationQueryInfo {
    pub final_query_string: String,
    pub query_params: HashMap<String, String>,
    pub same_origin_only: bool,
    pub include_in_requests: Vec<String>,
}

/// Parses a query string like `"foo=1&bar=2"` into a map.
pub fn parse_query_params(qs: &str) -> HashMap<String, String> {
    let mut map = HashMap::new();
    if qs.is_empty() {
        return map;
    }
    for pair in qs.split('&') {
        let mut kv = pair.splitn(2, '=');
        let k = kv.next().unwrap_or("").to_string();
        let v = kv.next().unwrap_or("").to_string();
        if !k.is_empty() {
            map.insert(k, v);
        }
    }
    map
}

/// Extracts the origin (scheme + host + port) from a URL string.
fn origin_from_url(url: &str) -> String {
    // Find the authority section.
    let after_scheme = if let Some(s) = url.find("://") {
        &url[s + 3..]
    } else {
        return url.to_string();
    };
    let end = after_scheme.find('/').unwrap_or(after_scheme.len());
    let scheme_end = url.find("://").unwrap();
    format!("{}://{}", &url[..scheme_end], &after_scheme[..end])
}

/// Computes the merged query string for a single MPD hierarchy level given the
/// descriptor, the parent-level initial string, and the MPD URL query.
fn build_query_info(
    descriptor: Option<&QueryInfoDescriptor>,
    parent_initial: &str,
    parent_final: &str,
    parent_include: &[String],
    mpd_url_query: &str,
) -> QueryParamInfo {
    let mut result = QueryParamInfo::default();

    // -- initial query string --
    let initial = if let Some(d) = descriptor {
        let qs = d.query_string.as_deref().unwrap_or("");
        let merged = if !qs.is_empty() {
            if !parent_initial.is_empty() {
                format!("{}&{}", parent_initial, qs)
            } else {
                qs.to_string()
            }
        } else {
            parent_initial.to_string()
        };
        let with_mpd = if d.use_mpd_url_query && !mpd_url_query.is_empty() {
            if !merged.is_empty() {
                format!("{}&{}", merged, mpd_url_query)
            } else {
                mpd_url_query.to_string()
            }
        } else {
            merged
        };
        with_mpd
    } else {
        parent_initial.to_string()
    };
    result.initial_query_string = initial.clone();

    // -- final query string --
    let final_qs = if let Some(d) = descriptor {
        let tmpl = d.query_template.as_deref().unwrap_or("");
        if tmpl == "$querypart$" {
            initial.clone()
        } else {
            String::new()
        }
    } else {
        parent_final.to_string()
    };
    result.final_query_string = final_qs.clone();
    result.query_params = parse_query_params(&final_qs);

    // -- same-origin-only --
    result.same_origin_only = descriptor.map(|d| d.same_origin_only).unwrap_or(false);

    // -- include-in-requests --
    result.include_in_requests = if let Some(d) = descriptor {
        if !d.include_in_requests.is_empty() {
            d.include_in_requests.clone()
        } else {
            vec!["Segment".to_string()]
        }
    } else {
        parent_include.to_vec()
    };

    result
}

impl ExtUrlQueryInfoController {
    pub fn new() -> Self {
        Self::default()
    }

    /// Builds and caches query-parameter information for every level of the
    /// provided MPD structure.
    ///
    /// `manifest_url` is the URL from which the MPD was loaded.
    /// `periods` is a slice of period descriptors.
    pub fn create_final_query_strings(
        &mut self,
        manifest_url: &str,
        manifest_descriptor: Option<&QueryInfoDescriptor>,
        periods: &[PeriodInput],
    ) {
        let mpd_url_query = manifest_url.splitn(2, '?').nth(1).unwrap_or("");
        let origin = origin_from_url(manifest_url);

        let mpd_info = build_query_info(manifest_descriptor, "", "", &[], mpd_url_query);

        let mut period_infos = Vec::new();
        for period in periods {
            let period_qi = build_query_info(
                period.descriptor.as_ref(),
                &mpd_info.initial_query_string,
                &mpd_info.final_query_string,
                &mpd_info.include_in_requests,
                mpd_url_query,
            );
            let mut adaptation_infos = Vec::new();
            for adaptation in &period.adaptations {
                let adapt_qi = build_query_info(
                    adaptation.descriptor.as_ref(),
                    &period_qi.initial_query_string,
                    &period_qi.final_query_string,
                    &period_qi.include_in_requests,
                    mpd_url_query,
                );
                let mut repr_infos = Vec::new();
                for representation in &adaptation.representations {
                    let repr_qi = build_query_info(
                        representation.descriptor.as_ref(),
                        &adapt_qi.initial_query_string,
                        &adapt_qi.final_query_string,
                        &adapt_qi.include_in_requests,
                        mpd_url_query,
                    );
                    repr_infos.push(RepresentationQueryInfo {
                        final_query_string: repr_qi.final_query_string,
                        query_params: repr_qi.query_params,
                        same_origin_only: repr_qi.same_origin_only,
                        include_in_requests: repr_qi.include_in_requests,
                    });
                }
                adaptation_infos.push(AdaptationQueryInfo {
                    final_query_string: adapt_qi.final_query_string,
                    query_params: adapt_qi.query_params,
                    same_origin_only: adapt_qi.same_origin_only,
                    include_in_requests: adapt_qi.include_in_requests,
                    representations: repr_infos,
                });
            }
            period_infos.push(PeriodQueryInfo {
                final_query_string: period_qi.final_query_string,
                query_params: period_qi.query_params,
                same_origin_only: period_qi.same_origin_only,
                include_in_requests: period_qi.include_in_requests,
                adaptations: adaptation_infos,
            });
        }

        self.mpd_query_string_information = Some(MpdQueryInfo {
            origin,
            final_query_string: mpd_info.final_query_string,
            query_params: mpd_info.query_params,
            same_origin_only: mpd_info.same_origin_only,
            include_in_requests: mpd_info.include_in_requests,
            periods: period_infos,
        });
    }

    /// Returns the query parameters to append to a request of the given type.
    ///
    /// `period_idx`, `adaptation_idx`, `representation_idx` are only used for
    /// segment requests.
    pub fn get_final_query_string(
        &self,
        request_type: &str,
        request_url: &str,
        period_idx: usize,
        adaptation_idx: usize,
        representation_idx: usize,
    ) -> Option<&HashMap<String, String>> {
        let info = self.mpd_query_string_information.as_ref()?;

        match request_type {
            REQUEST_TYPE_MEDIA_SEGMENT | REQUEST_TYPE_INIT_SEGMENT => {
                let qi = info
                    .periods
                    .get(period_idx)?
                    .adaptations
                    .get(adaptation_idx)?
                    .representations
                    .get(representation_idx)?;
                let req_origin = origin_from_url(request_url);
                let can_send = !qi.same_origin_only || info.origin == req_origin;
                let in_request = qi.include_in_requests.iter().any(|r| r == "Segment");
                if in_request && can_send {
                    Some(&qi.query_params)
                } else {
                    None
                }
            }
            REQUEST_TYPE_MPD => {
                let in_request = info
                    .include_in_requests
                    .iter()
                    .any(|r| r == "MPD" || r == "MPDPatch");
                if in_request { Some(&info.query_params) } else { None }
            }
            REQUEST_TYPE_CONTENT_STEERING => {
                let in_request = info.include_in_requests.iter().any(|r| r == "Steering");
                if in_request { Some(&info.query_params) } else { None }
            }
            _ => None,
        }
    }

    pub fn reset(&mut self) {
        self.mpd_query_string_information = None;
    }
}

// ---------------------------------------------------------------------------
// Input types used when building the query-string cache.
// ---------------------------------------------------------------------------

#[derive(Clone, Debug, Default)]
pub struct RepresentationInput {
    pub descriptor: Option<QueryInfoDescriptor>,
}

#[derive(Clone, Debug, Default)]
pub struct AdaptationInput {
    pub descriptor: Option<QueryInfoDescriptor>,
    pub representations: Vec<RepresentationInput>,
}

#[derive(Clone, Debug, Default)]
pub struct PeriodInput {
    pub descriptor: Option<QueryInfoDescriptor>,
    pub adaptations: Vec<AdaptationInput>,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_query_params_basic() {
        let params = parse_query_params("foo=1&bar=2");
        assert_eq!(params.get("foo"), Some(&"1".to_string()));
        assert_eq!(params.get("bar"), Some(&"2".to_string()));
    }

    #[test]
    fn empty_input_returns_none() {
        let ctrl = ExtUrlQueryInfoController::new();
        assert!(ctrl
            .get_final_query_string(REQUEST_TYPE_MPD, "https://example.com/manifest.mpd", 0, 0, 0)
            .is_none());
    }

    #[test]
    fn origin_extraction() {
        assert_eq!(origin_from_url("https://cdn.example.com/path/file.mpd"), "https://cdn.example.com");
        assert_eq!(origin_from_url("http://cdn.example.com:8080/x"), "http://cdn.example.com:8080");
    }
}
