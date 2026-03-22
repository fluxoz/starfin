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

#[cfg(test)]
mod tests {
    use super::*;

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
}
