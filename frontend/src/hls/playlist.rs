//! HLS playlist parser
//!
//! Parses M3U8 master playlists and media playlists according to RFC 8216.

use crate::hls::error::{ErrorCode, HlsError, HlsResult};
use std::collections::HashMap;

/// Resolution of a video level
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Resolution {
    pub width: u32,
    pub height: u32,
}

impl Resolution {
    pub fn new(width: u32, height: u32) -> Self {
        Self { width, height }
    }
    
    /// Parse resolution from "WIDTHxHEIGHT" string
    pub fn parse(s: &str) -> Option<Self> {
        let parts: Vec<&str> = s.split('x').collect();
        if parts.len() == 2 {
            let width = parts[0].parse().ok()?;
            let height = parts[1].parse().ok()?;
            Some(Self { width, height })
        } else {
            None
        }
    }
    
    /// Get pixel count
    pub fn pixels(&self) -> u64 {
        self.width as u64 * self.height as u64
    }
}

/// Video quality level information
#[derive(Clone, Debug)]
pub struct Level {
    /// Level index
    pub index: usize,
    /// Bandwidth in bits per second
    pub bitrate: u64,
    /// Average bandwidth if available
    pub avg_bitrate: Option<u64>,
    /// Video resolution
    pub resolution: Option<Resolution>,
    /// Video codec string
    pub video_codec: Option<String>,
    /// Audio codec string
    pub audio_codec: Option<String>,
    /// Codecs string from manifest
    pub codecs: Option<String>,
    /// Frame rate
    pub frame_rate: Option<f64>,
    /// HDCP level requirement
    pub hdcp_level: Option<String>,
    /// Audio group ID
    pub audio_group: Option<String>,
    /// Subtitle group ID
    pub subtitle_group: Option<String>,
    /// Closed caption group ID
    pub cc_group: Option<String>,
    /// Playlist URL
    pub url: String,
    /// Name/label for the level
    pub name: String,
}

impl Level {
    /// Create a level from stream info attributes
    pub fn from_stream_inf(index: usize, attrs: &HashMap<String, String>, url: &str) -> Self {
        let bitrate = attrs
            .get("BANDWIDTH")
            .and_then(|s| s.parse().ok())
            .unwrap_or(0);
            
        let avg_bitrate = attrs.get("AVERAGE-BANDWIDTH").and_then(|s| s.parse().ok());
        
        let resolution = attrs.get("RESOLUTION").and_then(|s| Resolution::parse(s));
        
        let codecs = attrs.get("CODECS").cloned();
        let (video_codec, audio_codec) = codecs
            .as_ref()
            .map(|c| parse_codecs(c))
            .unwrap_or((None, None));
            
        let frame_rate = attrs.get("FRAME-RATE").and_then(|s| s.parse().ok());
        let hdcp_level = attrs.get("HDCP-LEVEL").cloned();
        let audio_group = attrs.get("AUDIO").cloned();
        let subtitle_group = attrs.get("SUBTITLES").cloned();
        let cc_group = attrs.get("CLOSED-CAPTIONS").cloned();
        
        // Generate name from resolution and bitrate
        let name = if let Some(ref res) = resolution {
            format!("{}p ({}kbps)", res.height, bitrate / 1000)
        } else {
            format!("{}kbps", bitrate / 1000)
        };
        
        Self {
            index,
            bitrate,
            avg_bitrate,
            resolution,
            video_codec,
            audio_codec,
            codecs,
            frame_rate,
            hdcp_level,
            audio_group,
            subtitle_group,
            cc_group,
            url: url.to_string(),
            name,
        }
    }
    
    /// Check if this level is compatible with given HDCP requirement
    pub fn is_hdcp_compatible(&self, required: Option<&str>) -> bool {
        match (required, &self.hdcp_level) {
            (None, _) => true,
            (Some(_), None) => true,
            (Some(req), Some(level)) => {
                // HDCP levels: NONE < TYPE-0 < TYPE-1
                let req_val = hdcp_value(req);
                let level_val = hdcp_value(level);
                level_val <= req_val
            }
        }
    }
    
