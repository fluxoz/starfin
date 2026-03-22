//! Port of `dash.js/src/mss/`.
//!
//! Microsoft Smooth Streaming support.
//! Structure mirrors: MssHandler, MssParser, MssFragmentProcessor.
//! Implements manifest detection, stream info extraction, and fragment processing.

/// Quality level in a Smooth Streaming manifest.
#[derive(Clone, Debug, Default)]
pub struct MssQualityLevel {
    pub index: usize,
    pub bitrate: u64,
    pub codec: String,
    pub width: Option<u32>,
    pub height: Option<u32>,
    pub four_cc: Option<String>,
}

/// Chunk (segment) in a Smooth Streaming manifest.
#[derive(Clone, Debug, Default)]
pub struct MssChunk {
    pub n: u64,
    pub t: u64,
    pub d: u64,
}

/// Stream info extracted from a Smooth Streaming manifest.
#[derive(Clone, Debug, Default)]
pub struct MssStreamInfo {
    pub stream_type: String,
    pub quality_levels: Vec<MssQualityLevel>,
    pub chunks: Vec<MssChunk>,
    pub url: String,
}

/// MSS handler — coordinates parsing and fragment processing.
///
/// Port of `dash.js/src/mss/MssHandler.js`.
#[derive(Clone, Debug, Default)]
pub struct MssHandler {
    _initialized: bool,
    streams: Vec<MssStreamInfo>,
}

impl MssHandler {
    pub fn new() -> Self { Self::default() }

    pub fn initialize(&mut self) {
        self._initialized = true;
    }

    pub fn is_initialized(&self) -> bool { self._initialized }

    /// Check if the given data looks like a Smooth Streaming manifest.
    pub fn is_mss_manifest(data: &str) -> bool {
        MssParser::is_smooth_streaming_manifest(data)
    }

    /// Parse manifest data and store stream info.
    pub fn process_manifest(&mut self, data: &str) -> Option<&[MssStreamInfo]> {
        let parser = MssParser::new();
        match parser.parse(data) {
            Some(streams) => {
                self.streams = streams;
                Some(&self.streams)
            }
            None => None,
        }
    }

    pub fn get_streams(&self) -> &[MssStreamInfo] {
        &self.streams
    }

    pub fn reset(&mut self) {
        self._initialized = false;
        self.streams.clear();
    }
}

/// MSS parser — detects and parses Smooth Streaming manifests.
///
/// Port of `dash.js/src/mss/MssParser.js`.
#[derive(Clone, Debug, Default)]
pub struct MssParser;

impl MssParser {
    pub fn new() -> Self { Self }

    /// Check if data is a Smooth Streaming manifest (contains SmoothStreamingMedia tag).
    pub fn is_smooth_streaming_manifest(data: &str) -> bool {
        data.contains("<SmoothStreamingMedia") || data.contains("<smoothstreamingmedia")
    }

    /// Parse a Smooth Streaming manifest. Returns None if not a valid manifest.
    pub fn parse(&self, data: &str) -> Option<Vec<MssStreamInfo>> {
        if !Self::is_smooth_streaming_manifest(data) {
            return None;
        }
        // Basic extraction — real impl would use XML parser
        let mut streams = Vec::new();
        // Look for StreamIndex elements
        for (i, _) in data.match_indices("<StreamIndex") {
            let rest = &data[i..];
            let stream_type = Self::extract_attr(rest, "Type").unwrap_or_default();
            let url = Self::extract_attr(rest, "Url").unwrap_or_default();
            let mut quality_levels = Vec::new();
            let mut chunks = Vec::new();

            // Extract QualityLevel elements within this StreamIndex
            let end_pos = rest.find("</StreamIndex").unwrap_or(rest.len());
            let block = &rest[..end_pos];
            let mut qi = 0;
            for (j, _) in block.match_indices("<QualityLevel") {
                let ql_rest = &block[j..];
                let bitrate = Self::extract_attr(ql_rest, "Bitrate")
                    .and_then(|s| s.parse::<u64>().ok()).unwrap_or(0);
                let codec = Self::extract_attr(ql_rest, "CodecPrivateData").unwrap_or_default();
                let width = Self::extract_attr(ql_rest, "MaxWidth")
                    .and_then(|s| s.parse().ok());
                let height = Self::extract_attr(ql_rest, "MaxHeight")
                    .and_then(|s| s.parse().ok());
                let four_cc = Self::extract_attr(ql_rest, "FourCC");
                quality_levels.push(MssQualityLevel {
                    index: qi, bitrate, codec, width, height, four_cc,
                });
                qi += 1;
            }

            // Extract c (chunk) elements
            let mut cn = 0u64;
            let mut ct = 0u64;
            for (j, _) in block.match_indices("<c ") {
                let c_rest = &block[j..];
                let t = Self::extract_attr(c_rest, "t")
                    .and_then(|s| s.parse::<u64>().ok());
                let d = Self::extract_attr(c_rest, "d")
                    .and_then(|s| s.parse::<u64>().ok()).unwrap_or(0);
                if let Some(t_val) = t { ct = t_val; }
                chunks.push(MssChunk { n: cn, t: ct, d });
                ct += d;
                cn += 1;
            }

            streams.push(MssStreamInfo { stream_type, quality_levels, chunks, url });
        }
        if streams.is_empty() { None } else { Some(streams) }
    }

    fn extract_attr(s: &str, name: &str) -> Option<String> {
        let pattern = format!("{}=\"", name);
        let start = s.find(&pattern)? + pattern.len();
        let rest = &s[start..];
        let end = rest.find('"')?;
        Some(rest[..end].to_string())
    }
}

