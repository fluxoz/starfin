//! Port of dash.js matchers: DurationMatcher, DateTimeMatcher, NumericMatcher, LangMatcher.

use chrono::{DateTime, FixedOffset};

/// Parse an ISO 8601 duration string (e.g. `PT1H30M10.5S`) into seconds.
pub fn parse_duration(iso: &str) -> Option<f64> {
    let s = iso.trim();
    if !s.starts_with('P') {
        return None;
    }
    let s = &s[1..];
    let (date_part, time_part) = if let Some(pos) = s.find('T') {
        (&s[..pos], &s[pos + 1..])
    } else {
        (s, "")
    };

    let mut total_seconds: f64 = 0.0;

    // Parse date part (Y, M, D)
    if !date_part.is_empty() {
        let mut num_start = 0;
        for (i, c) in date_part.char_indices() {
            match c {
                'Y' => {
                    let v: f64 = date_part[num_start..i].parse().ok()?;
                    total_seconds += v * 365.25 * 24.0 * 3600.0;
                    num_start = i + 1;
                }
                'M' => {
                    let v: f64 = date_part[num_start..i].parse().ok()?;
                    total_seconds += v * 30.44 * 24.0 * 3600.0;
                    num_start = i + 1;
                }
                'D' => {
                    let v: f64 = date_part[num_start..i].parse().ok()?;
                    total_seconds += v * 24.0 * 3600.0;
                    num_start = i + 1;
                }
                _ => {}
            }
        }
    }

    // Parse time part (H, M, S)
    if !time_part.is_empty() {
        let mut num_start = 0;
        for (i, c) in time_part.char_indices() {
            match c {
                'H' => {
                    let v: f64 = time_part[num_start..i].parse().ok()?;
                    total_seconds += v * 3600.0;
                    num_start = i + 1;
                }
                'M' => {
                    let v: f64 = time_part[num_start..i].parse().ok()?;
                    total_seconds += v * 60.0;
                    num_start = i + 1;
                }
                'S' => {
                    let v: f64 = time_part[num_start..i].parse().ok()?;
                    total_seconds += v;
                    num_start = i + 1;
                }
                _ => {}
            }
        }
        let _ = num_start;
    }

    Some(total_seconds)
}

/// Parse an ISO 8601 datetime string.
pub fn parse_datetime(s: &str) -> Option<DateTime<FixedOffset>> {
    let s = s.trim();
    // Try standard parse
    if let Ok(dt) = DateTime::parse_from_rfc3339(s) {
        return Some(dt);
    }
    // Try with space instead of T
    if let Ok(dt) = DateTime::parse_from_str(s, "%Y-%m-%dT%H:%M:%S%.f%:z") {
        return Some(dt);
    }
    if let Ok(dt) = DateTime::parse_from_str(s, "%Y-%m-%dT%H:%M:%S%:z") {
        return Some(dt);
    }
    // Try without timezone (assume UTC)
    if let Ok(naive) = chrono::NaiveDateTime::parse_from_str(s, "%Y-%m-%dT%H:%M:%S%.f") {
        return Some(DateTime::from_naive_utc_and_offset(naive, FixedOffset::east_opt(0)?));
    }
    if let Ok(naive) = chrono::NaiveDateTime::parse_from_str(s, "%Y-%m-%dT%H:%M:%S") {
        return Some(DateTime::from_naive_utc_and_offset(naive, FixedOffset::east_opt(0)?));
    }
    // Try with Z suffix
    let trimmed = s.trim_end_matches('Z');
    if let Ok(naive) = chrono::NaiveDateTime::parse_from_str(trimmed, "%Y-%m-%dT%H:%M:%S%.f") {
        return Some(DateTime::from_naive_utc_and_offset(naive, FixedOffset::east_opt(0)?));
    }
    if let Ok(naive) = chrono::NaiveDateTime::parse_from_str(trimmed, "%Y-%m-%dT%H:%M:%S") {
        return Some(DateTime::from_naive_utc_and_offset(naive, FixedOffset::east_opt(0)?));
    }
    None
}

/// Test if a value looks like an ISO 8601 duration (starts with 'P').
pub fn is_duration(value: &str) -> bool {
    let v = value.trim();
    v.starts_with('P') && v.len() > 1
}

/// Test if a value looks like an ISO 8601 datetime.
pub fn is_datetime(value: &str) -> bool {
    let v = value.trim();
    // Must have T separator and at least a date-like pattern
    v.len() >= 10 && v.contains('T') && v.chars().next().map_or(false, |c| c.is_ascii_digit())
}

