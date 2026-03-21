//! Port of `dash.js/src/dash/controllers/SegmentBaseController.js` (stub).
//!
//! Manages segment base (sidx) loading and index resolution.

use crate::dash::vo::representation::Representation;
use crate::dash::vo::segment::Segment;

/// Manages SegmentBase (sidx) loading and caching.
pub struct SegmentBaseController {
    /// Cache of loaded segment indices, keyed by representation ID.
    segment_cache: std::collections::HashMap<String, Vec<Segment>>,
}

impl SegmentBaseController {
    pub fn new() -> Self {
        Self {
            segment_cache: std::collections::HashMap::new(),
        }
    }

    /// Check if we have loaded segments for a representation.
    pub fn has_segments_for(&self, representation: &Representation) -> bool {
        representation
            .id
            .as_ref()
            .map_or(false, |id| self.segment_cache.contains_key(id))
    }

    /// Store loaded segments for a representation.
    pub fn set_segments_for(&mut self, representation: &Representation, segments: Vec<Segment>) {
        if let Some(ref id) = representation.id {
            self.segment_cache.insert(id.clone(), segments);
        }
    }

    /// Get cached segments for a representation.
    pub fn get_segments_for(&self, representation: &Representation) -> Option<&Vec<Segment>> {
        representation
            .id
            .as_ref()
            .and_then(|id| self.segment_cache.get(id))
    }

    /// Parse an index range string (e.g., "100-999") into (start, end).
    pub fn parse_index_range(range: &str) -> Option<(u64, u64)> {
        let parts: Vec<&str> = range.split('-').collect();
        if parts.len() == 2 {
            let start = parts[0].parse::<u64>().ok()?;
            let end = parts[1].parse::<u64>().ok()?;
            Some((start, end))
        } else {
            None
        }
    }

    /// Reset all cached segment info.
    pub fn reset(&mut self) {
        self.segment_cache.clear();
    }
}

impl Default for SegmentBaseController {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_index_range() {
        let (start, end) = SegmentBaseController::parse_index_range("100-999").unwrap();
        assert_eq!(start, 100);
        assert_eq!(end, 999);
    }

    #[test]
    fn test_parse_index_range_invalid() {
        assert!(SegmentBaseController::parse_index_range("invalid").is_none());
        assert!(SegmentBaseController::parse_index_range("").is_none());
    }

    #[test]
    fn test_segment_cache() {
        let mut ctrl = SegmentBaseController::new();
        let rep = Representation {
            id: Some("rep1".to_string()),
            ..Representation::default()
        };

        assert!(!ctrl.has_segments_for(&rep));

        let segments = vec![Segment {
            index: Some(0),
            media: Some("seg-0.m4s".to_string()),
            ..Segment::default()
        }];
        ctrl.set_segments_for(&rep, segments);

        assert!(ctrl.has_segments_for(&rep));
        let cached = ctrl.get_segments_for(&rep).unwrap();
        assert_eq!(cached.len(), 1);
    }

    #[test]
    fn test_reset() {
        let mut ctrl = SegmentBaseController::new();
        let rep = Representation {
            id: Some("rep1".to_string()),
            ..Representation::default()
        };

        ctrl.set_segments_for(
            &rep,
            vec![Segment {
                index: Some(0),
                ..Segment::default()
            }],
        );
        ctrl.reset();
        assert!(!ctrl.has_segments_for(&rep));
    }
}
