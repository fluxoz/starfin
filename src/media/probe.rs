//! Metadata probing — replaces all `Command::new("ffprobe")` invocations.
//!
//! Uses `ffmpeg_next::format::input()` to open a media file and read its
//! duration and format-level tags (title, genre, date, director/artist).

use std::path::Path;

/// Metadata extracted from format-level tags.
#[derive(Debug, Default, Clone)]
pub struct ProbeMeta {
    pub title: Option<String>,
    pub genre: Option<String>,
    pub year: Option<u16>,
    pub director: Option<String>,
}

/// Codec information extracted from the first video and audio streams.
///
/// Used to populate the `codecs` attribute in DASH MPD manifests, matching
/// dash.js `SourceBufferSink._getCodecStringForRepresentation()` which
/// constructs `mimeType + ';codecs="' + codecs + '"'` from the MPD.
#[derive(Debug, Default, Clone)]
pub struct CodecInfo {
    /// RFC 6381 video codec string, e.g. `"avc1.640029"` for H.264 High L4.1.
    pub video_codec: Option<String>,
    /// RFC 6381 audio codec string, e.g. `"mp4a.40.2"` for AAC-LC.
    pub audio_codec: Option<String>,
}

/// Video stream properties needed for DASH manifest generation and UI display.
#[derive(Debug, Default, Clone, serde::Serialize)]
pub struct StreamInfo {
    /// Width of the first video stream in pixels.
    pub width: u32,
    /// Height of the first video stream in pixels.
    pub height: u32,
    /// Total bitrate of the container in bits/sec (video + audio).
    /// Falls back to the sum of individual stream bit_rates.
    pub bitrate: u64,
}

impl CodecInfo {
    /// Build a combined codecs string for DASH MPD `@codecs` attribute.
    /// Returns e.g. `"avc1.640029,mp4a.40.2"` or just `"avc1.640029"` if
    /// there is no audio.
    pub fn codecs_string(&self) -> Option<String> {
        match (&self.video_codec, &self.audio_codec) {
            (Some(v), Some(a)) => Some(format!("{v},{a}")),
            (Some(v), None) => Some(v.clone()),
            (None, Some(a)) => Some(a.clone()),
            (None, None) => None,
        }
    }
}

/// Detect codec information from a media file.
///
/// Reads the first video and first audio stream and builds RFC 6381 codec
/// strings.  For H.264 this inspects the profile/level stored in
/// `AVCodecParameters` to produce the `avc1.PPCCLL` form that browsers
/// require in `MediaSource.addSourceBuffer()`.
pub fn probe_codecs(path: &Path) -> CodecInfo {
    super::ensure_init();

    let input = match ffmpeg_next::format::input(path) {
        Ok(ctx) => ctx,
        Err(_) => return CodecInfo::default(),
    };

    let mut info = CodecInfo::default();

    for stream in input.streams() {
        let params = stream.parameters();
        match params.medium() {
            ffmpeg_next::media::Type::Video if info.video_codec.is_none() => {
                info.video_codec = Some(video_codec_string(&params));
            }
            ffmpeg_next::media::Type::Audio if info.audio_codec.is_none() => {
                info.audio_codec = Some(audio_codec_string(&params));
            }
            _ => {}
        }
    }

    info
}

/// Probe stream-level info (resolution, bitrate) from a media file.
///
/// The returned [`StreamInfo`] contains the first video stream's width/height
/// and the container-level or summed stream-level bitrate in bits/sec.
pub fn probe_stream_info(path: &Path) -> StreamInfo {
    super::ensure_init();

    let input = match ffmpeg_next::format::input(path) {
        Ok(ctx) => ctx,
        Err(_) => return StreamInfo::default(),
    };

    let mut info = StreamInfo::default();

    // Container-level bit_rate (most reliable when set).
    let container_br = input.bit_rate() as u64;

    let mut stream_br_sum: u64 = 0;
    for stream in input.streams() {
        let params = stream.parameters();
        let ptr = unsafe { params.as_ptr() };
        let br = unsafe { (*ptr).bit_rate } as u64;
        stream_br_sum += br;

        if params.medium() == ffmpeg_next::media::Type::Video && info.width == 0 {
            let (w, h) = unsafe { ((*ptr).width as u32, (*ptr).height as u32) };
            info.width = w;
            info.height = h;
        }
    }

    info.bitrate = if container_br > 0 { container_br } else { stream_br_sum };
    info
}