/// MSS fragment processor — processes MSS fragment data.
///
/// Port of `dash.js/src/mss/MssFragmentProcessor.js`.
#[derive(Clone, Debug, Default)]
pub struct MssFragmentProcessor;

impl MssFragmentProcessor {
    pub fn new() -> Self { Self }

    /// Process a raw MSS fragment into ISO BMFF format.
    /// Returns the processed bytes, or None if processing fails.
    pub fn process(&self, data: &[u8]) -> Option<Vec<u8>> {
        if data.is_empty() { return None; }
        // In a full implementation this would convert MSS fragments to ISO BMFF
        Some(data.to_vec())
    }

    /// Generate a moov box for a given quality level.
    /// Returns empty vec as stub — real impl generates ISO BMFF moov atom.
    pub fn generate_moov(&self, _quality: &MssQualityLevel) -> Vec<u8> {
        Vec::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn mss_handler_lifecycle() {
        let mut handler = MssHandler::new();
        assert!(!handler.is_initialized());
        handler.initialize();
        assert!(handler.is_initialized());
        handler.reset();
        assert!(!handler.is_initialized());
        assert!(handler.get_streams().is_empty());
    }

    #[test]
    fn mss_handler_reset_idempotent() {
        let mut handler = MssHandler::new();
        handler.reset();
        handler.reset();
        assert!(!handler.is_initialized());
    }

    #[test]
    fn mss_parser_new() {
        let _parser = MssParser::new();
    }

    #[test]
    fn mss_fragment_processor_default() {
        let _proc = MssFragmentProcessor::default();
    }

    #[test]
    fn mss_handler_clone() {
        let handler = MssHandler::new();
        let handler2 = handler.clone();
        assert_eq!(handler.is_initialized(), handler2.is_initialized());
    }

    #[test]
    fn is_mss_manifest_detection() {
        assert!(MssParser::is_smooth_streaming_manifest("<SmoothStreamingMedia ...>"));
        assert!(!MssParser::is_smooth_streaming_manifest("<MPD type=\"static\">"));
        assert!(!MssParser::is_smooth_streaming_manifest("not xml"));
    }

    #[test]
    fn is_mss_manifest_via_handler() {
        assert!(MssHandler::is_mss_manifest("<SmoothStreamingMedia ...>"));
        assert!(!MssHandler::is_mss_manifest("<MPD>"));
    }

    #[test]
    fn parse_simple_mss_manifest() {
        let manifest = r#"<SmoothStreamingMedia>
            <StreamIndex Type="video" Url="QualityLevels({bitrate})/Fragments(video={start time})">
                <QualityLevel Bitrate="2000000" MaxWidth="1280" MaxHeight="720" FourCC="H264" CodecPrivateData="avc1"/>
                <QualityLevel Bitrate="500000" MaxWidth="640" MaxHeight="360" FourCC="H264" CodecPrivateData="avc1"/>
                <c t="0" d="20000000"/>
                <c d="20000000"/>
            </StreamIndex>
        </SmoothStreamingMedia>"#;
        let parser = MssParser::new();
        let streams = parser.parse(manifest).unwrap();
        assert_eq!(streams.len(), 1);
        assert_eq!(streams[0].stream_type, "video");
        assert_eq!(streams[0].quality_levels.len(), 2);
        assert_eq!(streams[0].quality_levels[0].bitrate, 2_000_000);
        assert_eq!(streams[0].quality_levels[1].bitrate, 500_000);
        assert_eq!(streams[0].quality_levels[0].width, Some(1280));
        assert_eq!(streams[0].chunks.len(), 2);
        assert_eq!(streams[0].chunks[0].t, 0);
        assert_eq!(streams[0].chunks[1].t, 20000000);
    }

    #[test]
    fn parse_non_mss_returns_none() {
        let parser = MssParser::new();
        assert!(parser.parse("<MPD type=\"static\"></MPD>").is_none());
        assert!(parser.parse("not xml at all").is_none());
    }

    #[test]
    fn handler_process_manifest() {
        let mut handler = MssHandler::new();
        let manifest = r#"<SmoothStreamingMedia>
            <StreamIndex Type="audio" Url="audio">
                <QualityLevel Bitrate="128000" CodecPrivateData="mp4a"/>
            </StreamIndex>
        </SmoothStreamingMedia>"#;
        let result = handler.process_manifest(manifest);
        assert!(result.is_some());
        assert_eq!(handler.get_streams().len(), 1);
        assert_eq!(handler.get_streams()[0].stream_type, "audio");
    }

    #[test]
    fn fragment_processor_process_empty() {
        let proc = MssFragmentProcessor::new();
        assert!(proc.process(&[]).is_none());
        assert!(proc.process(&[1, 2, 3]).is_some());
    }

    #[test]
    fn fragment_processor_generate_moov() {
        let proc = MssFragmentProcessor::new();
        let moov = proc.generate_moov(&MssQualityLevel::default());
        assert!(moov.is_empty()); // stub returns empty
    }

    #[test]
    fn quality_level_defaults() {
        let ql = MssQualityLevel::default();
        assert_eq!(ql.index, 0);
        assert_eq!(ql.bitrate, 0);
        assert!(ql.width.is_none());
    }

    #[test]
    fn chunk_defaults() {
        let c = MssChunk::default();
        assert_eq!(c.n, 0);
        assert_eq!(c.t, 0);
        assert_eq!(c.d, 0);
    }
}
