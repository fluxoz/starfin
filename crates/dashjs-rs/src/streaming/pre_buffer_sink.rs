//! Port of `dash.js/src/streaming/PreBufferSink.js`.
//!
//! A temporary sink that holds media chunks before a real `SourceBuffer` is
//! available. Call `discharge()` to retrieve all buffered chunks in the order
//! they should be appended.

/// A single media or initialisation chunk.
#[derive(Clone, Debug)]
pub struct MediaChunk {
    pub start: f64,
    pub end: f64,
    pub segment_type: SegmentType,
    pub representation_id: String,
    pub data: Vec<u8>,
}

#[derive(Clone, Debug, PartialEq)]
pub enum SegmentType {
    InitializationSegment,
    MediaSegment,
}

/// A buffered time range.
#[derive(Clone, Debug)]
pub struct TimeRange {
    pub start: f64,
    pub end: f64,
}

/// Holds media chunks in memory until they can be pushed to a real source
/// buffer. Mirrors the `FragmentSink` interface from dash.js.
#[derive(Clone, Debug, Default)]
pub struct PreBufferSink {
    chunks: Vec<MediaChunk>,
    init_segments: Vec<MediaChunk>,
    outstanding_init: Option<MediaChunk>,
}

impl PreBufferSink {
    pub fn new() -> Self {
        Self::default()
    }

    /// Appends a chunk. Init segments are stored separately; media segments are
    /// inserted in presentation-time order.
    pub fn append(&mut self, chunk: MediaChunk) {
        if chunk.segment_type == SegmentType::InitializationSegment {
            if !self.init_segments.iter().any(|c| c.representation_id == chunk.representation_id) {
                self.init_segments.push(chunk.clone());
            }
            self.outstanding_init = Some(chunk);
        } else {
            self.chunks.push(chunk);
            self.chunks.sort_by(|a, b| a.start.partial_cmp(&b.start).unwrap_or(std::cmp::Ordering::Equal));
            self.outstanding_init = None;
        }
    }

    /// Removes chunks whose time ranges overlap `[start, end)`.
    pub fn remove(&mut self, start: f64, end: f64) {
        self.chunks.retain(|c| {
            let end_ok = end.is_nan() || c.start < end;
            let start_ok = start.is_nan() || c.end > start;
            !(end_ok && start_ok)
        });
    }

    /// Returns the buffered time ranges as a `Vec<TimeRange>`.
    pub fn get_all_buffer_ranges(&self) -> Vec<TimeRange> {
        let mut ranges: Vec<TimeRange> = Vec::new();
        for chunk in &self.chunks {
            if let Some(last) = ranges.last_mut() {
                if chunk.start <= last.end {
                    if chunk.end > last.end {
                        last.end = chunk.end;
                    }
                    continue;
                }
            }
            ranges.push(TimeRange { start: chunk.start, end: chunk.end });
        }
        ranges
    }

    /// Returns all buffered chunks (with their paired init segments interleaved)
    /// and clears the internal buffers.
    pub fn discharge(&mut self) -> Vec<MediaChunk> {
        let mut result: Vec<MediaChunk> = Vec::new();
        let media_chunks = std::mem::take(&mut self.chunks);
        let init_segments = std::mem::take(&mut self.init_segments);

        let mut last_repr: Option<String> = None;
        for chunk in media_chunks {
            let need_init = last_repr.as_deref() != Some(chunk.representation_id.as_str());
            if need_init {
                if let Some(init) = init_segments.iter().find(|i| i.representation_id == chunk.representation_id) {
                    result.push(init.clone());
                }
            }
            last_repr = Some(chunk.representation_id.clone());
            result.push(chunk);
        }

        if let Some(init) = self.outstanding_init.take() {
            result.push(init);
        }

        result
    }

    pub fn abort(&mut self) {}

    pub fn reset(&mut self) {
        self.chunks.clear();
        self.init_segments.clear();
        self.outstanding_init = None;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn make_media(repr: &str, start: f64, end: f64) -> MediaChunk {
        MediaChunk { start, end, segment_type: SegmentType::MediaSegment, representation_id: repr.to_string(), data: vec![] }
    }
    fn make_init(repr: &str) -> MediaChunk {
        MediaChunk { start: 0.0, end: 0.0, segment_type: SegmentType::InitializationSegment, representation_id: repr.to_string(), data: vec![] }
    }

    #[test]
    fn discharge_prepends_init() {
        let mut sink = PreBufferSink::new();
        sink.append(make_init("v1"));
        sink.append(make_media("v1", 0.0, 2.0));
        sink.append(make_media("v1", 2.0, 4.0));
        let out = sink.discharge();
        assert_eq!(out.len(), 3);
        assert_eq!(out[0].segment_type, SegmentType::InitializationSegment);
    }

    #[test]
    fn remove_filters_overlapping() {
        let mut sink = PreBufferSink::new();
        sink.append(make_media("v1", 0.0, 2.0));
        sink.append(make_media("v1", 2.0, 4.0));
        sink.remove(0.0, 2.0);
        let ranges = sink.get_all_buffer_ranges();
        assert_eq!(ranges.len(), 1);
        assert_eq!(ranges[0].start, 2.0);
    }

    #[test]
    fn buffer_ranges_contiguous() {
        let mut sink = PreBufferSink::new();
        sink.append(make_media("v1", 0.0, 2.0));
        sink.append(make_media("v1", 2.0, 4.0));
        let ranges = sink.get_all_buffer_ranges();
        assert_eq!(ranges.len(), 1);
        assert_eq!(ranges[0].end, 4.0);
    }
}
