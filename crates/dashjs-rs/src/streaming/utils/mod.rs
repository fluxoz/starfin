//! Port of `dash.js/src/streaming/utils/`.

use serde::{Deserialize, Serialize};
use std::fmt;

// ---------------------------------------------------------------------------
// CustomTimeRanges
// ---------------------------------------------------------------------------

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

    /// Merge all overlapping/adjacent ranges in-place.
    pub fn merge_overlapping(&mut self) {
        if self.ranges.len() <= 1 {
            return;
        }
        self.ranges.sort_by(|a, b| a.0.partial_cmp(&b.0).unwrap_or(std::cmp::Ordering::Equal));
        let mut merged: Vec<(f64, f64)> = vec![self.ranges[0]];
        for &(s, e) in &self.ranges[1..] {
            let last = merged.last_mut().unwrap();
            if s <= last.1 {
                last.1 = last.1.max(e);
            } else {
                merged.push((s, e));
            }
        }
        self.ranges = merged;
    }

    /// Returns `true` if `time` falls within any range.
    pub fn contains(&self, time: f64) -> bool {
        self.ranges.iter().any(|&(s, e)| s <= time && time < e)
    }

    /// Sum of all range durations.
    pub fn total_duration(&self) -> f64 {
        self.ranges.iter().map(|&(s, e)| e - s).sum()
    }

    /// Find the first gap that starts after `after_time`. Returns `(gap_start, gap_end)`.
    pub fn find_gap(&self, after_time: f64) -> Option<(f64, f64)> {
        let mut sorted = self.ranges.clone();
        sorted.sort_by(|a, b| a.0.partial_cmp(&b.0).unwrap_or(std::cmp::Ordering::Equal));
        for pair in sorted.windows(2) {
            let gap_start = pair[0].1;
            let gap_end = pair[1].0;
            if gap_start > after_time && gap_start < gap_end {
                return Some((gap_start, gap_end));
            }
        }
        None
    }

    /// Remove all ranges that overlap with `[start, end)`.
    pub fn remove(&mut self, start: f64, end: f64) {
        self.ranges.retain(|&(s, e)| e <= start || s >= end);
    }
}

// ---------------------------------------------------------------------------
// UrlUtils
// ---------------------------------------------------------------------------

/// URL manipulation utilities (port of URLUtils.js).
#[derive(Clone, Debug, Default)]
pub struct UrlUtils;

impl UrlUtils {
    /// Resolve a relative URL against a base URL.
    pub fn resolve(base: &str, relative: &str) -> String {
        if !Self::is_relative(relative) {
            return relative.to_string();
        }
        if let Some(pos) = base.rfind('/') {
            format!("{}/{}", &base[..pos], relative)
        } else {
            relative.to_string()
        }
    }

    /// Returns `true` if the URL is relative (no scheme).
    pub fn is_relative(url: &str) -> bool {
        !url.contains("://")
    }

    /// Extract the base URL (everything up to and including the last `/`).
    pub fn get_base_url(url: &str) -> String {
        if let Some(pos) = url.rfind('/') {
            url[..=pos].to_string()
        } else {
            String::new()
        }
    }

    /// Returns `true` if the URL uses http or https.
    pub fn is_http_or_https(url: &str) -> bool {
        url.starts_with("http://") || url.starts_with("https://")
    }
}

// ---------------------------------------------------------------------------
// BoxParser stub
// ---------------------------------------------------------------------------

/// Stub for ISO Base Media File Format (ISOBMFF / MP4) box parser.
///
/// A full implementation would parse ftyp, moov, moof, mdat, sidx, etc.
#[derive(Clone, Debug, Default)]
pub struct BoxParser;

/// Parsed box header.
#[derive(Clone, Debug)]
pub struct BoxHeader {
    pub box_type: [u8; 4],
    pub size: u64,
    pub header_size: u8,
}

impl BoxParser {
    pub fn new() -> Self { Self }

    /// Parse the header of the first box in `data`.
    pub fn parse_header(data: &[u8]) -> Option<BoxHeader> {
        if data.len() < 8 {
            return None;
        }
        let size = u32::from_be_bytes([data[0], data[1], data[2], data[3]]) as u64;
        let box_type = [data[4], data[5], data[6], data[7]];
        if size == 1 && data.len() >= 16 {
            let extended = u64::from_be_bytes([
                data[8], data[9], data[10], data[11],
                data[12], data[13], data[14], data[15],
            ]);
            Some(BoxHeader { box_type, size: extended, header_size: 16 })
        } else {
            Some(BoxHeader { box_type, size, header_size: 8 })
        }
    }

    /// Convenience: return the 4CC string for the first box.
    pub fn get_box_type(data: &[u8]) -> Option<String> {
        Self::parse_header(data).map(|h| String::from_utf8_lossy(&h.box_type).to_string())
    }
}

// ---------------------------------------------------------------------------
// DomStorage stub
// ---------------------------------------------------------------------------

