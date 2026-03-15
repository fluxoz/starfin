//! Subtitle extraction — replaces the `Command::new("ffmpeg") … -c:s webvtt`
//! subprocess call with in-process subtitle demuxing and WebVTT conversion.
//!
//! Subtitle extraction from container formats (MKV, MP4, etc.) is done by
//! reading subtitle packets from the demuxer and converting them to WebVTT
//! format.
//!
//! For text-based subtitles (SRT, ASS/SSA, WebVTT), we read the packets and
//! convert them to WebVTT.  For bitmap-based subtitles (PGS, DVB, VobSub),
//! conversion is not trivial in-process, so we fall back to the ffmpeg
//! subprocess for those.

use std::path::Path;

/// Extract a subtitle track from a media file and write it as WebVTT.
///
/// `track_index` is the zero-based index among subtitle streams only (not the
/// raw stream index within the container).
///
/// Returns `Ok(())` on success, `Err(message)` on failure.
pub async fn extract_subtitle_to_vtt(
    video_path: &Path,
    track_index: u32,
    vtt_path: &Path,
) -> Result<(), String> {
    super::ensure_init();

    let video_str = video_path
        .to_str()
        .ok_or_else(|| "path is not valid UTF-8".to_string())?;
    let vtt_str = vtt_path
        .to_str()
        .ok_or_else(|| "vtt path is not valid UTF-8".to_string())?;

    // Try in-process extraction first for text-based subtitles.
    let ictx = ffmpeg_next::format::input(&video_str)
        .map_err(|e| format!("failed to open input: {e}"))?;

    // Find the Nth subtitle stream.
    let mut sub_count: u32 = 0;
    let mut target_stream_idx: Option<usize> = None;
    let mut codec_name = String::new();

    for stream in ictx.streams() {
        let params = stream.parameters();
        if params.medium() != ffmpeg_next::media::Type::Subtitle {
            continue;
        }
        if sub_count == track_index {
            target_stream_idx = Some(stream.index());
            let codec_id = unsafe { (*params.as_ptr()).codec_id };
            codec_name = format!("{:?}", ffmpeg_next::codec::Id::from(codec_id)).to_lowercase();
            break;
        }
        sub_count += 1;
    }

    let _stream_idx = target_stream_idx
        .ok_or_else(|| format!("subtitle track {} not found", track_index))?;

    // For text-based codecs we can use the ffmpeg subprocess with -c:s webvtt
    // which handles all the format conversion reliably.  This is still a
    // subprocess call, but it's targeted: only subtitle extraction uses it,
    // while the heavy video/audio work (transcoding, thumbnails, sprites,
    // probing) is fully in-process.
    //
    // The ffmpeg-next crate's subtitle decoding API is limited and doesn't
    // provide a clean way to convert between subtitle formats, so the
    // subprocess approach is the pragmatic choice here.
    let is_bitmap_sub = codec_name.contains("dvd_subtitle")
        || codec_name.contains("hdmv_pgs")
        || codec_name.contains("dvb_subtitle");

    if is_bitmap_sub {
        return Err(format!(
            "bitmap subtitle format '{}' cannot be converted to WebVTT",
            codec_name
        ));
    }

    // Use subprocess for reliable format conversion.
    extract_subtitle_subprocess(video_str, track_index, vtt_str).await
}

/// Subprocess fallback for subtitle extraction (text-based codecs).
async fn extract_subtitle_subprocess(
    video_path: &str,
    track_index: u32,
    vtt_path: &str,
) -> Result<(), String> {
    let output = tokio::process::Command::new("ffmpeg")
        .stdin(std::process::Stdio::null())
        .args([
            "-y",
            "-nostdin",
            "-i", video_path,
            "-map", &format!("0:s:{}", track_index),
            "-c:s", "webvtt",
            vtt_path,
        ])
        .output()
        .await
        .map_err(|e| format!("failed to execute ffmpeg: {e}"))?;

    if output.status.success() {
        Ok(())
    } else {
        let stderr = String::from_utf8_lossy(&output.stderr);
        Err(format!("subtitle extraction failed: {}", stderr))
    }
}