/// Test if a value is purely numeric (integer or float).
pub fn is_numeric(value: &str) -> bool {
    let v = value.trim();
    if v.is_empty() {
        return false;
    }
    v.parse::<f64>().is_ok()
}

/// Parse a numeric string value.
pub fn parse_numeric(value: &str) -> Option<f64> {
    value.trim().parse::<f64>().ok()
}

/// Duration attributes in MPD that should be parsed as durations.
const DURATION_ATTRS: &[&str] = &[
    "mediaPresentationDuration",
    "minimumUpdatePeriod",
    "timeShiftBufferDepth",
    "maxSegmentDuration",
    "maxSubsegmentDuration",
    "suggestedPresentationDelay",
    "minBufferTime",
    "start",
    "duration",
];

/// Datetime attributes in MPD that should be parsed as datetimes.
const DATETIME_ATTRS: &[&str] = &[
    "availabilityStartTime",
    "availabilityEndTime",
    "publishTime",
    "wallClockTime",
];

/// Check if an attribute name + value should be matched as a duration.
pub fn matches_duration(attr_name: &str, value: &str) -> bool {
    DURATION_ATTRS.contains(&attr_name) && is_duration(value)
}

/// Check if an attribute name + value should be matched as a datetime.
pub fn matches_datetime(attr_name: &str, _value: &str) -> bool {
    DATETIME_ATTRS.contains(&attr_name)
}

/// Check if a value should be parsed as numeric. Excludes known non-numeric attrs.
pub fn matches_numeric(_element_name: &str, attr_name: &str, value: &str) -> bool {
    // Don't convert id, lang, or known string attributes
    let string_attrs = [
        "id", "lang", "contentType", "value", "mimeType", "codecs",
        "profiles", "sar", "frameRate", "audioSamplingRate", "schemeIdUri",
        "scanType", "media", "initialization", "sourceURL", "type",
        "dependencyId", "mediaStreamStructureId", "codecPrivateData",
        "codingDependency",
    ];
    if string_attrs.contains(&attr_name) {
        return false;
    }
    is_numeric(value)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_duration_simple() {
        assert!((parse_duration("PT10S").unwrap() - 10.0).abs() < 0.001);
        assert!((parse_duration("PT1M30S").unwrap() - 90.0).abs() < 0.001);
        assert!((parse_duration("PT1H").unwrap() - 3600.0).abs() < 0.001);
        assert!((parse_duration("PT1H30M10.5S").unwrap() - 5410.5).abs() < 0.001);
    }

    #[test]
    fn test_parse_duration_with_days() {
        assert!((parse_duration("P1D").unwrap() - 86400.0).abs() < 0.001);
        assert!((parse_duration("P1DT12H").unwrap() - 129600.0).abs() < 0.001);
    }

    #[test]
    fn test_parse_duration_fractional() {
        assert!((parse_duration("PT0.5S").unwrap() - 0.5).abs() < 0.001);
        assert!((parse_duration("PT2.016S").unwrap() - 2.016).abs() < 0.001);
    }

    #[test]
    fn test_parse_duration_invalid() {
        assert!(parse_duration("").is_none());
        assert!(parse_duration("not-a-duration").is_none());
    }

    #[test]
    fn test_parse_datetime() {
        let dt = parse_datetime("2023-01-15T10:30:00Z").unwrap();
        assert_eq!(dt.timestamp(), 1673778600);

        let dt = parse_datetime("2023-01-15T10:30:00.000Z").unwrap();
        assert_eq!(dt.timestamp(), 1673778600);
    }

    #[test]
    fn test_is_duration() {
        assert!(is_duration("PT10S"));
        assert!(is_duration("P1DT12H"));
        assert!(!is_duration("2023-01-01T00:00:00Z"));
        assert!(!is_duration("123"));
    }

    #[test]
    fn test_is_numeric() {
        assert!(is_numeric("123"));
        assert!(is_numeric("1.5"));
        assert!(is_numeric("-10"));
        assert!(!is_numeric("abc"));
        assert!(!is_numeric("PT10S"));
    }

    #[test]
    fn test_matches_duration_attrs() {
        assert!(matches_duration("mediaPresentationDuration", "PT30S"));
        assert!(matches_duration("minBufferTime", "PT2S"));
        assert!(!matches_duration("bandwidth", "PT10S"));
    }
}