    /// Check if level fits within resolution limits
    pub fn fits_resolution(&self, max_width: u32, max_height: u32) -> bool {
        if max_width == 0 && max_height == 0 {
            return true;
        }
        
        match &self.resolution {
            Some(res) => {
                (max_width == 0 || res.width <= max_width) &&
                (max_height == 0 || res.height <= max_height)
            }
            None => true, // Can't determine, assume it fits
        }
    }
}

fn hdcp_value(level: &str) -> u32 {
    match level.to_uppercase().as_str() {
        "NONE" => 0,
        "TYPE-0" => 1,
        "TYPE-1" => 2,
        _ => 0,
    }
}

/// Parse codecs string into video and audio components
fn parse_codecs(codecs: &str) -> (Option<String>, Option<String>) {
    let parts: Vec<&str> = codecs.split(',').map(|s| s.trim()).collect();
    
    let mut video = None;
    let mut audio = None;
    
    for codec in parts {
        let codec_lower = codec.to_lowercase();
        if codec_lower.starts_with("avc")   // H.264
            || codec_lower.starts_with("hvc")  // H.265
            || codec_lower.starts_with("hev")  // H.265
            || codec_lower.starts_with("vp")   // VP8/VP9
            || codec_lower.starts_with("av1")
        {
            video = Some(codec.to_string());
        } else if codec_lower.starts_with("mp4a") // AAC
            || codec_lower.starts_with("ac-")  // Dolby
            || codec_lower.starts_with("ec-")  // Dolby
            || codec_lower.starts_with("opus")
            || codec_lower.starts_with("flac")
        {
            audio = Some(codec.to_string());
        }
    }
    
    (video, audio)
}