/// Build an RFC 6381 video codec string from `AVCodecParameters`.
fn video_codec_string(params: &ffmpeg_next::codec::Parameters) -> String {
    let ptr = unsafe { params.as_ptr() };
    // Safety: `ptr` is valid as long as `params` is alive.
    let (codec_id, profile, level) = unsafe {
        ((*ptr).codec_id, (*ptr).profile, (*ptr).level)
    };
    let id = ffmpeg_next::codec::Id::from(codec_id);

    match id {
        ffmpeg_next::codec::Id::H264 => {
            // H.264 / AVC: avc1.PPCCLL
            //   PP = profile_idc (hex, 2 digits)
            //   CC = constraint_set_flags (hex, 2 digits) — use 0x00 as default
            //   LL = level_idc (hex, 2 digits)
            let profile_idc: u8 = match profile {
                66  => 0x42, // Baseline
                77  => 0x4D, // Main
                88  => 0x58, // Extended
                100 => 0x64, // High
                110 => 0x6E, // High 10
                122 => 0x7A, // High 4:2:2
                244 => 0xF4, // High 4:4:4 Predictive
                _   => if profile >= 0 { profile as u8 } else { 0x64 },
            };
            // constraint_set_flags — ffmpeg doesn't expose them directly;
            // use a reasonable default per profile.
            let constraint: u8 = match profile_idc {
                0x42 => 0xC0, // Baseline: constraint_set0_flag | constraint_set1_flag
                0x4D => 0x40, // Main: constraint_set1_flag
                _    => 0x00, // High and above
            };
            let level_idc: u8 = if level > 0 { level as u8 } else { 0x29 }; // default L4.1
            format!("avc1.{profile_idc:02X}{constraint:02X}{level_idc:02X}")
        }
        ffmpeg_next::codec::Id::HEVC => {
            // H.265 / HEVC — simplified form
            format!("hev1.1.6.L{}.B0", if level > 0 { level } else { 120 })
        }
        ffmpeg_next::codec::Id::VP9 => "vp09.00.10.08".to_string(),
        ffmpeg_next::codec::Id::AV1 => "av01.0.01M.08".to_string(),
        _ => "avc1.640029".to_string(), // fallback to H.264 High L4.1
    }
}

/// Build an RFC 6381 audio codec string from `AVCodecParameters`.
fn audio_codec_string(params: &ffmpeg_next::codec::Parameters) -> String {
    let ptr = unsafe { params.as_ptr() };
    let codec_id = unsafe { (*ptr).codec_id };
    let id = ffmpeg_next::codec::Id::from(codec_id);

    match id {
        ffmpeg_next::codec::Id::AAC => "mp4a.40.2".to_string(),    // AAC-LC
        ffmpeg_next::codec::Id::EAC3 => "ec-3".to_string(),        // E-AC3
        ffmpeg_next::codec::Id::AC3 => "ac-3".to_string(),
        ffmpeg_next::codec::Id::OPUS => "opus".to_string(),
        ffmpeg_next::codec::Id::VORBIS => "vorbis".to_string(),
        ffmpeg_next::codec::Id::FLAC => "fLaC".to_string(),
        ffmpeg_next::codec::Id::MP3 => "mp4a.40.34".to_string(),
        _ => "mp4a.40.2".to_string(), // fallback to AAC-LC
    }
}

/// Probe a media file and return `(duration_secs, metadata)`.
///
/// Returns `(0.0, ProbeMeta::default())` on any error (file not found, corrupt
/// header, etc.) — mirroring the old ffprobe-based fallback behaviour.
///
/// The duration is returned as `f64` (fractional seconds) so that DASH
/// manifests and MSE `MediaSource.duration` get sub-second precision.
/// Callers that only need integer seconds for display (e.g. grid cards)
/// should truncate with `as u32`.
pub fn probe_video(path: &Path) -> (f64, ProbeMeta) {
    super::ensure_init();

    let input = match ffmpeg_next::format::input(path) {
        Ok(ctx) => ctx,
        Err(_) => return (0.0, ProbeMeta::default()),
    };

    // Duration: libavformat stores it in AV_TIME_BASE (1 000 000) units.
    let duration_secs: f64 = if input.duration() >= 0 {
        input.duration() as f64 / f64::from(ffmpeg_next::ffi::AV_TIME_BASE)
    } else {
        0.0
    };

    let meta = input.metadata();

    let title = meta.get("title").map(str::to_owned);
    let genre = meta.get("genre").map(str::to_owned);
    let year = meta
        .get("date")
        .and_then(|s| s.get(..4))
        .and_then(|s| s.parse::<u16>().ok());
    let director = meta
        .get("director")
        .or_else(|| meta.get("artist"))
        .map(str::to_owned);

    (
        duration_secs,
        ProbeMeta {
            title,
            genre,
            year,
            director,
        },
    )
}

/// Information about a subtitle stream embedded in a media file.
#[derive(Debug, Clone)]
pub struct SubtitleStreamInfo {
    /// Zero-based index among subtitle streams only (not the raw stream index).
    pub index: u32,
    pub language: Option<String>,
    pub title: Option<String>,
    pub codec_name: String,
}

/// List all subtitle streams in a media file.
///
/// Returns an empty Vec if the file cannot be opened or contains no subtitle
/// streams.
pub fn list_subtitle_streams(path: &Path) -> Vec<SubtitleStreamInfo> {
    super::ensure_init();

    let input = match ffmpeg_next::format::input(path) {
        Ok(ctx) => ctx,
        Err(_) => return Vec::new(),
    };

    let mut tracks = Vec::new();
    let mut sub_index: u32 = 0;

    for stream in input.streams() {
        let params = stream.parameters();
        if params.medium() != ffmpeg_next::media::Type::Subtitle {
            continue;
        }

        let codec_id = unsafe { (*params.as_ptr()).codec_id };
        let codec_name = ffmpeg_next::codec::Id::from(codec_id);

        let meta = stream.metadata();
        let language = meta.get("language").map(str::to_owned);
        let title = meta.get("title").map(str::to_owned);

        tracks.push(SubtitleStreamInfo {
            index: sub_index,
            language,
            title,
            codec_name: format!("{:?}", codec_name).to_lowercase(),
        });
        sub_index += 1;
    }

    tracks
}
