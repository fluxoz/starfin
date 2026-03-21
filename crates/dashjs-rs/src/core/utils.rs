// Port of dash.js/src/core/Utils.js
//
// Common utility functions used throughout the player.

use std::collections::HashMap;

/// Append query parameters to a URL string.
///
/// Each `(key, value)` pair is percent-encoded and appended with the
/// appropriate `?` or `&` separator.
pub fn add_query_params(url: &str, params: &[(&str, &str)]) -> String {
    if params.is_empty() {
        return url.to_owned();
    }

    let mut result = url.to_owned();
    for (key, value) in params {
        let sep = if result.contains('?') { '&' } else { '?' };
        result.push(sep);
        result.push_str(&urlencoding::encode(key));
        result.push('=');
        result.push_str(&urlencoding::encode(value));
    }
    result
}

/// Parse HTTP header string (as returned by `getAllResponseHeaders()`)
/// into a `HashMap`.
pub fn parse_http_headers(header_str: &str) -> HashMap<String, String> {
    let mut headers = HashMap::new();
    for line in header_str.trim().split("\r\n") {
        if let Some(idx) = line.find(": ") {
            let key = &line[..idx];
            let value = &line[idx + 2..];
            headers.insert(key.to_owned(), value.to_owned());
        }
    }
    headers
}

/// Generate a simple Java-style hash code for a string.
///
/// Matches the dash.js `Utils.generateHashCode` implementation.
pub fn generate_hash_code(s: &str) -> i32 {
    let mut hash: i32 = 0;
    for ch in s.chars() {
        hash = hash.wrapping_mul(31).wrapping_add(ch as i32);
    }
    hash
}

/// Check if a string starts with `http://` or `https://`.
pub fn string_has_protocol(s: &str) -> bool {
    s.starts_with("http://") || s.starts_with("https://")
}

/// Extract the host portion from a URL string.
pub fn get_host_from_url(url_string: &str) -> Option<String> {
    url::Url::parse(url_string).ok().map(|u| {
        match u.port() {
            Some(port) => format!("{}:{}", u.host_str().unwrap_or(""), port),
            None => u.host_str().unwrap_or("").to_owned(),
        }
    })
}

/// Convert a byte slice to a lower-case hex string.
pub fn bytes_to_hex(data: &[u8]) -> String {
    let mut hex = String::with_capacity(data.len() * 2);
    for byte in data {
        hex.push_str(&format!("{byte:02x}"));
    }
    hex
}

/// Determine the codec family from a codec string (e.g. `"avc1.64001E"` → `"avc"`).
pub fn get_codec_family(codec_string: &str) -> &str {
    let base = codec_string.split('.').next().unwrap_or(codec_string);
    match base {
        "mp4a" => {
            let profile = codec_string
                .split('.')
                .skip(1)
                .collect::<Vec<_>>()
                .join(".");
            match profile.as_str() {
                "69" | "6b" | "40.34" => "mp3",
                "66" | "67" | "68" | "40.2" | "40.02" | "40.5" | "40.05" | "40.29"
                | "40.42" => "aac",
                "a5" => "ac3",
                "e6" => "ec3",
                "b2" => "dtsx",
                "a9" => "dtsc",
                _ => "mp4a",
            }
        }
        "avc1" | "avc3" => "avc",
        "hvc1" | "hvc3" => "hevc",
        _ => base,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn add_params_to_url() {
        let url = add_query_params("https://example.com/path", &[("key", "val"), ("a", "b")]);
        assert!(url.starts_with("https://example.com/path?"));
        assert!(url.contains("key=val"));
        assert!(url.contains("&a=b"));
    }

    #[test]
    fn add_params_empty() {
        let url = add_query_params("https://example.com", &[]);
        assert_eq!(url, "https://example.com");
    }

    #[test]
    fn parse_headers() {
        let raw = "Content-Type: text/html\r\nX-Custom: value\r\n";
        let map = parse_http_headers(raw);
        assert_eq!(map.get("Content-Type").unwrap(), "text/html");
        assert_eq!(map.get("X-Custom").unwrap(), "value");
    }

    #[test]
    fn hash_code_deterministic() {
        let a = generate_hash_code("hello");
        let b = generate_hash_code("hello");
        assert_eq!(a, b);
        assert_ne!(generate_hash_code("hello"), generate_hash_code("world"));
    }

    #[test]
    fn has_protocol() {
        assert!(string_has_protocol("https://foo.com"));
        assert!(string_has_protocol("http://foo.com"));
        assert!(!string_has_protocol("ftp://foo.com"));
        assert!(!string_has_protocol("/relative/path"));
    }

    #[test]
    fn host_extraction() {
        assert_eq!(
            get_host_from_url("https://example.com:8080/path"),
            Some("example.com:8080".into())
        );
        assert_eq!(
            get_host_from_url("https://example.com/path"),
            Some("example.com".into())
        );
    }

    #[test]
    fn hex_encoding() {
        assert_eq!(bytes_to_hex(&[0x0a, 0xff, 0x00]), "0aff00");
    }

    #[test]
    fn codec_families() {
        assert_eq!(get_codec_family("avc1.64001E"), "avc");
        assert_eq!(get_codec_family("hvc1.2.4.L93.B0"), "hevc");
        assert_eq!(get_codec_family("mp4a.40.2"), "aac");
        assert_eq!(get_codec_family("mp4a.69"), "mp3");
        assert_eq!(get_codec_family("mp4a.a5"), "ac3");
        assert_eq!(get_codec_family("vp09.00.10.08"), "vp09");
    }
}
