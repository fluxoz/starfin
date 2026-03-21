//! Port of `dash.js/src/streaming/net/`.
//!
//! Network loader traits and implementations.

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

/// Mock HTTP loader for testing.
#[derive(Clone, Debug, Default)]
pub struct MockHttpLoader;

impl HttpLoader for MockHttpLoader {
    fn load(&self, _url: &str, _range: Option<&str>) -> Result<Vec<u8>, String> {
        Ok(Vec::new())
    }
    fn abort(&self) {}
}
