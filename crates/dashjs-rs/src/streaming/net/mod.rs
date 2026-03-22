//! Port of `dash.js/src/streaming/net/`.
//!
//! Network loader traits and implementations.

use serde::{Deserialize, Serialize};
use std::fmt;

// ---------------------------------------------------------------------------
// Error types
// ---------------------------------------------------------------------------

/// Errors that can occur during network loading.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum LoadError {
    /// Request timed out.
    Timeout,
    /// Generic network error with optional message.
    NetworkError(String),
    /// Request was aborted by the caller.
    AbortError,
    /// Server returned an HTTP error status code.
    HttpError(u16),
}

impl fmt::Display for LoadError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            LoadError::Timeout => write!(f, "request timed out"),
            LoadError::NetworkError(msg) => write!(f, "network error: {}", msg),
            LoadError::AbortError => write!(f, "request aborted"),
            LoadError::HttpError(code) => write!(f, "HTTP error {}", code),
        }
    }
}

// ---------------------------------------------------------------------------
// Configuration
// ---------------------------------------------------------------------------

/// Retry configuration for network requests.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct RetryConfig {
    pub max_retries: u32,
    pub retry_delay_ms: u64,
    pub retry_backoff_factor: f64,
}

impl Default for RetryConfig {
    fn default() -> Self {
        Self {
            max_retries: 3,
            retry_delay_ms: 1000,
            retry_backoff_factor: 2.0,
        }
    }
}

impl RetryConfig {
    /// Calculate the delay for the given attempt (0-indexed).
    pub fn delay_for_attempt(&self, attempt: u32) -> u64 {
        let factor = self.retry_backoff_factor.powi(attempt as i32);
        (self.retry_delay_ms as f64 * factor) as u64
    }
}

/// Loader configuration.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct LoaderConfig {
    pub timeout_ms: u64,
    pub retry_config: RetryConfig,
}

impl Default for LoaderConfig {
    fn default() -> Self {
        Self {
            timeout_ms: 15000,
            retry_config: RetryConfig::default(),
        }
    }
}

// ---------------------------------------------------------------------------
// Traits
// ---------------------------------------------------------------------------

/// Trait for loading resources over HTTP.
pub trait HttpLoader: Send {
    fn load(&self, url: &str, range: Option<&str>) -> Result<Vec<u8>, String>;
    fn abort(&self);
}

/// Trait for loading URLs (manifest, segments).
pub trait UrlLoader: Send {
    fn load_manifest(&self, url: &str) -> Result<String, String>;
    fn load_segment(&self, url: &str, range: Option<&str>) -> Result<Vec<u8>, String>;
}

// ---------------------------------------------------------------------------
// FetchLoader — stub for wasm-bindgen fetch API
// ---------------------------------------------------------------------------

/// Stub for a fetch-based loader (to be backed by `wasm-bindgen` `fetch` API).
///
/// In a browser/wasm context this would use `web_sys::Request` / `web_sys::Response`.
/// Here we provide the interface so downstream code can depend on it.
#[derive(Clone, Debug, Default)]
pub struct FetchLoader {
    pub config: LoaderConfig,
}

impl FetchLoader {
    pub fn new(config: LoaderConfig) -> Self {
        Self { config }
    }

    /// Stub — always returns `NetworkError` outside wasm.
    pub fn load(&self, _url: &str, _range: Option<&str>) -> Result<Vec<u8>, LoadError> {
        Err(LoadError::NetworkError(
            "FetchLoader is only available in wasm targets".into(),
        ))
    }

    pub fn abort(&self) {
        // no-op outside wasm
    }
}

// ---------------------------------------------------------------------------
// XhrLoader — stub for XMLHttpRequest-based loading
// ---------------------------------------------------------------------------

/// Stub for an XHR-based loader.
#[derive(Clone, Debug, Default)]
pub struct XhrLoader {
    pub config: LoaderConfig,
}

impl XhrLoader {
    pub fn new(config: LoaderConfig) -> Self {
        Self { config }
    }