/// Stub for browser localStorage / sessionStorage access.
///
/// In a wasm environment this would use `web_sys::Storage`.
#[derive(Clone, Debug, Default)]
pub struct DomStorage {
    store: std::collections::HashMap<String, String>,
}

impl DomStorage {
    pub fn new() -> Self { Self::default() }
    pub fn get_item(&self, key: &str) -> Option<&String> { self.store.get(key) }
    pub fn set_item(&mut self, key: &str, value: &str) { self.store.insert(key.to_string(), value.to_string()); }
    pub fn remove_item(&mut self, key: &str) { self.store.remove(key); }
    pub fn clear(&mut self) { self.store.clear(); }
    pub fn length(&self) -> usize { self.store.len() }
}

// ---------------------------------------------------------------------------
// ErrorHandler
// ---------------------------------------------------------------------------

/// Centralized error handler (port of ErrorHandler.js).
#[derive(Clone, Debug, Default)]
pub struct ErrorHandler {
    errors: Vec<HandledError>,
}

/// A recorded error entry.
#[derive(Clone, Debug)]
pub struct HandledError {
    pub code: u32,
    pub message: String,
    pub fatal: bool,
}

impl fmt::Display for HandledError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "[{}] {}{}", self.code, self.message, if self.fatal { " (fatal)" } else { "" })
    }
}

impl ErrorHandler {
    pub fn new() -> Self { Self::default() }

    pub fn error(&mut self, code: u32, message: &str) {
        self.errors.push(HandledError { code, message: message.to_string(), fatal: true });
    }

    pub fn warning(&mut self, code: u32, message: &str) {
        self.errors.push(HandledError { code, message: message.to_string(), fatal: false });
    }

    pub fn get_errors(&self) -> &[HandledError] { &self.errors }
    pub fn has_fatal(&self) -> bool { self.errors.iter().any(|e| e.fatal) }
    pub fn reset(&mut self) { self.errors.clear(); }
}

// ---------------------------------------------------------------------------
// SupervisorTools
// ---------------------------------------------------------------------------

/// Parameter validation utilities (port of SupervisorTools.js).
#[derive(Clone, Debug, Default)]
pub struct SupervisorTools;

impl SupervisorTools {
    /// Returns `true` when `value` matches the expected type name.
    /// Supported type names: `"string"`, `"number"`, `"boolean"`.
    pub fn check_parameter_type(value: ParameterValue, expected: &str) -> bool {
        match (value, expected) {
            (ParameterValue::Str(_), "string") => true,
            (ParameterValue::Number(_), "number") => true,
            (ParameterValue::Bool(_), "boolean") => true,
            _ => false,
        }
    }
}

/// A loosely-typed parameter value for validation.
#[derive(Clone, Debug)]
pub enum ParameterValue {
    Str(String),
    Number(f64),
    Bool(bool),
}

