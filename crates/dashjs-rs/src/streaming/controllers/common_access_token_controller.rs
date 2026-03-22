//! Port of `dash.js/src/streaming/controllers/CommonAccessTokenController.js`.
//!
//! Stores Common-Access-Token (CAT) response headers keyed by host name and
//! returns the appropriate token for outgoing requests.

use std::collections::HashMap;

/// Extracts the hostname from a URL string.
fn host_from_url(url: &str) -> Option<String> {
    // Minimal host extraction: handle http(s)://host[:port]/path patterns.
    let without_scheme = url.strip_prefix("https://").or_else(|| url.strip_prefix("http://"))?;
    let host_end = without_scheme.find(['/', '?', '#']).unwrap_or(without_scheme.len());
    let host = &without_scheme[..host_end];
    if host.is_empty() { None } else { Some(host.to_string()) }
}

/// The HTTP response header name defined by the DASH-IF CAT specification.
pub const COMMON_ACCESS_TOKEN_HEADER: &str = "common-access-token";

/// A minimal HTTP response representation used by `process_response_headers`.
#[derive(Clone, Debug, Default)]
pub struct HttpResponse {
    /// The URL of the original request.
    pub url: String,
    /// Headers from the response. Key should be lower-case.
    pub headers: HashMap<String, String>,
}

/// Tracks Common-Access-Token values per host and provides them for outgoing
/// requests.
#[derive(Clone, Debug, Default)]
pub struct CommonAccessTokenController {
    host_token_map: HashMap<String, String>,
}

impl CommonAccessTokenController {
    pub fn new() -> Self {
        Self::default()
    }

    /// Examines an HTTP response for a `common-access-token` header and stores
    /// the token keyed by the request host.
    pub fn process_response_headers(&mut self, response: &HttpResponse) {
        if response.url.is_empty() {
            return;
        }
        if let Some(token) = response.headers.get(COMMON_ACCESS_TOKEN_HEADER) {
            if let Some(host) = host_from_url(&response.url) {
                self.host_token_map.insert(host, token.clone());
            }
        }
    }

    /// Returns the stored token for the host of `url`, or `None`.
    pub fn get_common_access_token_for_url(&self, url: &str) -> Option<&str> {
        if url.is_empty() {
            return None;
        }
        let host = host_from_url(url)?;
        self.host_token_map.get(&host).map(|s| s.as_str())
    }

    pub fn reset(&mut self) {
        self.host_token_map.clear();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn stores_and_retrieves_token() {
        let mut ctrl = CommonAccessTokenController::new();
        let mut headers = HashMap::new();
        headers.insert(COMMON_ACCESS_TOKEN_HEADER.to_string(), "tok123".to_string());
        ctrl.process_response_headers(&HttpResponse {
            url: "https://cdn.example.com/seg.mp4".to_string(),
            headers,
        });
        assert_eq!(ctrl.get_common_access_token_for_url("https://cdn.example.com/other.mp4"), Some("tok123"));
    }

    #[test]
    fn different_hosts_independent() {
        let mut ctrl = CommonAccessTokenController::new();
        let mut h1 = HashMap::new();
        h1.insert(COMMON_ACCESS_TOKEN_HEADER.to_string(), "tokA".to_string());
        ctrl.process_response_headers(&HttpResponse { url: "https://a.example.com/x".to_string(), headers: h1 });

        let mut h2 = HashMap::new();
        h2.insert(COMMON_ACCESS_TOKEN_HEADER.to_string(), "tokB".to_string());
        ctrl.process_response_headers(&HttpResponse { url: "https://b.example.com/x".to_string(), headers: h2 });

        assert_eq!(ctrl.get_common_access_token_for_url("https://a.example.com/seg"), Some("tokA"));
        assert_eq!(ctrl.get_common_access_token_for_url("https://b.example.com/seg"), Some("tokB"));
    }

    #[test]
    fn unknown_host_returns_none() {
        let ctrl = CommonAccessTokenController::new();
        assert!(ctrl.get_common_access_token_for_url("https://unknown.example.com/seg").is_none());
    }

    #[test]
    fn reset_clears_tokens() {
        let mut ctrl = CommonAccessTokenController::new();
        let mut h = HashMap::new();
        h.insert(COMMON_ACCESS_TOKEN_HEADER.to_string(), "tok".to_string());
        ctrl.process_response_headers(&HttpResponse { url: "https://cdn.example.com/x".to_string(), headers: h });
        ctrl.reset();
        assert!(ctrl.get_common_access_token_for_url("https://cdn.example.com/x").is_none());
    }
}
