//! Port of `dash.js/src/streaming/XlinkLoader.js`.
//!
//! Loads xlink-referenced XML fragments over HTTP (simulated synchronously).

pub const RESOLVE_TO_ZERO: &str = "urn:mpeg:dash:resolve-to-zero:2013";

/// Result of a single xlink load attempt.
#[derive(Clone, Debug)]
pub struct XlinkLoadResult {
    pub url: String,
    pub content: Option<String>,
    pub resolve_to_zero: bool,
}

/// Manages pending xlink loads. In the Rust port loading is performed by the
/// caller; this struct tracks state and exposes helpers used by
/// `XlinkController`.
#[derive(Clone, Debug, Default)]
pub struct XlinkLoader {
    pending_urls: Vec<String>,
    aborted: bool,
}

impl XlinkLoader {
    pub fn new() -> Self {
        Self::default()
    }

    /// Registers a URL for loading. Returns `true` when the URL resolves to
    /// zero (the caller should treat the element as removed) and `false` when
    /// an actual HTTP request is required.
    pub fn load(&mut self, url: &str) -> bool {
        if url == RESOLVE_TO_ZERO {
            return true;
        }
        if !self.pending_urls.contains(&url.to_string()) {
            self.pending_urls.push(url.to_string());
        }
        false
    }

    /// Completes a load by supplying the retrieved content for `url`.
    /// Returns the constructed `XlinkLoadResult`.
    pub fn complete(&mut self, url: &str, content: Option<String>) -> XlinkLoadResult {
        self.pending_urls.retain(|u| u != url);
        XlinkLoadResult { url: url.to_string(), content, resolve_to_zero: false }
    }

    pub fn get_pending_urls(&self) -> &[String] {
        &self.pending_urls
    }

    pub fn abort(&mut self) {
        self.pending_urls.clear();
        self.aborted = true;
    }

    pub fn reset(&mut self) {
        self.pending_urls.clear();
        self.aborted = false;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn resolve_to_zero_returns_true() {
        let mut loader = XlinkLoader::new();
        assert!(loader.load(RESOLVE_TO_ZERO));
        assert!(loader.get_pending_urls().is_empty());
    }

    #[test]
    fn normal_url_queued() {
        let mut loader = XlinkLoader::new();
        assert!(!loader.load("http://example.com/period.xml"));
        assert_eq!(loader.get_pending_urls().len(), 1);
    }

    #[test]
    fn complete_removes_from_pending() {
        let mut loader = XlinkLoader::new();
        loader.load("http://example.com/x.xml");
        let result = loader.complete("http://example.com/x.xml", Some("<Period/>".to_string()));
        assert!(result.content.is_some());
        assert!(loader.get_pending_urls().is_empty());
    }
}