    /// Stub — always returns `NetworkError` outside a browser context.
    pub fn load(&self, _url: &str, _range: Option<&str>) -> Result<Vec<u8>, LoadError> {
        Err(LoadError::NetworkError(
            "XhrLoader is only available in browser targets".into(),
        ))
    }

    pub fn abort(&self) {
        // no-op
    }
}

// ---------------------------------------------------------------------------
// SchemeLoaderFactory
// ---------------------------------------------------------------------------

/// Selects a loader implementation based on URL scheme.
#[derive(Clone, Debug, Default)]
pub struct SchemeLoaderFactory;

/// The kind of loader selected by `SchemeLoaderFactory`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum LoaderKind {
    Fetch,
    Xhr,
    Unknown(String),
}

impl SchemeLoaderFactory {
    pub fn new() -> Self {
        Self
    }

    /// Determine the appropriate loader kind for a URL.
    pub fn get_loader_kind(&self, url: &str) -> LoaderKind {
        if let Some(scheme_end) = url.find("://") {
            let scheme = &url[..scheme_end];
            match scheme {
                "http" | "https" => LoaderKind::Fetch,
                "data" | "blob" => LoaderKind::Xhr,
                other => LoaderKind::Unknown(other.to_string()),
            }
        } else {
            // Relative URL — default to Fetch
            LoaderKind::Fetch
        }
    }

    /// Returns true when the URL uses http or https.
    pub fn is_http_url(&self, url: &str) -> bool {
        matches!(self.get_loader_kind(url), LoaderKind::Fetch)
    }
}

// ---------------------------------------------------------------------------
// Mock loader (kept from original)
// ---------------------------------------------------------------------------

/// Mock HTTP loader for testing.
#[derive(Clone, Debug, Default)]
pub struct MockHttpLoader;

impl HttpLoader for MockHttpLoader {
    fn load(&self, _url: &str, _range: Option<&str>) -> Result<Vec<u8>, String> {
        Ok(Vec::new())
    }
    fn abort(&self) {}
}

#[cfg(test)]
mod tests {
    use super::*;

    // --- original tests (unchanged) ---

    #[test]
    fn mock_http_loader_load_returns_empty() {
        let loader = MockHttpLoader;
        let result = loader.load("http://example.com/seg.mp4", None);
        assert!(result.is_ok());
        assert!(result.unwrap().is_empty());
    }

    #[test]
    fn mock_http_loader_load_with_range() {
        let loader = MockHttpLoader;
        let result = loader.load("http://example.com/seg.mp4", Some("bytes=0-1023"));
        assert!(result.is_ok());
    }

    #[test]
    fn mock_http_loader_abort_no_panic() {
        let loader = MockHttpLoader;
        loader.abort(); // should not panic
    }

    #[test]
    fn http_loader_trait_object_safety() {
        let loader: Box<dyn HttpLoader> = Box::new(MockHttpLoader);
        let result = loader.load("http://example.com", None);
        assert!(result.is_ok());
        loader.abort();
    }

    #[test]
    fn mock_http_loader_is_send() {
        fn assert_send<T: Send>() {}
        assert_send::<MockHttpLoader>();
    }

    // --- LoadError tests ---

    #[test]
    fn load_error_display_timeout() {
        assert_eq!(LoadError::Timeout.to_string(), "request timed out");
    }

    #[test]
    fn load_error_display_network() {
        let e = LoadError::NetworkError("dns failed".into());
        assert!(e.to_string().contains("dns failed"));
    }

    #[test]
    fn load_error_display_abort() {
        assert_eq!(LoadError::AbortError.to_string(), "request aborted");
    }

    #[test]
    fn load_error_display_http() {
        assert_eq!(LoadError::HttpError(404).to_string(), "HTTP error 404");
    }

    #[test]
    fn load_error_equality() {
        assert_eq!(LoadError::Timeout, LoadError::Timeout);
        assert_ne!(LoadError::Timeout, LoadError::AbortError);
        assert_eq!(LoadError::HttpError(500), LoadError::HttpError(500));
        assert_ne!(LoadError::HttpError(500), LoadError::HttpError(404));
    }