/// Alternative rendition (audio/subtitle track)
#[derive(Clone, Debug)]
pub struct Rendition {
    pub media_type: RenditionType,
    pub group_id: String,
    pub name: String,
    pub language: Option<String>,
    pub assoc_language: Option<String>,
    pub default: bool,
    pub autoselect: bool,
    pub forced: bool,
    pub characteristics: Option<String>,
    pub channels: Option<String>,
    pub uri: Option<String>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RenditionType {
    Audio,
    Video,
    Subtitles,
    ClosedCaptions,
}

impl Rendition {
    pub fn parse(attrs: &HashMap<String, String>) -> Option<Self> {
        let media_type = match attrs.get("TYPE")?.to_uppercase().as_str() {
            "AUDIO" => RenditionType::Audio,
            "VIDEO" => RenditionType::Video,
            "SUBTITLES" => RenditionType::Subtitles,
            "CLOSED-CAPTIONS" => RenditionType::ClosedCaptions,
            _ => return None,
        };
        
        Some(Self {
            media_type,
            group_id: attrs.get("GROUP-ID")?.trim_matches('"').to_string(),
            name: attrs.get("NAME").map(|s| s.trim_matches('"').to_string()).unwrap_or_default(),
            language: attrs.get("LANGUAGE").map(|s| s.trim_matches('"').to_string()),
            assoc_language: attrs.get("ASSOC-LANGUAGE").map(|s| s.trim_matches('"').to_string()),
            default: attrs.get("DEFAULT").map(|s| s.eq_ignore_ascii_case("YES")).unwrap_or(false),
            autoselect: attrs.get("AUTOSELECT").map(|s| s.eq_ignore_ascii_case("YES")).unwrap_or(false),
            forced: attrs.get("FORCED").map(|s| s.eq_ignore_ascii_case("YES")).unwrap_or(false),
            characteristics: attrs.get("CHARACTERISTICS").map(|s| s.trim_matches('"').to_string()),
            channels: attrs.get("CHANNELS").map(|s| s.trim_matches('"').to_string()),
            uri: attrs.get("URI").map(|s| s.trim_matches('"').to_string()),
        })
    }
}

/// Master playlist containing quality levels and renditions
#[derive(Clone, Debug)]
pub struct MasterPlaylist {
    pub levels: Vec<Level>,
    pub audio_renditions: Vec<Rendition>,
    pub subtitle_renditions: Vec<Rendition>,
    pub cc_renditions: Vec<Rendition>,
    pub session_data: Vec<SessionData>,
    pub session_keys: Vec<KeyInfo>,
}

/// Session data from playlist
#[derive(Clone, Debug)]
pub struct SessionData {
    pub data_id: String,
    pub value: Option<String>,
    pub uri: Option<String>,
    pub language: Option<String>,
}

/// Media segment in a playlist
#[derive(Clone, Debug)]
pub struct Segment {
    /// Sequence number
    pub sequence_number: u64,
    /// Duration in seconds
    pub duration: f64,
    /// Segment URI
    pub uri: String,
    /// Start time in seconds (calculated)
    pub start_time: f64,
    /// Byte range (offset, length)
    pub byte_range: Option<(u64, u64)>,
    /// Discontinuity before this segment
    pub discontinuity: bool,
    /// Program date time
    pub program_date_time: Option<String>,
    /// Title/comment
    pub title: Option<String>,
    /// Encryption key info
    pub key: Option<KeyInfo>,
    /// Init segment info (for fMP4)
    pub map: Option<MapInfo>,
    /// Gap marker
    pub gap: bool,
}

/// Encryption key information
#[derive(Clone, Debug)]
pub struct KeyInfo {
    pub method: EncryptionMethod,
    pub uri: Option<String>,
    pub iv: Option<Vec<u8>>,
    pub key_format: Option<String>,
    pub key_format_versions: Option<String>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum EncryptionMethod {
    None,
    Aes128,
    SampleAes,
    SampleAesCtr,
    SampleAesCenc,
}

impl EncryptionMethod {
    pub fn parse(s: &str) -> Self {
        match s.to_uppercase().as_str() {
            "NONE" => Self::None,
            "AES-128" => Self::Aes128,
            "SAMPLE-AES" => Self::SampleAes,
            "SAMPLE-AES-CTR" => Self::SampleAesCtr,
            "SAMPLE-AES-CENC" => Self::SampleAesCenc,
            _ => Self::None,
        }
    }
}

/// Initialization segment info
#[derive(Clone, Debug)]
pub struct MapInfo {
    pub uri: String,
    pub byte_range: Option<(u64, u64)>,
}

/// Date range tag
#[derive(Clone, Debug)]
pub struct DateRangeTag {
    pub id: String,
    pub class: Option<String>,
    pub start_date: String,
    pub end_date: Option<String>,
    pub duration: Option<f64>,
    pub planned_duration: Option<f64>,
    pub scte35_cmd: Option<String>,
    pub scte35_out: Option<String>,
    pub scte35_in: Option<String>,
    pub end_on_next: bool,
    pub client_attributes: HashMap<String, String>,
}

/// Media playlist containing segments
#[derive(Clone, Debug)]
pub struct MediaPlaylist {
    /// Target segment duration
    pub target_duration: f64,
    /// First segment sequence number
    pub media_sequence: u64,
    /// Discontinuity sequence number
    pub discontinuity_sequence: u64,
    /// Is VOD (video on demand) or live
    pub is_vod: bool,
    /// End list marker present
    pub ended: bool,
    /// Playlist type (VOD/EVENT)
    pub playlist_type: Option<String>,
    /// Total duration in seconds
    pub total_duration: f64,
    /// Segments
    pub segments: Vec<Segment>,
    /// Current init segment (for fMP4)
    pub current_map: Option<MapInfo>,
    /// Date range tags
    pub date_ranges: Vec<DateRangeTag>,
    /// Start offset
    pub start_offset: Option<f64>,
    /// Part information for LL-HLS
    pub part_inf: Option<PartInfo>,
    /// Server control for LL-HLS
    pub server_control: Option<ServerControl>,
}

/// Part information for low-latency HLS
#[derive(Clone, Debug)]
pub struct PartInfo {
    pub part_target: f64,
}

/// Server control for LL-HLS
#[derive(Clone, Debug)]
pub struct ServerControl {
    pub can_skip_until: Option<f64>,
    pub can_block_reload: bool,
    pub hold_back: Option<f64>,
    pub part_hold_back: Option<f64>,
}

/// Parse attribute list from a tag line
fn parse_attributes(line: &str) -> HashMap<String, String> {
    let mut attrs = HashMap::new();
    let mut chars = line.chars().peekable();
    
    while chars.peek().is_some() {
        // Skip whitespace
        while chars.peek() == Some(&' ') || chars.peek() == Some(&',') {
            chars.next();
        }
        
        // Read attribute name
        let mut name = String::new();
        while let Some(&c) = chars.peek() {
            if c == '=' {
                chars.next();
                break;
            }
            name.push(c);
            chars.next();
        }
        
        if name.is_empty() {
            break;
        }
        
        // Read attribute value
        let mut value = String::new();
        let quoted = chars.peek() == Some(&'"');
        if quoted {
            chars.next(); // Skip opening quote
            while let Some(&c) = chars.peek() {
                chars.next();
                if c == '"' {
                    break;
                }
                value.push(c);
            }
        } else {
            while let Some(&c) = chars.peek() {
                if c == ',' {
                    break;
                }
                value.push(c);
                chars.next();
            }
        }
        
        attrs.insert(name.trim().to_string(), value);
    }
    
    attrs
}

impl MasterPlaylist {
    /// Parse a master playlist from text
    pub fn parse(text: &str, base_url: &str) -> HlsResult<Self> {
        let lines: Vec<&str> = text.lines().collect();
        
        if lines.is_empty() || !lines[0].starts_with("#EXTM3U") {
            return Err(HlsError::manifest(
                ErrorCode::ManifestParsingError,
                "Invalid M3U8 playlist - missing #EXTM3U tag",
            ));
        }
        
        let mut levels = Vec::new();
        let mut audio_renditions = Vec::new();
        let mut subtitle_renditions = Vec::new();
        let mut cc_renditions = Vec::new();
        let mut session_data = Vec::new();
        let mut session_keys = Vec::new();
        let mut pending_stream_inf: Option<HashMap<String, String>> = None;
        
        for (_i, line) in lines.iter().enumerate() {
            let line = line.trim();
            
            if let Some(rest) = line.strip_prefix("#EXT-X-STREAM-INF:") {
                pending_stream_inf = Some(parse_attributes(rest));
            } else if let Some(ref attrs) = pending_stream_inf {
                if !line.starts_with('#') && !line.is_empty() {
                    let url = resolve_url(base_url, line);
                    levels.push(Level::from_stream_inf(levels.len(), attrs, &url));
                    pending_stream_inf = None;
                }
            } else if let Some(rest) = line.strip_prefix("#EXT-X-MEDIA:") {
                let attrs = parse_attributes(rest);
                if let Some(rendition) = Rendition::parse(&attrs) {
                    match rendition.media_type {
                        RenditionType::Audio => audio_renditions.push(rendition),
                        RenditionType::Subtitles => subtitle_renditions.push(rendition),
                        RenditionType::ClosedCaptions => cc_renditions.push(rendition),
                        _ => {}
                    }
                }
            } else if let Some(rest) = line.strip_prefix("#EXT-X-SESSION-DATA:") {
                let attrs = parse_attributes(rest);
                if let Some(data_id) = attrs.get("DATA-ID") {
                    session_data.push(SessionData {
                        data_id: data_id.clone(),
                        value: attrs.get("VALUE").cloned(),
                        uri: attrs.get("URI").cloned(),
                        language: attrs.get("LANGUAGE").cloned(),
                    });
                }
            } else if let Some(rest) = line.strip_prefix("#EXT-X-SESSION-KEY:") {
                let attrs = parse_attributes(rest);
                if let Some(key) = parse_key_info(&attrs) {
                    session_keys.push(key);
                }
            }
        }
        
        // Sort levels by bitrate (descending by default, but user can re-sort)
        levels.sort_by(|a, b| b.bitrate.cmp(&a.bitrate));
        
        // Re-index after sort
        for (i, level) in levels.iter_mut().enumerate() {
            level.index = i;
        }
        
        Ok(Self {
            levels,
            audio_renditions,
            subtitle_renditions,
            cc_renditions,
            session_data,
            session_keys,
        })
    }
    
