//! Subtitle extraction — fully in-process via `ffmpeg-next`.
//!
//! Subtitle extraction from container formats (MKV, MP4, etc.) is done by
//! reading subtitle packets from the demuxer, decoding them with the
//! appropriate codec, and converting the result to WebVTT format.
//!
//! Text-based subtitles (SRT, ASS/SSA, WebVTT, MOV_TEXT, etc.) are decoded
//! in-process and written as WebVTT.  Bitmap-based subtitles (PGS, DVB,
//! VobSub) cannot be converted to text-based WebVTT and are rejected.

use std::fmt::Write as _;
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
    let video = video_path.to_path_buf();
    let vtt = vtt_path.to_path_buf();
    tokio::task::spawn_blocking(move || extract_subtitle_inprocess(&video, track_index, &vtt))
        .await
        .map_err(|e| format!("subtitle task panicked: {e}"))?
}

/// In-process subtitle extraction using ffmpeg-next's subtitle decoder.
fn extract_subtitle_inprocess(
    video_path: &Path,
    track_index: u32,
    vtt_path: &Path,
) -> Result<(), String> {
    super::ensure_init();

    let mut ictx = ffmpeg_next::format::input(video_path)
        .map_err(|e| format!("failed to open input: {e}"))?;

    // Find the Nth subtitle stream and its raw stream index.
    let mut sub_count: u32 = 0;
    let mut target_stream_idx: Option<usize> = None;
    let mut codec_name = String::new();
    let mut stream_time_base = ffmpeg_next::Rational::new(1, 1000);

    for stream in ictx.streams() {
        let params = stream.parameters();
        if params.medium() != ffmpeg_next::media::Type::Subtitle {
            continue;
        }
        if sub_count == track_index {
            target_stream_idx = Some(stream.index());
            stream_time_base = stream.time_base();
            let codec_id = unsafe { (*params.as_ptr()).codec_id };
            codec_name = format!("{:?}", ffmpeg_next::codec::Id::from(codec_id)).to_lowercase();
            break;
        }
        sub_count += 1;
    }

    let stream_idx = target_stream_idx
        .ok_or_else(|| format!("subtitle track {} not found", track_index))?;

    let is_bitmap_sub = codec_name.contains("dvd_subtitle")
        || codec_name.contains("hdmv_pgs")
        || codec_name.contains("dvb_subtitle");

    if is_bitmap_sub {
        return Err(format!(
            "bitmap subtitle format '{}' cannot be converted to WebVTT",
            codec_name
        ));
    }

    // Set up subtitle decoder.
    let stream = ictx.stream(stream_idx).unwrap();
    let decoder_ctx = ffmpeg_next::codec::context::Context::from_parameters(stream.parameters())
        .map_err(|e| format!("subtitle decoder context: {e}"))?;
    let mut decoder = decoder_ctx
        .decoder()
        .subtitle()
        .map_err(|e| format!("subtitle decoder: {e}"))?;

    // Collect decoded subtitle cues.
    let mut cues: Vec<(f64, f64, String)> = Vec::new();

    for (pkt_stream, packet) in ictx.packets() {
        if pkt_stream.index() != stream_idx {
            continue;
        }

        let mut subtitle = ffmpeg_next::Subtitle::new();
        match decoder.decode(&packet, &mut subtitle) {
            Ok(true) => {}
            Ok(false) => continue,
            Err(_) => continue,
        }

        // Compute absolute timestamps.
        // PTS is in AV_TIME_BASE (1_000_000) units.
        // start_display_time / end_display_time are in milliseconds relative
        // to the PTS.
        let pts_secs = if let Some(pts) = subtitle.pts() {
            pts as f64 / f64::from(ffmpeg_next::ffi::AV_TIME_BASE)
        } else if let Some(pkt_pts) = packet.pts() {
            pkt_pts as f64 * f64::from(stream_time_base.0) / f64::from(stream_time_base.1)
        } else {
            continue;
        };

        let start_secs = pts_secs + subtitle.start() as f64 / 1000.0;
        let end_secs = pts_secs + subtitle.end() as f64 / 1000.0;

        if end_secs <= start_secs {
            continue;
        }

        // Extract text from subtitle rects.
        let mut text = String::new();
        for rect in subtitle.rects() {
            match rect {
                ffmpeg_next::subtitle::Rect::Text(t) => {
                    let s = t.get().trim();
                    if !s.is_empty() {
                        if !text.is_empty() {
                            text.push('\n');
                        }
                        text.push_str(s);
                    }
                }
                ffmpeg_next::subtitle::Rect::Ass(a) => {
                    let plain = ass_to_plain_text(a.get());
                    let s = plain.trim();
                    if !s.is_empty() {
                        if !text.is_empty() {
                            text.push('\n');
                        }
                        text.push_str(s);
                    }
                }
                _ => {}
            }
        }

        if !text.is_empty() {
            cues.push((start_secs, end_secs, text));
        }
    }

    // Sort cues by start time (they should already be in order, but be safe).
    cues.sort_by(|a, b| a.0.partial_cmp(&b.0).unwrap_or(std::cmp::Ordering::Equal));

    // Write WebVTT output.
    let mut output = String::from("WEBVTT\n\n");
    for (i, (start, end, text)) in cues.iter().enumerate() {
        let _ = writeln!(output, "{}", i + 1);
        let _ = writeln!(output, "{} --> {}", format_vtt_time(*start), format_vtt_time(*end));
        let _ = writeln!(output, "{}", text);
        output.push('\n');
    }

    std::fs::write(vtt_path, output)
        .map_err(|e| format!("failed to write VTT file: {e}"))
}

/// Format seconds as a WebVTT timestamp: `HH:MM:SS.mmm`.
fn format_vtt_time(secs: f64) -> String {
    let total_ms = (secs * 1000.0).round() as u64;
    let ms = total_ms % 1000;
    let total_s = total_ms / 1000;
    let s = total_s % 60;
    let total_m = total_s / 60;
    let m = total_m % 60;
    let h = total_m / 60;
    format!("{:02}:{:02}:{:02}.{:03}", h, m, s, ms)
}

/// Number of fields in an ASS dialogue event before the text content.
/// Format: `ReadOrder,Layer,Style,Name,MarginL,MarginR,MarginV,Effect,Text`
const ASS_FIELDS_BEFORE_TEXT: usize = 9;

/// Convert an ASS dialogue event string to plain text.
///
/// The ASS rect text from ffmpeg is typically in the format:
///   `ReadOrder,Layer,Style,Name,MarginL,MarginR,MarginV,Effect,Text`
///
/// We extract the text after the 8th comma, strip ASS override tags
/// (`{\\...}`), and convert `\\N` to newlines.
fn ass_to_plain_text(ass: &str) -> String {
    // Find the text portion after the 8th comma.
    let text_part = match ass.splitn(ASS_FIELDS_BEFORE_TEXT, ',').last() {
        Some(t) => t,
        None => ass,
    };

    // Strip ASS override tags: `{...}`.
    let mut result = String::with_capacity(text_part.len());
    let mut in_tag = false;
    for ch in text_part.chars() {
        match ch {
            '{' => in_tag = true,
            '}' => in_tag = false,
            _ if !in_tag => result.push(ch),
            _ => {}
        }
    }

    // Convert ASS newline markers to real newlines.
    result.replace("\\N", "\n").replace("\\n", "\n")
}