    // --- RetryConfig tests ---

    #[test]
    fn retry_config_defaults() {
        let rc = RetryConfig::default();
        assert_eq!(rc.max_retries, 3);
        assert_eq!(rc.retry_delay_ms, 1000);
        assert_eq!(rc.retry_backoff_factor, 2.0);
    }

    #[test]
    fn retry_config_delay_for_attempt() {
        let rc = RetryConfig {
            max_retries: 3,
            retry_delay_ms: 1000,
            retry_backoff_factor: 2.0,
        };
        assert_eq!(rc.delay_for_attempt(0), 1000);
        assert_eq!(rc.delay_for_attempt(1), 2000);
        assert_eq!(rc.delay_for_attempt(2), 4000);
    }

    #[test]
    fn retry_config_delay_no_backoff() {
        let rc = RetryConfig {
            max_retries: 5,
            retry_delay_ms: 500,
            retry_backoff_factor: 1.0,
        };
        assert_eq!(rc.delay_for_attempt(0), 500);
        assert_eq!(rc.delay_for_attempt(3), 500);
    }

    // --- LoaderConfig tests ---

    #[test]
    fn loader_config_defaults() {
        let lc = LoaderConfig::default();
        assert_eq!(lc.timeout_ms, 15000);
        assert_eq!(lc.retry_config.max_retries, 3);
    }

    // --- FetchLoader tests ---

    #[test]
    fn fetch_loader_returns_network_error() {
        let fl = FetchLoader::new(LoaderConfig::default());
        let r = fl.load("http://example.com", None);
        assert!(r.is_err());
        assert!(matches!(r.unwrap_err(), LoadError::NetworkError(_)));
    }

    #[test]
    fn fetch_loader_abort_no_panic() {
        let fl = FetchLoader::default();
        fl.abort();
    }

    // --- XhrLoader tests ---

    #[test]
    fn xhr_loader_returns_network_error() {
        let xl = XhrLoader::new(LoaderConfig::default());
        let r = xl.load("http://example.com", None);
        assert!(r.is_err());
        assert!(matches!(r.unwrap_err(), LoadError::NetworkError(_)));
    }

    #[test]
    fn xhr_loader_abort_no_panic() {
        let xl = XhrLoader::default();
        xl.abort();
    }

    // --- SchemeLoaderFactory tests ---

    #[test]
    fn scheme_factory_http() {
        let f = SchemeLoaderFactory::new();
        assert_eq!(f.get_loader_kind("http://example.com"), LoaderKind::Fetch);
    }

    #[test]
    fn scheme_factory_https() {
        let f = SchemeLoaderFactory::new();
        assert_eq!(f.get_loader_kind("https://example.com"), LoaderKind::Fetch);
    }

    #[test]
    fn scheme_factory_data() {
        let f = SchemeLoaderFactory::new();
        assert_eq!(f.get_loader_kind("data://abc"), LoaderKind::Xhr);
    }

    #[test]
    fn scheme_factory_blob() {
        let f = SchemeLoaderFactory::new();
        assert_eq!(f.get_loader_kind("blob://abc"), LoaderKind::Xhr);
    }

    #[test]
    fn scheme_factory_unknown() {
        let f = SchemeLoaderFactory::new();
        assert_eq!(
            f.get_loader_kind("ftp://files.example.com"),
            LoaderKind::Unknown("ftp".into())
        );
    }

    #[test]
    fn scheme_factory_relative_url() {
        let f = SchemeLoaderFactory::new();
        assert_eq!(f.get_loader_kind("segments/seg1.m4s"), LoaderKind::Fetch);
    }

    #[test]
    fn scheme_factory_is_http_url() {
        let f = SchemeLoaderFactory::new();
        assert!(f.is_http_url("https://example.com/manifest.mpd"));
        assert!(!f.is_http_url("ftp://example.com/file"));
    }
}