// ---------------------------------------------------------------------------
// Capability detection (kept from original)
// ---------------------------------------------------------------------------

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

    // --- original CustomTimeRanges tests ---
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

    // --- new CustomTimeRanges tests ---
    #[test]
    fn time_ranges_merge_overlapping() {
        let mut tr = CustomTimeRanges::new();
        tr.add(0.0, 5.0);
        tr.add(3.0, 8.0);
        tr.add(10.0, 12.0);
        tr.merge_overlapping();
        assert_eq!(tr.length(), 2);
        assert_eq!(tr.start(0), Some(0.0));
        assert_eq!(tr.end(0), Some(8.0));
        assert_eq!(tr.start(1), Some(10.0));
    }

    #[test]
    fn time_ranges_merge_no_overlap() {
        let mut tr = CustomTimeRanges::new();
        tr.add(0.0, 1.0);
        tr.add(5.0, 6.0);
        tr.merge_overlapping();
        assert_eq!(tr.length(), 2);
    }

    #[test]
    fn time_ranges_contains() {
        let mut tr = CustomTimeRanges::new();
        tr.add(1.0, 5.0);
        assert!(tr.contains(1.0));
        assert!(tr.contains(3.0));
        assert!(!tr.contains(5.0)); // exclusive end
        assert!(!tr.contains(0.5));
    }

    #[test]
    fn time_ranges_total_duration() {
        let mut tr = CustomTimeRanges::new();
        tr.add(0.0, 5.0);
        tr.add(10.0, 15.0);
        assert_eq!(tr.total_duration(), 10.0);
    }

    #[test]
    fn time_ranges_find_gap() {
        let mut tr = CustomTimeRanges::new();
        tr.add(0.0, 5.0);
        tr.add(8.0, 12.0);
        let gap = tr.find_gap(0.0);
        assert_eq!(gap, Some((5.0, 8.0)));
    }

    #[test]
    fn time_ranges_find_gap_none() {
        let mut tr = CustomTimeRanges::new();
        tr.add(0.0, 10.0);
        assert!(tr.find_gap(0.0).is_none());
    }

    #[test]
    fn time_ranges_remove() {
        let mut tr = CustomTimeRanges::new();
        tr.add(0.0, 5.0);
        tr.add(3.0, 8.0);
        tr.add(10.0, 15.0);
        tr.remove(2.0, 9.0);
        // only (10,15) survives
        assert_eq!(tr.length(), 1);
        assert_eq!(tr.start(0), Some(10.0));
    }

    // --- original Capabilities tests ---
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

    // --- original InitCache tests ---
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

    // --- UrlUtils tests ---
    #[test]
    fn url_utils_resolve_absolute() {
        assert_eq!(UrlUtils::resolve("http://a.com/b/", "http://c.com/d"), "http://c.com/d");
    }

    #[test]
    fn url_utils_resolve_relative() {
        assert_eq!(UrlUtils::resolve("http://a.com/b/c.mpd", "seg/1.m4s"), "http://a.com/b/seg/1.m4s");
    }

    #[test]
    fn url_utils_is_relative() {
        assert!(UrlUtils::is_relative("seg/1.m4s"));
        assert!(!UrlUtils::is_relative("http://a.com/seg"));
    }

    #[test]
    fn url_utils_get_base_url() {
        assert_eq!(UrlUtils::get_base_url("http://a.com/b/c.mpd"), "http://a.com/b/");
    }

    #[test]
    fn url_utils_is_http_or_https() {
        assert!(UrlUtils::is_http_or_https("http://a.com"));
        assert!(UrlUtils::is_http_or_https("https://a.com"));
        assert!(!UrlUtils::is_http_or_https("ftp://a.com"));
    }

    // --- BoxParser tests ---
    #[test]
    fn box_parser_header() {
        // ftyp box, size=20
        let data = [0, 0, 0, 20, b'f', b't', b'y', b'p', 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0];
        let hdr = BoxParser::parse_header(&data).unwrap();
        assert_eq!(&hdr.box_type, b"ftyp");
        assert_eq!(hdr.size, 20);
        assert_eq!(hdr.header_size, 8);
    }

    #[test]
    fn box_parser_too_short() {
        assert!(BoxParser::parse_header(&[0, 0, 0]).is_none());
    }

    #[test]
    fn box_parser_get_box_type() {
        let data = [0, 0, 0, 8, b'm', b'o', b'o', b'v'];
        assert_eq!(BoxParser::get_box_type(&data), Some("moov".into()));
    }

    // --- DomStorage tests ---
    #[test]
    fn dom_storage_set_get() {
        let mut ds = DomStorage::new();
        ds.set_item("key", "val");
        assert_eq!(ds.get_item("key"), Some(&"val".to_string()));
    }

    #[test]
    fn dom_storage_remove() {
        let mut ds = DomStorage::new();
        ds.set_item("k", "v");
        ds.remove_item("k");
        assert!(ds.get_item("k").is_none());
    }

    #[test]
    fn dom_storage_clear_and_length() {
        let mut ds = DomStorage::new();
        ds.set_item("a", "1");
        ds.set_item("b", "2");
        assert_eq!(ds.length(), 2);
        ds.clear();
        assert_eq!(ds.length(), 0);
    }

    // --- ErrorHandler tests ---
    #[test]
    fn error_handler_error_and_warning() {
        let mut eh = ErrorHandler::new();
        eh.error(10, "bad manifest");
        eh.warning(20, "low buffer");
        assert_eq!(eh.get_errors().len(), 2);
        assert!(eh.has_fatal());
    }

    #[test]
    fn error_handler_no_fatal() {
        let mut eh = ErrorHandler::new();
        eh.warning(1, "minor");
        assert!(!eh.has_fatal());
    }

    #[test]
    fn error_handler_reset() {
        let mut eh = ErrorHandler::new();
        eh.error(1, "e");
        eh.reset();
        assert!(eh.get_errors().is_empty());
    }

    #[test]
    fn handled_error_display() {
        let e = HandledError { code: 10, message: "test".into(), fatal: true };
        assert!(e.to_string().contains("(fatal)"));
    }

    // --- SupervisorTools tests ---
    #[test]
    fn supervisor_check_string() {
        assert!(SupervisorTools::check_parameter_type(ParameterValue::Str("hi".into()), "string"));
        assert!(!SupervisorTools::check_parameter_type(ParameterValue::Str("hi".into()), "number"));
    }

    #[test]
    fn supervisor_check_number() {
        assert!(SupervisorTools::check_parameter_type(ParameterValue::Number(1.0), "number"));
        assert!(!SupervisorTools::check_parameter_type(ParameterValue::Number(1.0), "boolean"));
    }

    #[test]
    fn supervisor_check_bool() {
        assert!(SupervisorTools::check_parameter_type(ParameterValue::Bool(true), "boolean"));
        assert!(!SupervisorTools::check_parameter_type(ParameterValue::Bool(true), "string"));
    }
}