    /// Get level by index, with bounds checking
    pub fn get_level(&self, index: usize) -> Option<&Level> {
        self.levels.get(index)
    }
    
    /// Find levels that fit within resolution constraints
    pub fn levels_within_resolution(&self, max_width: u32, max_height: u32) -> Vec<&Level> {
        self.levels
            .iter()
            .filter(|l| l.fits_resolution(max_width, max_height))
            .collect()
    }
    
    /// Find the best level for a given bandwidth
    pub fn best_level_for_bandwidth(&self, bandwidth: u64, factor: f64) -> Option<usize> {
        let effective_bandwidth = (bandwidth as f64 * factor) as u64;
        
        self.levels
            .iter()
            .filter(|l| l.bitrate <= effective_bandwidth)
            .max_by_key(|l| l.bitrate)
            .map(|l| l.index)
    }
}

impl MediaPlaylist {
    /// Parse a media playlist from text
    pub fn parse(text: &str, base_url: &str) -> HlsResult<Self> {
        let lines: Vec<&str> = text.lines().collect();
        
        if lines.is_empty() || !lines[0].starts_with("#EXTM3U") {
            return Err(HlsError::manifest(
                ErrorCode::ManifestParsingError,
                "Invalid M3U8 playlist - missing #EXTM3U tag",
            ));
        }
        
        let mut target_duration = 0.0;
        let mut media_sequence = 0;
        let mut discontinuity_sequence = 0;
        let mut playlist_type = None;
        let mut ended = false;
        let mut segments = Vec::new();
        let mut date_ranges = Vec::new();
        let mut current_map: Option<MapInfo> = None;
        let mut current_key: Option<KeyInfo> = None;
        let mut start_offset = None;
        let mut part_inf = None;
        let mut server_control = None;
        
        let mut pending_duration: Option<f64> = None;
        let mut pending_title: Option<String> = None;
        let mut pending_discontinuity = false;
        let mut pending_program_date_time: Option<String> = None;
        let mut pending_byte_range: Option<(u64, u64)> = None;
        let mut pending_gap = false;
        
        let mut current_time = 0.0;
        
        for line in lines.iter() {
            let line = line.trim();
            
            if let Some(rest) = line.strip_prefix("#EXTINF:") {
                let parts: Vec<&str> = rest.splitn(2, ',').collect();
                pending_duration = parts[0].trim().parse().ok();
                pending_title = parts.get(1).map(|s| s.trim().to_string());
            } else if let Some(rest) = line.strip_prefix("#EXT-X-TARGETDURATION:") {
                target_duration = rest.trim().parse().unwrap_or(0.0);
            } else if let Some(rest) = line.strip_prefix("#EXT-X-MEDIA-SEQUENCE:") {
                media_sequence = rest.trim().parse().unwrap_or(0);
            } else if let Some(rest) = line.strip_prefix("#EXT-X-DISCONTINUITY-SEQUENCE:") {
                discontinuity_sequence = rest.trim().parse().unwrap_or(0);
            } else if let Some(rest) = line.strip_prefix("#EXT-X-PLAYLIST-TYPE:") {
                playlist_type = Some(rest.trim().to_string());
            } else if line == "#EXT-X-ENDLIST" {
                ended = true;
            } else if line == "#EXT-X-DISCONTINUITY" {
                pending_discontinuity = true;
            } else if line == "#EXT-X-GAP" {
                pending_gap = true;
            } else if let Some(rest) = line.strip_prefix("#EXT-X-PROGRAM-DATE-TIME:") {
                pending_program_date_time = Some(rest.trim().to_string());
            } else if let Some(rest) = line.strip_prefix("#EXT-X-BYTERANGE:") {
                pending_byte_range = parse_byte_range(rest.trim());
            } else if let Some(rest) = line.strip_prefix("#EXT-X-MAP:") {
                let attrs = parse_attributes(rest);
                if let Some(uri) = attrs.get("URI") {
                    current_map = Some(MapInfo {
                        uri: resolve_url(base_url, uri),
                        byte_range: attrs.get("BYTERANGE").and_then(|s| parse_byte_range(s)),
                    });
                }
            } else if let Some(rest) = line.strip_prefix("#EXT-X-KEY:") {
                let attrs = parse_attributes(rest);
                current_key = parse_key_info(&attrs);
            } else if let Some(rest) = line.strip_prefix("#EXT-X-DATERANGE:") {
                let attrs = parse_attributes(rest);
                if let Some(dr) = parse_date_range(&attrs) {
                    date_ranges.push(dr);
                }
            } else if let Some(rest) = line.strip_prefix("#EXT-X-START:") {
                let attrs = parse_attributes(rest);
                if let Some(offset) = attrs.get("TIME-OFFSET") {
                    start_offset = offset.parse().ok();
                }
            } else if let Some(rest) = line.strip_prefix("#EXT-X-PART-INF:") {
                let attrs = parse_attributes(rest);
                if let Some(target) = attrs.get("PART-TARGET") {
                    part_inf = Some(PartInfo {
                        part_target: target.parse().unwrap_or(0.0),
                    });
                }
            } else if let Some(rest) = line.strip_prefix("#EXT-X-SERVER-CONTROL:") {
                let attrs = parse_attributes(rest);
                server_control = Some(ServerControl {
                    can_skip_until: attrs.get("CAN-SKIP-UNTIL").and_then(|s| s.parse().ok()),
                    can_block_reload: attrs.get("CAN-BLOCK-RELOAD")
                        .map(|s| s.eq_ignore_ascii_case("YES"))
                        .unwrap_or(false),
                    hold_back: attrs.get("HOLD-BACK").and_then(|s| s.parse().ok()),
                    part_hold_back: attrs.get("PART-HOLD-BACK").and_then(|s| s.parse().ok()),
                });
            } else if !line.starts_with('#') && !line.is_empty() {
                // This is a segment URI
                if let Some(duration) = pending_duration {
                    let url = resolve_url(base_url, line);
                    let sn = media_sequence + segments.len() as u64;
                    
                    segments.push(Segment {
                        sequence_number: sn,
                        duration,
                        uri: url,
                        start_time: current_time,
                        byte_range: pending_byte_range.take(),
                        discontinuity: pending_discontinuity,
                        program_date_time: pending_program_date_time.take(),
                        title: pending_title.take(),
                        key: current_key.clone(),
                        map: current_map.clone(),
                        gap: pending_gap,
                    });
                    
                    current_time += duration;
                    pending_duration = None;
                    pending_discontinuity = false;
                    pending_gap = false;
                }
            }
        }
        
        let is_vod = playlist_type.as_ref().map(|t| t == "VOD").unwrap_or(ended);
        let total_duration = current_time;
        
        Ok(Self {
            target_duration,
            media_sequence,
            discontinuity_sequence,
            is_vod,
            ended,
            playlist_type,
            total_duration,
            segments,
            current_map,
            date_ranges,
            start_offset,
            part_inf,
            server_control,
        })
    }
    
