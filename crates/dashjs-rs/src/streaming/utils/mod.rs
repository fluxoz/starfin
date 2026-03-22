//! Port of `dash.js/src/streaming/utils/`.

use serde::{Deserialize, Serialize};

/// Custom time ranges (port of CustomTimeRanges.js).
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct CustomTimeRanges {
    ranges: Vec<(f64, f64)>,
}

impl CustomTimeRanges {
    pub fn new() -> Self { Self::default() }
    pub fn add(&mut self, start: f64, end: f64) { self.ranges.push((start, end)); }
    pub fn length(&self) -> usize { self.ranges.len() }
    pub fn start(&self, index: usize) -> Option<f64> { self.ranges.get(index).map(|r| r.0) }
    pub fn end(&self, index: usize) -> Option<f64> { self.ranges.get(index).map(|r| r.1) }
    pub fn clear(&mut self) { self.ranges.clear(); }
}

/// Capability detection stub.
#[derive(Clone, Debug, Default)]
pub struct Capabilities;
impl Capabilities {
    pub fn supports_media_source() -> bool { true }
    pub fn supports_codec(codec: &str) -> bool { !codec.is_empty() }
    pub fn supports_encrypted_media() -> bool { true }
}

/// Init cache for initialization segments.
#[derive(Clone, Debug, Default)]
pub struct InitCache {
    cache: std::collections::HashMap<String, Vec<u8>>,
}
impl InitCache {
    pub fn new() -> Self { Self::default() }
    pub fn save(&mut self, key: String, data: Vec<u8>) { self.cache.insert(key, data); }
    pub fn get(&self, key: &str) -> Option<&Vec<u8>> { self.cache.get(key) }
    pub fn reset(&mut self) { self.cache.clear(); }
}

#[cfg(test)]
mod tests {
    use super::*;

    // CustomTimeRanges tests
    #[test]
    fn time_ranges_add_and_length() {
        let mut tr = CustomTimeRanges::new();
        assert_eq!(tr.length(), 0);
        tr.add(0.0, 5.0);
        tr.add(10.0, 15.0);
        assert_eq!(tr.length(), 2);
    }

    #[test]
    fn time_ranges_start_end() {
        let mut tr = CustomTimeRanges::new();
        tr.add(1.0, 3.0);
        assert_eq!(tr.start(0), Some(1.0));
        assert_eq!(tr.end(0), Some(3.0));
    }

    #[test]
    fn time_ranges_out_of_bounds() {
        let tr = CustomTimeRanges::new();
        assert_eq!(tr.start(0), None);
        assert_eq!(tr.end(0), None);
    }

    #[test]
    fn time_ranges_clear() {
        let mut tr = CustomTimeRanges::new();
        tr.add(0.0, 1.0);
        tr.clear();
        assert_eq!(tr.length(), 0);
    }

    // Capabilities tests
    #[test]
    fn capabilities_supports_media_source() {
        assert!(Capabilities::supports_media_source());
    }

    #[test]
    fn capabilities_supports_codec() {
        assert!(Capabilities::supports_codec("avc1.42E01E"));
        assert!(!Capabilities::supports_codec(""));
    }

    #[test]
    fn capabilities_supports_encrypted_media() {
        assert!(Capabilities::supports_encrypted_media());
    }

    // InitCache tests
    #[test]
    fn init_cache_save_and_get() {
        let mut cache = InitCache::new();
        cache.save("video_init".into(), vec![0, 1, 2, 3]);
        let data = cache.get("video_init");
        assert!(data.is_some());
        assert_eq!(data.unwrap(), &vec![0, 1, 2, 3]);
    }

    #[test]
    fn init_cache_missing_key() {
        let cache = InitCache::new();
        assert!(cache.get("nonexistent").is_none());
    }

    #[test]
    fn init_cache_overwrite() {
        let mut cache = InitCache::new();
        cache.save("key".into(), vec![1]);
        cache.save("key".into(), vec![2]);
        assert_eq!(cache.get("key").unwrap(), &vec![2]);
    }

    #[test]
    fn init_cache_reset() {
        let mut cache = InitCache::new();
        cache.save("a".into(), vec![1]);
        cache.save("b".into(), vec![2]);
        cache.reset();
        assert!(cache.get("a").is_none());
        assert!(cache.get("b").is_none());
    }
}
