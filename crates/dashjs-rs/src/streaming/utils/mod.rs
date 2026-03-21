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