    /// Get segment at a given time position
    pub fn segment_at_time(&self, time: f64) -> Option<&Segment> {
        self.segments.iter().find(|s| {
            time >= s.start_time && time < s.start_time + s.duration
        })
    }
    
    /// Get segment index at a given time position  
    pub fn segment_index_at_time(&self, time: f64) -> Option<usize> {
        self.segments.iter().position(|s| {
            time >= s.start_time && time < s.start_time + s.duration
        })
    }
    
    /// Get segment by sequence number
    pub fn segment_by_sn(&self, sn: u64) -> Option<&Segment> {
        self.segments.iter().find(|s| s.sequence_number == sn)
    }
    
    /// Check if playlist has encryption
    pub fn is_encrypted(&self) -> bool {
        self.segments.iter().any(|s| {
            s.key.as_ref().map(|k| k.method != EncryptionMethod::None).unwrap_or(false)
        })
    }
}

/// Parse byte range "length@offset" or "length"
fn parse_byte_range(s: &str) -> Option<(u64, u64)> {
    let parts: Vec<&str> = s.split('@').collect();
    let length: u64 = parts[0].parse().ok()?;
    let offset: u64 = parts.get(1).and_then(|s| s.parse().ok()).unwrap_or(0);
    Some((offset, length))
}

/// Parse key info from attributes
fn parse_key_info(attrs: &HashMap<String, String>) -> Option<KeyInfo> {
    let method_str = attrs.get("METHOD")?;
    let method = EncryptionMethod::parse(method_str);
    
    let uri = attrs.get("URI").map(|s| s.to_string());
    let iv = attrs.get("IV").and_then(|s| parse_hex_string(s));
    let key_format = attrs.get("KEYFORMAT").map(|s| s.to_string());
    let key_format_versions = attrs.get("KEYFORMATVERSIONS").map(|s| s.to_string());
    
    Some(KeyInfo {
        method,
        uri,
        iv,
        key_format,
        key_format_versions,
    })
}

/// Parse hex string (with or without 0x prefix)
fn parse_hex_string(s: &str) -> Option<Vec<u8>> {
    let s = s.trim_start_matches("0x").trim_start_matches("0X");
    if s.len() % 2 != 0 {
        return None;
    }
    
    let bytes: Result<Vec<u8>, _> = (0..s.len())
        .step_by(2)
        .map(|i| u8::from_str_radix(&s[i..i + 2], 16))
        .collect();
    
    bytes.ok()
}

/// Parse date range attributes
fn parse_date_range(attrs: &HashMap<String, String>) -> Option<DateRangeTag> {
    let id = attrs.get("ID")?.to_string();
    let start_date = attrs.get("START-DATE")?.to_string();
    
    let mut client_attributes = HashMap::new();
    for (key, value) in attrs {
        if key.starts_with("X-") {
            client_attributes.insert(key.clone(), value.clone());
        }
    }
    
    Some(DateRangeTag {
        id,
        class: attrs.get("CLASS").cloned(),
        start_date,
        end_date: attrs.get("END-DATE").cloned(),
        duration: attrs.get("DURATION").and_then(|s| s.parse().ok()),
        planned_duration: attrs.get("PLANNED-DURATION").and_then(|s| s.parse().ok()),
        scte35_cmd: attrs.get("SCTE35-CMD").cloned(),
        scte35_out: attrs.get("SCTE35-OUT").cloned(),
        scte35_in: attrs.get("SCTE35-IN").cloned(),
        end_on_next: attrs.get("END-ON-NEXT").map(|s| s.eq_ignore_ascii_case("YES")).unwrap_or(false),
        client_attributes,
    })
}

/// Resolve a URL relative to a base URL
pub fn resolve_url(base: &str, relative: &str) -> String {
    if relative.starts_with("http://") || relative.starts_with("https://") {
        return relative.to_string();
    }
    
    if relative.starts_with('/') {
        // Absolute path - find origin
        if let Some(idx) = base.find("://") {
            let after_scheme = &base[idx + 3..];
            if let Some(path_start) = after_scheme.find('/') {
                let origin = &base[..idx + 3 + path_start];
                return format!("{}{}", origin, relative);
            }
        }
        return format!("{}{}", base.trim_end_matches('/'), relative);
    }
    
    // Relative path - join with base directory
    let base_dir = if let Some(idx) = base.rfind('/') {
        &base[..=idx]
    } else {
        base
    };
    
    format!("{}{}", base_dir, relative)
}

/// Determine if content is a master playlist
pub fn is_master_playlist(text: &str) -> bool {
    text.contains("#EXT-X-STREAM-INF") || text.contains("#EXT-X-MEDIA:")
}
