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

/// Probe a media file and return `(duration_secs, metadata)`.
///
/// Returns `(0, ProbeMeta::default())` on any error (file not found, corrupt
/// header, etc.) — mirroring the old ffprobe-based fallback behaviour.
pub fn probe_video(path: &Path) -> (u32, ProbeMeta) {
    super::ensure_init();

    let input = match ffmpeg_next::format::input(path) {
        Ok(ctx) => ctx,
        Err(_) => return (0, ProbeMeta::default()),
    };

    // Duration: libavformat stores it in AV_TIME_BASE (1 000 000) units.
    let duration_secs = if input.duration() >= 0 {
        (input.duration() as f64 / f64::from(ffmpeg_next::ffi::AV_TIME_BASE)) as u32
    } else {
        0
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
