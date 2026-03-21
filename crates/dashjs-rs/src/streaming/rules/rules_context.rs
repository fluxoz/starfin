//! Port of `dash.js/src/streaming/rules/RulesContext.js`.
//!
//! Context passed to ABR rules for making quality decisions.

use super::switch_request::RepresentationInfo;

/// Media type classification.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub enum MediaType {
    Video,
    Audio,
    Text,
    Image,
}

impl MediaType {
    pub fn as_str(&self) -> &str {
        match self {
            MediaType::Video => "video",
            MediaType::Audio => "audio",
            MediaType::Text => "text",
            MediaType::Image => "image",
        }
    }
}

impl std::fmt::Display for MediaType {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.as_str())
    }
}

/// Buffer state as reported by the schedule controller.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum BufferState {
    Empty,
    Loaded,
    Default,
}

/// Context supplied to every ABR rule invocation.
#[derive(Clone, Debug)]
pub struct RulesContext {
    pub media_type: MediaType,
    pub stream_id: String,
    pub current_representation: Option<RepresentationInfo>,
    pub available_representations: Vec<RepresentationInfo>,
    pub buffer_level: f64,
    pub throughput: f64,
    pub safe_throughput: f64,
    pub latency: f64,
    pub is_dynamic: bool,
    pub dropped_frames_total: u32,
    pub total_frames: u32,
    pub schedule_controller_state: BufferState,
    pub fragment_duration: f64,
    pub playback_rate: f64,
    pub low_latency_enabled: bool,
}

impl Default for RulesContext {
    fn default() -> Self {
        Self {
            media_type: MediaType::Video,
            stream_id: String::new(),
            current_representation: None,
            available_representations: Vec::new(),
            buffer_level: 0.0,
            throughput: 0.0,
            safe_throughput: 0.0,
            latency: 0.0,
            is_dynamic: false,
            dropped_frames_total: 0,
            total_frames: 0,
            schedule_controller_state: BufferState::Default,
            fragment_duration: 4.0,
            playback_rate: 1.0,
            low_latency_enabled: false,
        }
    }
}

impl RulesContext {
    /// Find the representation for the given bitrate (highest bandwidth ≤ bitrate).
    pub fn get_optimal_representation_for_bitrate(&self, bitrate_kbps: f64) -> Option<&RepresentationInfo> {
        let mut best: Option<&RepresentationInfo> = None;
        for rep in &self.available_representations {
            if rep.bitrate_in_kbit <= bitrate_kbps {
                match best {
                    Some(b) if b.bitrate_in_kbit >= rep.bitrate_in_kbit => {}
                    _ => best = Some(rep),
                }
            }
        }
        // If no representation fits, return the lowest
        if best.is_none() {
            best = self.available_representations.first();
        }
        best
    }

    /// Get lowest quality representation.
    pub fn get_lowest_representation(&self) -> Option<&RepresentationInfo> {
        self.available_representations.first()
    }

    /// Check if we are playing at the lowest quality.
    pub fn is_playing_at_lowest_quality(&self) -> bool {
        match (&self.current_representation, self.available_representations.first()) {
            (Some(current), Some(lowest)) => current.quality_index == lowest.quality_index,
            _ => true,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::streaming::rules::switch_request::RepresentationInfo;

    fn make_reps() -> Vec<RepresentationInfo> {
        vec![
            RepresentationInfo { quality_index: 0, bandwidth: 500_000, bitrate_in_kbit: 500.0, media_type: "video".into(), id: Some("0".into()), absolute_index: 0 },
            RepresentationInfo { quality_index: 1, bandwidth: 1_000_000, bitrate_in_kbit: 1000.0, media_type: "video".into(), id: Some("1".into()), absolute_index: 1 },
            RepresentationInfo { quality_index: 2, bandwidth: 2_000_000, bitrate_in_kbit: 2000.0, media_type: "video".into(), id: Some("2".into()), absolute_index: 2 },
            RepresentationInfo { quality_index: 3, bandwidth: 4_000_000, bitrate_in_kbit: 4000.0, media_type: "video".into(), id: Some("3".into()), absolute_index: 3 },
        ]
    }

    #[test]
    fn optimal_representation_for_bitrate() {
        let ctx = RulesContext {
            available_representations: make_reps(),
            ..Default::default()
        };
        let rep = ctx.get_optimal_representation_for_bitrate(1500.0).unwrap();
        assert_eq!(rep.quality_index, 1);

        let rep = ctx.get_optimal_representation_for_bitrate(5000.0).unwrap();
        assert_eq!(rep.quality_index, 3);

        let rep = ctx.get_optimal_representation_for_bitrate(100.0).unwrap();
        assert_eq!(rep.quality_index, 0);
    }

    #[test]
    fn media_type_display() {
        assert_eq!(MediaType::Video.as_str(), "video");
        assert_eq!(MediaType::Audio.as_str(), "audio");
    }
}
