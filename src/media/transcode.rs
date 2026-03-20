//! DASH fMP4 segment creation — direct remux when possible, transcode as fallback.
//!
//! Each segment is a 6-second fMP4 (fragmented MP4 / CMAF) chunk.  For
//! **Original** quality with browser-compatible codecs (H.264 video + stereo
//! AAC/MP3 audio) the segment is created by **remuxing** — copying compressed
//! packets directly from the source file without decoding or re-encoding.
//! This is near-instant (pure I/O, like VLC playback) and gives performance
//! parity with direct file access.
//!
//! When the video is H.264 but the audio isn't directly usable in browsers
//! (e.g. multi-channel 5.1/7.1 AAC, or a non-AAC codec like FLAC/AC-3) the
//! **hybrid** path copies video packets losslessly while transcoding only the
//! audio to stereo AAC — giving the speed of remux with browser-compatible
//! output.
//!
//! When remuxing is not possible (incompatible video codec, or High/Medium/Low
//! quality that requires re-encoding or resolution scaling) the segment is
//! **transcoded** in-process via `ffmpeg-next`.
//!
//! Hardware-accelerated encoding (NVENC, VAAPI, QSV, etc.) is available for
//! the transcode fallback path via the raw FFI bindings.

use std::path::Path;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;

use super::hwaccel::HwAccel;

/// Duration of each DASH segment in seconds.
pub const SEGMENT_DURATION: f64 = 6.0;

/// Error message returned when a background operation is cancelled by a kill
/// flag (e.g. playback started while a background worker was running).
pub const CANCELLED: &str = "cancelled";

/// Bitrate for AAC audio encoding in the transcode and hybrid paths.
/// 256 kbps stereo AAC-LC is transparent quality for music and dialogue.
const AAC_ENCODE_BITRATE: usize = 256_000;

/// Create a single fMP4 segment — remux if possible, transcode otherwise.
///
/// For **Original** quality with H.264 + stereo AAC/MP3 source, packets are
/// copied directly (remux).  For H.264 video with multi-channel or
/// incompatible audio, video packets are copied and only audio is transcoded
/// to stereo AAC (hybrid).  For incompatible video codecs or lower quality
/// tiers, the full transcode path is used.
///
/// Writes to a temporary file first, then atomically renames.
pub async fn transcode_segment(
    abs_path: &str,
    seg_dir: &Path,
    seg_index: usize,
    hwaccel: &HwAccel,
    quality: super::transcode::Quality,
) -> Result<(), String> {
    let filename = format!("seg_{:05}.m4s", seg_index);
    let seg_path = seg_dir.join(&filename);

    if seg_path.exists() {
        return Ok(());
    }

    let start_time = seg_index as f64 * SEGMENT_DURATION;
    debug_assert!(start_time >= 0.0 && start_time.is_finite());

    // Run the I/O or CPU-intensive work on a blocking thread so we don't
    // starve the tokio runtime.
    let abs_path = abs_path.to_owned();
    let seg_dir = seg_dir.to_owned();
    let hwaccel = hwaccel.clone();
    tokio::task::spawn_blocking(move || {
        create_segment(&abs_path, &seg_dir, seg_index, &hwaccel, quality, None)
    })
    .await
    .map_err(|e| format!("transcode task panicked: {e}"))?
}

/// Like [`transcode_segment`] but accepts a shared kill flag.  When the flag
/// is set to `true`, the in-progress I/O work bails out early so that
/// background pre-caching yields CPU and disk to on-demand playback.
pub async fn transcode_segment_with_kill(
    abs_path: &str,
    seg_dir: &Path,
    seg_index: usize,
    hwaccel: &HwAccel,
    quality: super::transcode::Quality,
    kill: Arc<AtomicBool>,
) -> Result<(), String> {
    let filename = format!("seg_{:05}.m4s", seg_index);
    let seg_path = seg_dir.join(&filename);

    if seg_path.exists() {
        return Ok(());
    }

    let start_time = seg_index as f64 * SEGMENT_DURATION;
    debug_assert!(start_time >= 0.0 && start_time.is_finite());

    let abs_path = abs_path.to_owned();
    let seg_dir = seg_dir.to_owned();
    let hwaccel = hwaccel.clone();
    tokio::task::spawn_blocking(move || {
        create_segment(&abs_path, &seg_dir, seg_index, &hwaccel, quality, Some(&kill))
    })
    .await
    .map_err(|e| format!("transcode task panicked: {e}"))?
}

/// Quality / mode for on-demand segment creation.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Default, serde::Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Quality {
    /// Direct remux — copy packets without re-encoding.
    /// Fastest option: pure I/O, no CPU decode/encode.
    /// Requires source to have browser-compatible codecs (H.264 + AAC/MP3).
    /// Falls back to High transcode if source codecs are incompatible.
    #[default]
    Original,
    /// Re-encode at native resolution (CRF 18, veryslow or HW encoder).
    High,
    /// Re-encode at ≤720p (CRF 26, fast preset).
    Medium,
    /// Re-encode at ≤480p (CRF 30, faster preset).
    Low,
}

impl Quality {
    pub fn as_str(self) -> &'static str {
        match self {
            Quality::Original => "original",
            Quality::High     => "high",
            Quality::Medium   => "medium",
            Quality::Low      => "low",
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            Quality::Original => "Original",
            Quality::High     => "High",
            Quality::Medium   => "Medium",
            Quality::Low      => "Low",
        }
    }

    /// Whether this quality level can potentially use the fast remux path
    /// (no re-encoding).  Only Original uses remux; all others always
    /// transcode.
    fn can_remux(self) -> bool {
        self == Quality::Original
    }
}

// ── Codec compatibility ──────────────────────────────────────────────────────

/// Return `true` if the video stream is H.264 and can be packet-copied into
/// MPEG-TS without re-encoding.
fn video_is_remuxable(ictx: &ffmpeg_next::format::context::Input) -> bool {
    ictx.streams()
        .best(ffmpeg_next::media::Type::Video)
        .map(|s| s.parameters().id() == ffmpeg_next::codec::Id::H264)
        .unwrap_or(false)
}

/// Return `true` if the audio stream can be packet-copied into MPEG-TS and
/// played by web browsers.  This requires:
///   1. Codec is AAC or MP3 (browser-compatible).
///   2. Channel count ≤ 2 (stereo/mono).  Browsers cannot decode multi-channel
///      AAC (e.g. 5.1/7.1) in HLS/MPEG-TS — the audio track is simply dropped.
///
/// Returns `true` when there is no audio stream (video-only remux is fine).
fn audio_is_remuxable(ictx: &ffmpeg_next::format::context::Input) -> bool {
    use ffmpeg_next::codec::Id;

    ictx.streams()
        .best(ffmpeg_next::media::Type::Audio)
        .map(|s| {
            let id = s.parameters().id();
            let codec_ok = id == Id::AAC || id == Id::MP3;
            // Read channel count from codec parameters.
            // SAFETY: `s.parameters().as_ptr()` returns a valid pointer to an
            // initialized `AVCodecParameters` owned by the stream.  We only
            // read the `ch_layout.nb_channels` field (a plain integer) through
            // the pointer — no mutation, no lifetime extension.
            let channels = unsafe { (*s.parameters().as_ptr()).ch_layout.nb_channels };
            let channels_ok = channels <= 2;
            codec_ok && channels_ok
        })
        // No audio stream is fine — video-only remux is valid.
        .unwrap_or(true)
}

/// Full remux: both video and audio can be packet-copied.
fn source_is_remuxable(ictx: &ffmpeg_next::format::context::Input) -> bool {
    video_is_remuxable(ictx) && audio_is_remuxable(ictx)
}

/// Returns true if the channel layout is fully usable for the resampler —
/// meaning it passes `av_channel_layout_check` AND has a non-UNSPEC order.
///
/// `av_channel_layout_check` returns 1 (valid) even for UNSPEC layouts when
/// `nb_channels > 0`, because they describe a channel *count* even without
/// positional labels.  However, SWR uses a poor generic mixing matrix for
/// UNSPEC inputs (all channels weighted equally) rather than the proper
/// 5.1→stereo downmix matrix.  More critically, some versions of
/// `swr_alloc_set_opts2` internally convert UNSPEC→default_layout during
/// `swr_init`; when the frame then arrives still UNSPEC, SWR's
/// `av_channel_layout_compare` detects a mismatch and returns
/// AVERROR_INPUT_CHANGED.  Requiring non-UNSPEC order in both the resampler
/// config and the frame prevents this mismatch and enables proper mixing.
#[inline]
unsafe fn layout_is_fully_specified(raw: *const ffmpeg_next::ffi::AVFrame) -> bool {
    use ffmpeg_next::ffi::AVChannelOrder;
    unsafe {
        let cl = &(*raw).ch_layout;
        ffmpeg_next::ffi::av_channel_layout_check(cl) != 0
            && cl.order != AVChannelOrder::AV_CHANNEL_ORDER_UNSPEC
    }
}

/// Get a valid, non-UNSPEC channel layout for an audio frame.
///
/// On FFmpeg ≤6 (old bitflags API), an "empty" layout means `channels() == 0`.
/// On FFmpeg 7+ (AVChannelLayout struct API), the decoder may set
/// `ch_layout.nb_channels = N` but leave `ch_layout.order = UNSPEC`.
/// Both cases need to be fixed — `av_channel_layout_check` alone is
/// insufficient because it returns 1 (valid) for `{UNSPEC, nb_channels=N}`.
fn frame_channel_layout(frame: &ffmpeg_next::frame::Audio) -> ffmpeg_next::channel_layout::ChannelLayout {
    if unsafe { layout_is_fully_specified(frame.as_ptr()) } {
        // Layout already has a proper NATIVE/CUSTOM/AMBISONIC order —
        // return via the high-level wrapper (reads from the correct field
        // for each FFmpeg API version).
        return frame.channel_layout();
    }

    // Layout is UNSPEC, empty, or otherwise not fully specified.
    // Derive a proper native-order layout from the best available channel count.
    //
    // With FFmpeg 7+ (ffmpeg_7_0 feature), channels() reads ch_layout.nb_channels
    // which may be non-zero even when order == UNSPEC.
    // With FFmpeg ≤6 (no ffmpeg_7_0), channels() reads the deprecated channels
    // field which is set by the decoder even when ch_layout is not.
    let ch = frame.channels() as i32;
    let ch = if ch > 0 { ch } else {
        // Last resort: read nb_channels directly from the raw AVFrame struct.
        let raw_ch = unsafe { (*frame.as_ptr()).ch_layout.nb_channels };
        if raw_ch > 0 { raw_ch } else { 2 }
    };

    ffmpeg_next::channel_layout::ChannelLayout::default(ch)
}

/// Ensure the audio frame has a valid, non-UNSPEC channel layout set.
/// Works correctly on both FFmpeg ≤6 (old bitflags) and FFmpeg 7+
/// (AVChannelLayout struct).
///
/// Scenarios that need fixing:
///  • FFmpeg ≤6 decoders that only set the deprecated `channel_layout`
///    bitflags and leave `ch_layout.order == UNSPEC, nb_channels == 0`.
///  • FFmpeg 7/8 (Lavc62) decoders that set `ch_layout.nb_channels` but
///    leave `ch_layout.order == UNSPEC`.  `av_channel_layout_check` returns
///    1 (valid) for this case, but `swr_alloc_set_opts2` internally converts
///    UNSPEC to a default layout; if the frame still has UNSPEC when
///    `swr_convert_frame` is called, `av_channel_layout_compare` detects a
///    mismatch and returns AVERROR_INPUT_CHANGED ("Input changed").
///
/// Writes a NATIVE-order layout to BOTH `ch_layout` (via
/// `av_channel_layout_default`, checked by FFmpeg 7+ SWR) AND the deprecated
/// `channel_layout` bitflags field (via `frame.set_channel_layout`, checked
/// by FFmpeg ≤6 SWR), ensuring compatibility with every supported version.
fn ensure_frame_channel_layout(frame: &mut ffmpeg_next::frame::Audio) {
    if unsafe { layout_is_fully_specified(frame.as_ptr()) } {
        return;
    }

    // Derive a proper layout from the best available channel count.
    let ch = frame.channels() as i32;
    let ch = if ch > 0 { ch } else {
        let raw_ch = unsafe { (*frame.as_ptr()).ch_layout.nb_channels };
        if raw_ch > 0 { raw_ch } else { 2 }
    };

    // 1. Write to ch_layout via av_channel_layout_default.  This sets
    //    order = AV_CHANNEL_ORDER_NATIVE, nb_channels = ch, and the
    //    appropriate channel bitmask.  This is what FFmpeg 7+ SWR checks.
    unsafe {
        ffmpeg_next::ffi::av_channel_layout_default(
            &mut (*frame.as_mut_ptr()).ch_layout,
            ch,
        );
    }

    // 2. Also update via the high-level set_channel_layout wrapper.
    //    On FFmpeg ≤6 this writes to the deprecated channel_layout bitflags
    //    field (what the old SWR checks).  On FFmpeg 7+ it writes to ch_layout
    //    again (redundant but harmless).
    let derived = ffmpeg_next::channel_layout::ChannelLayout::default(ch);
    frame.set_channel_layout(derived);
}

/// Feed a resampled audio frame to the AAC encoder, splitting into chunks of
/// `encoder.frame_size()` samples if the frame is larger (e.g. FLAC decodes
/// 4608-sample frames but AAC requires exactly 1024).
///
/// Returns the number of encoded packets written to `octx`.
fn encode_audio_frame(
    frame: &ffmpeg_next::frame::Audio,
    encoder: &mut ffmpeg_next::encoder::Audio,
    octx: &mut ffmpeg_next::format::context::Output,
    out_stream_idx: usize,
    sample_rate: u32,
    out_tb: ffmpeg_next::Rational,
    pts_counter: &mut i64,
    ts_offset: i64,
) -> u32 {
    let frame_size = encoder.frame_size() as usize;
    let total = frame.samples();
    let channels = frame.channels() as usize;
    let bytes_per_sample = match frame.format() {
        ffmpeg_next::format::Sample::F32(_) => 4usize,
        ffmpeg_next::format::Sample::F64(_) => 8,
        ffmpeg_next::format::Sample::I16(_) => 2,
        ffmpeg_next::format::Sample::I32(_) => 4,
        other => {
            // Should not happen in practice (AAC encoder requires FLTP →
            // F32 Planar, so the resampler always outputs that).  Default
            // to 4 bytes (32-bit) as a best guess.
            eprintln!("[audio] unexpected sample format {other:?}, assuming 4 bytes/sample");
            4
        }
    };
    let is_planar = frame.is_planar();
    let mut written = 0u32;

    // If the frame fits the encoder, send directly.
    if total <= frame_size || frame_size == 0 {
        let new_pts = *pts_counter + ts_offset;
        // Clone the frame so we can set PTS.
        let mut f = frame.clone();
        f.set_pts(Some(new_pts));
        *pts_counter += total as i64;
        if encoder.send_frame(&f).is_ok() {
            let mut pkt = ffmpeg_next::Packet::empty();
            while encoder.receive_packet(&mut pkt).is_ok() {
                pkt.set_stream(out_stream_idx);
                pkt.rescale_ts(
                    ffmpeg_next::Rational::new(1, sample_rate as i32),
                    out_tb,
                );
                let _ = pkt.write_interleaved(octx);
                written += 1;
            }
        }
        return written;
    }

    // Frame is larger than encoder's frame_size — split into chunks.
    let mut offset = 0usize;
    while offset < total {
        let chunk = std::cmp::min(frame_size, total - offset);

        // Allocate a new frame for this chunk.
        unsafe {
            let chunk_frame = ffmpeg_next::ffi::av_frame_alloc();
            if chunk_frame.is_null() {
                eprintln!("[audio] av_frame_alloc failed (out of memory)");
                break;
            }
            (*chunk_frame).format = (*frame.as_ptr()).format;
            (*chunk_frame).sample_rate = frame.rate() as i32;
            (*chunk_frame).nb_samples = chunk as i32;

            // Copy channel layout from the source frame.
            ffmpeg_next::ffi::av_channel_layout_copy(
                &mut (*chunk_frame).ch_layout,
                &(*frame.as_ptr()).ch_layout,
            );

            // Allocate sample buffers.
            let ret = ffmpeg_next::ffi::av_frame_get_buffer(chunk_frame, 0);
            if ret < 0 {
                eprintln!("[audio] av_frame_get_buffer failed (error {ret})");
                ffmpeg_next::ffi::av_frame_free(&mut (chunk_frame as *mut _));
                break;
            }

            // Copy sample data using raw FFI pointers (the high-level
            // data() accessor may return a 0-length slice for resampled
            // frames if metadata like linesize isn't propagated).
            let src_ptr = (*frame.as_ptr()).data.as_ptr();
            if is_planar {
                for ch in 0..channels {
                    let src_plane = *src_ptr.add(ch);
                    if src_plane.is_null() { continue; }
                    let dst_plane = (*chunk_frame).data[ch];
                    if dst_plane.is_null() { continue; }
                    let src_off = src_plane.add(offset * bytes_per_sample);
                    std::ptr::copy_nonoverlapping(src_off, dst_plane, chunk * bytes_per_sample);
                }
            } else {
                // Packed/interleaved: one plane, samples are interleaved.
                let src_plane = *src_ptr;
                let dst_plane = (*chunk_frame).data[0];
                if !src_plane.is_null() && !dst_plane.is_null() {
                    let src_off = src_plane.add(offset * channels * bytes_per_sample);
                    std::ptr::copy_nonoverlapping(src_off, dst_plane, chunk * channels * bytes_per_sample);
                }
            }

            let new_pts = *pts_counter + ts_offset;
            (*chunk_frame).pts = new_pts;
            *pts_counter += chunk as i64;

            let wrapped = ffmpeg_next::frame::Audio::wrap(chunk_frame);
            if encoder.send_frame(&wrapped).is_ok() {
                let mut pkt = ffmpeg_next::Packet::empty();
                while encoder.receive_packet(&mut pkt).is_ok() {
                    pkt.set_stream(out_stream_idx);
                    pkt.rescale_ts(
                        ffmpeg_next::Rational::new(1, sample_rate as i32),
                        out_tb,
                    );
                    let _ = pkt.write_interleaved(octx);
                    written += 1;
                }
            }
        }
        offset += chunk;
    }
    written
}

// ── fMP4 muxer helper ────────────────────────────────────────────────────────

/// movflags value for fragmented MP4 / CMAF output.
///
/// - `empty_moov`  — write ftyp+moov with no sample tables (init segment).
/// - `frag_keyframe` — start a new moof at each keyframe.
/// - `default_base_moof` — make every moof self-contained (DASH/CMAF required).
const FMP4_MOVFLAGS: &str = "empty_moov+frag_keyframe+default_base_moof";

/// Create an fMP4 (CMAF) output context with `movflags` applied.
///
/// Uses `av_opt_set` on the `AVFormatContext` with `AV_OPT_SEARCH_CHILDREN`
/// to set movflags on the mp4 muxer's private data (MOVMuxContext).
/// This is the same pattern used by production Rust FFmpeg projects
/// (e.g. lite-nvr, video-rs).
fn create_fmp4_output(tmp_path: &Path) -> Result<ffmpeg_next::format::context::Output, String> {
    let mut octx = ffmpeg_next::format::output_as(tmp_path, "mp4")
        .map_err(|e| format!("output context: {e}"))?;

    unsafe {
        let key = std::ffi::CString::new("movflags").unwrap();
        let val = std::ffi::CString::new(FMP4_MOVFLAGS).unwrap();

        let ret = ffmpeg_next::ffi::av_opt_set(
            octx.as_mut_ptr() as *mut std::ffi::c_void,
            key.as_ptr(),
            val.as_ptr(),
            ffmpeg_next::ffi::AV_OPT_SEARCH_CHILDREN as i32,
        );
        if ret < 0 {
            return Err(format!(
                "av_opt_set movflags failed (ret={ret}) — mp4 muxer not available?"
            ));
        }
    }

    Ok(octx)
}

/// Write the fMP4 header via raw FFI.
///
/// Uses `avformat_write_header` directly instead of the safe `write_header()`
/// wrapper because the latter treats return code 1 (`AVSTREAM_INIT_IN_WRITE_HEADER`)
/// as an error.  Return code >= 0 means success.
fn write_fmp4_header(octx: &mut ffmpeg_next::format::context::Output) -> Result<(), String> {
    unsafe {
        let ret = ffmpeg_next::ffi::avformat_write_header(
            octx.as_mut_ptr(),
            std::ptr::null_mut(),
        );
        if ret < 0 {
            return Err(format!("write header (fMP4): error {ret}"));
        }
        Ok(())
    }
}

/// Write the fMP4 trailer via raw FFI.
///
/// Uses `av_write_trailer` directly instead of the safe `write_trailer()`
/// wrapper for consistency with [`write_fmp4_header`].
fn write_fmp4_trailer(octx: &mut ffmpeg_next::format::context::Output) -> Result<(), String> {
    unsafe {
        let ret = ffmpeg_next::ffi::av_write_trailer(octx.as_mut_ptr());
        if ret < 0 {
            return Err(format!("write trailer (fMP4): error {ret}"));
        }
        Ok(())
    }
}

// ── Init segment extraction ──────────────────────────────────────────────────

/// Extract the fMP4 init segment (ftyp + moov atoms) from a source video.
///
/// The init segment contains the codec configuration (SPS/PPS for H.264,
/// channel layout for AAC) that the browser's MSE SourceBuffer needs before
/// any media segments can be appended.
///
/// **Always** extracts ftyp+moov from a real segment 0 rather than creating
/// a header-only fMP4.  This guarantees the moov's track timescale matches
/// exactly what the media segments' moof boxes use.  A header-only mux can
/// produce a different timescale (ffmpeg's mp4 muxer adjusts timescale
/// during write_header based on actual packet data), causing the browser to
/// misinterpret baseMediaDecodeTime / sample durations and play at the wrong
/// speed.  Extracting from a real segment is the approach used by dash.js's
/// test content generator and by Shaka Packager.
pub fn create_init_segment(abs_path: &str, quality: Quality, hwaccel: &HwAccel) -> Result<Vec<u8>, String> {
    super::ensure_init();

    // Generate segment 0 and extract ftyp+moov from it.
    // This ensures the init segment's codec params AND timescale match
    // the media segments exactly — critical for correct MSE playback
    // timing in both Segments and Sequence SourceBuffer modes.
    //
    // Use a unique temp directory per call (thread ID + timestamp) to
    // avoid races when concurrent requests create the init for the same
    // video simultaneously.
    let unique = format!(
        "starfin_init_{}_{:?}",
        std::process::id(),
        std::thread::current().id(),
    );
    let tmp_dir = std::env::temp_dir().join(unique);
    let _ = std::fs::create_dir_all(&tmp_dir);

    let result = (|| -> Result<Vec<u8>, String> {
        create_segment(abs_path, &tmp_dir, 0, hwaccel, quality, None)?;
        let seg0_path = tmp_dir.join("seg_00000.m4s");
        let data = std::fs::read(&seg0_path)
            .map_err(|e| format!("failed to read segment 0: {e}"))?;
        extract_ftyp_moov(&data)
    })();

    // Cleanup regardless of success or failure.
    let _ = std::fs::remove_dir_all(&tmp_dir);

    result
}

/// Extract ftyp and moov boxes from an fMP4 byte buffer.
///
/// Parses MP4 box headers (4-byte size + 4-byte type) and copies only the
/// ftyp and moov boxes, discarding any moof/mdat/other boxes.
fn extract_ftyp_moov(data: &[u8]) -> Result<Vec<u8>, String> {
    let mut result = Vec::new();
    let mut pos = 0usize;

    while pos + 8 <= data.len() {
        let size = u32::from_be_bytes([data[pos], data[pos + 1], data[pos + 2], data[pos + 3]]) as usize;
        if size < 8 || pos + size > data.len() {
            break;
        }
        let box_type = &data[pos + 4..pos + 8];
        if box_type == b"ftyp" || box_type == b"moov" {
            result.extend_from_slice(&data[pos..pos + size]);
        }
        pos += size;
    }

    if result.is_empty() {
        return Err("no ftyp/moov boxes found in fMP4 data".into());
    }

    Ok(result)
}

// ── tfdt patching ────────────────────────────────────────────────────────────

/// Patch the `baseMediaDecodeTime` in all `tfdt` boxes within an fMP4 segment
/// so that each segment's samples appear at the correct absolute position on
/// the presentation timeline.
///
/// FFmpeg's fragmented MP4 muxer always normalizes the first DTS to 0
/// (in `mov_write_single_packet`), so `baseMediaDecodeTime` in the `tfdt`
/// boxes is always 0 regardless of the PTS values written to the packets.
/// This post-processing step restores the correct absolute timeline position
/// required for MSE Segments mode (used by dash.js and Shaka Player).
///
/// `start_time_secs` is the actual presentation time (in seconds) where this
/// segment's content should begin.  For the remux/hybrid paths this is the
/// PTS of the first video keyframe; for the transcode path it equals
/// `seg_index × SEGMENT_DURATION` because the encoder forces a keyframe at
/// the segment boundary.
///
/// The function:
///   1. Parses the `moov` box to build a track_id → timescale map.
///   2. Walks each `traf` inside the `moof` box, reads the `track_id`
///      from `tfhd`, and patches the `tfdt` with
///      `start_time_secs × timescale`.
fn patch_segment_tfdt(path: &Path, start_time_secs: f64) -> Result<(), String> {
    if start_time_secs < 0.001 {
        return Ok(()); // First segment starts at time 0 — no patch needed.
    }

    let mut data = std::fs::read(path)
        .map_err(|e| format!("patch_tfdt: read segment: {e}"))?;

    // Step 1: Parse moov to build track_id -> timescale map.
    let timescales = parse_moov_timescales(&data)?;

    // Step 2: Find the moof box and patch tfdt in each traf.
    let mut pos = 0usize;
    while pos + 8 <= data.len() {
        let size = u32::from_be_bytes([data[pos], data[pos + 1], data[pos + 2], data[pos + 3]])
            as usize;
        if size < 8 || pos + size > data.len() {
            break;
        }
        if &data[pos + 4..pos + 8] == b"moof" {
            patch_moof_tfdts(&mut data, pos + 8, pos + size, start_time_secs, &timescales);
            break; // Only one moof per segment.
        }
        pos += size;
    }

    std::fs::write(path, &data)
        .map_err(|e| format!("patch_tfdt: write segment: {e}"))?;

    Ok(())
}

/// Parse the `moov` box to extract a track_id → timescale mapping.
///
/// Walks `moov → trak → tkhd` (for `track_id`) and
/// `moov → trak → mdia → mdhd` (for `timescale`).
fn parse_moov_timescales(data: &[u8]) -> Result<std::collections::HashMap<u32, u32>, String> {
    use std::collections::HashMap;
    let mut timescales = HashMap::new();

    // Find moov box.
    let mut pos = 0usize;
    while pos + 8 <= data.len() {
        let size = u32::from_be_bytes([data[pos], data[pos + 1], data[pos + 2], data[pos + 3]])
            as usize;
        if size < 8 || pos + size > data.len() {
            break;
        }
        if &data[pos + 4..pos + 8] == b"moov" {
            let moov_end = pos + size;
            let mut child = pos + 8;
            while child + 8 <= moov_end {
                let cs =
                    u32::from_be_bytes([data[child], data[child + 1], data[child + 2], data[child + 3]])
                        as usize;
                if cs < 8 || child + cs > moov_end {
                    break;
                }
                if &data[child + 4..child + 8] == b"trak" {
                    if let Some((tid, ts)) = parse_trak_timescale(data, child + 8, child + cs) {
                        timescales.insert(tid, ts);
                    }
                }
                child += cs;
            }
            break;
        }
        pos += size;
    }

    if timescales.is_empty() {
        return Err("patch_tfdt: no tracks found in moov".into());
    }
    Ok(timescales)
}

/// Extract `(track_id, timescale)` from a single `trak` box's children.
fn parse_trak_timescale(data: &[u8], start: usize, end: usize) -> Option<(u32, u32)> {
    let mut track_id: Option<u32> = None;
    let mut timescale: Option<u32> = None;

    let mut pos = start;
    while pos + 8 <= end {
        let size = u32::from_be_bytes([data[pos], data[pos + 1], data[pos + 2], data[pos + 3]])
            as usize;
        if size < 8 || pos + size > end {
            break;
        }
        let btype = &data[pos + 4..pos + 8];

        if btype == b"tkhd" && size >= 24 {
            let version = data[pos + 8];
            let offset = if version == 0 { 20 } else { 28 };
            if pos + offset + 4 <= data.len() {
                track_id = Some(u32::from_be_bytes([
                    data[pos + offset],
                    data[pos + offset + 1],
                    data[pos + offset + 2],
                    data[pos + offset + 3],
                ]));
            }
        } else if btype == b"mdia" {
            // Walk mdia children to find mdhd.
            let mdia_end = pos + size;
            let mut m = pos + 8;
            while m + 8 <= mdia_end {
                let ms =
                    u32::from_be_bytes([data[m], data[m + 1], data[m + 2], data[m + 3]]) as usize;
                if ms < 8 || m + ms > mdia_end {
                    break;
                }
                if &data[m + 4..m + 8] == b"mdhd" && ms >= 24 {
                    let version = data[m + 8];
                    let offset = if version == 0 { 20 } else { 28 };
                    if m + offset + 4 <= data.len() {
                        timescale = Some(u32::from_be_bytes([
                            data[m + offset],
                            data[m + offset + 1],
                            data[m + offset + 2],
                            data[m + offset + 3],
                        ]));
                    }
                    break;
                }
                m += ms;
            }
        }
        pos += size;
    }

    match (track_id, timescale) {
        (Some(t), Some(s)) => Some((t, s)),
        _ => None,
    }
}

/// Walk all `traf` children inside a `moof` box and patch each `tfdt`.
fn patch_moof_tfdts(
    data: &mut [u8],
    start: usize,
    end: usize,
    start_time_secs: f64,
    timescales: &std::collections::HashMap<u32, u32>,
) {
    let mut pos = start;
    while pos + 8 <= end {
        let size = u32::from_be_bytes([data[pos], data[pos + 1], data[pos + 2], data[pos + 3]])
            as usize;
        if size < 8 || pos + size > end {
            break;
        }
        if &data[pos + 4..pos + 8] == b"traf" {
            patch_traf_tfdt(data, pos + 8, pos + size, start_time_secs, timescales);
        }
        pos += size;
    }
}

/// Inside a single `traf`, find the `tfhd` (for `track_id`) and `tfdt`
/// (for `baseMediaDecodeTime`) and overwrite the latter.
fn patch_traf_tfdt(
    data: &mut [u8],
    start: usize,
    end: usize,
    start_time_secs: f64,
    timescales: &std::collections::HashMap<u32, u32>,
) {
    let mut track_id: Option<u32> = None;
    let mut tfdt_pos: Option<usize> = None;
    let mut tfdt_version: u8 = 0;

    let mut pos = start;
    while pos + 8 <= end {
        let size = u32::from_be_bytes([data[pos], data[pos + 1], data[pos + 2], data[pos + 3]])
            as usize;
        if size < 8 || pos + size > end {
            break;
        }
        let btype = &data[pos + 4..pos + 8];

        if btype == b"tfhd" && size >= 16 {
            // tfhd: size(4) + type(4) + version(1) + flags(3) + track_id(4)
            track_id = Some(u32::from_be_bytes([
                data[pos + 12],
                data[pos + 13],
                data[pos + 14],
                data[pos + 15],
            ]));
        } else if btype == b"tfdt" {
            tfdt_version = data[pos + 8];
            tfdt_pos = Some(pos);
        }
        pos += size;
    }

    if let (Some(tid), Some(tp)) = (track_id, tfdt_pos) {
        if let Some(&ts) = timescales.get(&tid) {
            // SEGMENT_DURATION is an integer number of seconds (6) and
            // timescale fits in u32, so integer arithmetic is exact here.
            let bdt = (start_time_secs * ts as f64).round() as u64;
            if tfdt_version == 0 {
                // 32-bit baseMediaDecodeTime at box_start + 12
                let val = (bdt as u32).to_be_bytes();
                data[tp + 12..tp + 16].copy_from_slice(&val);
            } else {
                // 64-bit baseMediaDecodeTime at box_start + 12
                let val = bdt.to_be_bytes();
                data[tp + 12..tp + 20].copy_from_slice(&val);
            }
        }
    }
}

// ── Segment creation: decide remux vs transcode ─────────────────────────────

fn create_segment(
    abs_path: &str,
    seg_dir: &Path,
    seg_index: usize,
    hwaccel: &HwAccel,
    quality: Quality,
    kill: Option<&AtomicBool>,
) -> Result<(), String> {
    super::ensure_init();

    let start_time = seg_index as f64 * SEGMENT_DURATION;
    let tmp_filename = format!(".seg_{:05}.m4s.tmp", seg_index);
    let tmp_path = seg_dir.join(&tmp_filename);
    let filename = format!("seg_{:05}.m4s", seg_index);
    let seg_path = seg_dir.join(&filename);

    // Open input.
    let mut ictx = ffmpeg_next::format::input(&abs_path)
        .map_err(|e| format!("failed to open input: {e}"))?;

    // Decide: remux (fast copy) or transcode (re-encode).
    // The remux and hybrid paths return the actual PTS (in seconds) of the
    // first video keyframe so that patch_segment_tfdt can place the segment
    // at its true position on the presentation timeline.  The transcode path
    // forces a keyframe at the segment boundary, so start_time is exact.
    let actual_start = if quality.can_remux() && source_is_remuxable(&ictx) {
        // Pure remux — both video and audio packets copied directly.
        remux_segment(&mut ictx, start_time, &tmp_path, kill)?
    } else if quality.can_remux() && video_is_remuxable(&ictx) {
        // Hybrid — video packets copied, audio transcoded to stereo AAC.
        // This handles multi-channel AAC, non-AAC/MP3 codecs, etc.
        hybrid_segment(&mut ictx, start_time, &tmp_path, kill)?
    } else {
        // For Original quality with incompatible codecs, fall back to the
        // same settings as High (native resolution, best quality).
        let effective_quality = if quality == Quality::Original { Quality::High } else { quality };
        transcode_segment_inprocess(&mut ictx, start_time, hwaccel, effective_quality, &tmp_path, kill)?;
        start_time
    };

    // Patch tfdt baseMediaDecodeTime so each segment is positioned at the
    // correct absolute time on the presentation timeline.  FFmpeg's fMP4
    // muxer always normalizes DTS to 0, so without this patch all segments
    // would overlap at time 0 and MSE Segments mode would show only ~6 s.
    patch_segment_tfdt(&tmp_path, actual_start)?;

    // Atomic rename.
    std::fs::rename(&tmp_path, &seg_path)
        .map_err(|e| format!("failed to rename segment {seg_index}: {e}"))?;

    Ok(())
}

// ── Remux path (direct packet copy — near-instant) ──────────────────────────

/// Copy compressed packets from the source into an fMP4 segment without
/// decoding or re-encoding.  This is the equivalent of
/// `ffmpeg -ss <t> -i input -t 6 -c copy -f mp4 -movflags empty_moov+frag_keyframe+default_base_moof output.m4s`
/// and gives VLC-like performance.
///
/// Returns the actual PTS (in seconds) of the first video keyframe included
/// in the segment.  This may be later than `start_time` when the source
/// keyframe interval doesn't align with the segment duration.
fn remux_segment(
    ictx: &mut ffmpeg_next::format::context::Input,
    start_time: f64,
    tmp_path: &Path,
    kill: Option<&AtomicBool>,
) -> Result<f64, String> {
    let end_time = start_time + SEGMENT_DURATION;

    // Find best video/audio streams.
    let video_idx = ictx
        .streams()
        .best(ffmpeg_next::media::Type::Video)
        .map(|s| s.index());
    let audio_idx = ictx
        .streams()
        .best(ffmpeg_next::media::Type::Audio)
        .map(|s| s.index());

    if video_idx.is_none() {
        return Err("no video stream found".into());
    }

    // Seek to the nearest keyframe at or before start_time.
    let seek_ts = (start_time * f64::from(ffmpeg_next::ffi::AV_TIME_BASE)) as i64;
    let _ = ictx.seek(seek_ts, ..seek_ts);

    // Collect input stream info we need (time_bases, codec params).
    // We only copy the best video and (optionally) best audio stream.
    let in_video_tb = ictx.stream(video_idx.unwrap()).unwrap().time_base();
    let in_video_params = ictx.stream(video_idx.unwrap()).unwrap().parameters();

    let in_audio_tb;
    let in_audio_params;
    if let Some(ai) = audio_idx {
        in_audio_tb = Some(ictx.stream(ai).unwrap().time_base());
        in_audio_params = Some(ictx.stream(ai).unwrap().parameters());
    } else {
        in_audio_tb = None;
        in_audio_params = None;
    }

    // Create output muxer — fMP4 for DASH.
    let mut octx = create_fmp4_output(tmp_path)?;

    // Add output video stream, copying codec parameters from input.
    let out_video = octx.add_stream(ffmpeg_next::encoder::find(ffmpeg_next::codec::Id::H264))
        .map_err(|e| format!("add video stream: {e}"))?;
    unsafe {
        ffmpeg_next::ffi::avcodec_parameters_copy(
            out_video.parameters().as_mut_ptr(),
            in_video_params.as_ptr(),
        );
    }
    let out_video_idx = out_video.index();

    // Add output audio stream if present.
    let mut out_audio_idx_val: Option<usize> = None;
    let mut out_audio_tb = ffmpeg_next::Rational::new(1, 90000);

    if let Some(ref a_params) = in_audio_params {
        let audio_codec_id = a_params.id();
        let enc = ffmpeg_next::encoder::find(audio_codec_id);
        match octx.add_stream(enc) {
            Ok(out_audio) => {
                unsafe {
                    ffmpeg_next::ffi::avcodec_parameters_copy(
                        out_audio.parameters().as_mut_ptr(),
                        a_params.as_ptr(),
                    );
                }
                out_audio_tb = out_audio.time_base();
                out_audio_idx_val = Some(out_audio.index());
            }
            Err(e) => {
                eprintln!("[remux] failed to add audio stream (codec {:?}): {e}", audio_codec_id);
            }
        }
    }

    write_fmp4_header(&mut octx)?;

    // Re-read output time bases after write_header (muxer may adjust them).
    let out_video_tb = octx.stream(out_video_idx).unwrap().time_base();
    let out_audio_tb = out_audio_idx_val.map(|i| octx.stream(i).unwrap().time_base()).unwrap_or(out_audio_tb);

    let mut got_video_keyframe = false;
    // Actual PTS (in seconds) of the first video keyframe in this segment.
    let mut keyframe_pts_secs: f64 = start_time;
    // PTS offsets (in output time base) used to rebase timestamps so that
    // first-packet PTS is subtracted, then a segment-start offset is added,
    // producing continuous PTS across the presentation (seg N starts at
    // N × 6s).  This allows the browser to use MSE Segments mode for
    // random-access seeking per DASH-IF IOP v4.3 §3.2.
    let mut video_pts_offset: Option<i64> = None;
    let mut audio_pts_offset: Option<i64> = None;

    // Segment-start offsets in each output time base so PTS is continuous.
    let seg_video_start = (start_time * out_video_tb.1 as f64 / out_video_tb.0 as f64) as i64;
    let seg_audio_start = (start_time * out_audio_tb.1 as f64 / out_audio_tb.0 as f64) as i64;

    for (stream, mut packet) in ictx.packets() {
        if let Some(k) = kill {
            if k.load(Ordering::Relaxed) {
                let _ = std::fs::remove_file(tmp_path);
                return Err(CANCELLED.into());
            }
        }
        let si = stream.index();
        let is_video = Some(si) == video_idx;
        let is_audio = Some(si) == audio_idx;

        if !is_video && !is_audio {
            continue;
        }

        // Convert packet PTS to seconds for time-range filtering.
        let in_tb = if is_video { in_video_tb } else { in_audio_tb.unwrap_or(in_video_tb) };
        let pts = packet.pts().unwrap_or(0);
        let pts_secs = pts as f64 * f64::from(in_tb.0) / f64::from(in_tb.1);

        // Past segment end → done.
        if pts_secs >= end_time {
            break;
        }

        if is_video {
            // Wait for the first keyframe at or after start_time.
            // Only accept keyframes whose PTS is at or after the declared
            // segment start so consecutive segments never share overlapping
            // content.  Accepting a keyframe from *before* start_time would
            // cause the player to replay already-seen frames at every segment
            // boundary, producing the "stutter every 6 seconds" symptom.
            if !got_video_keyframe {
                if !packet.is_key() || pts_secs < start_time {
                    continue;
                }
                got_video_keyframe = true;
                keyframe_pts_secs = pts_secs;
            }
        } else {
            // Skip audio packets before the video keyframe region.
            if !got_video_keyframe {
                continue;
            }
        }

        // Map to the output stream and rescale timestamps.
        if is_video {
            packet.set_stream(out_video_idx);
            packet.rescale_ts(in_video_tb, out_video_tb);
            // Record the first video DTS (or PTS) as the offset to rebase
            // to zero, then add the segment-start offset for continuous PTS.
            // Using DTS prevents negative DTS values for B-frame content
            // where DTS < PTS on the first packet.
            if video_pts_offset.is_none() {
                video_pts_offset = packet.dts().or(packet.pts());
            }
            if let (Some(offset), Some(p)) = (video_pts_offset, packet.pts()) {
                packet.set_pts(Some(p - offset + seg_video_start));
            }
            if let (Some(offset), Some(d)) = (video_pts_offset, packet.dts()) {
                packet.set_dts(Some(d - offset + seg_video_start));
            }
        } else if let Some(out_ai) = out_audio_idx_val {
            packet.set_stream(out_ai);
            packet.rescale_ts(in_audio_tb.unwrap(), out_audio_tb);
            // Same rebase + segment-start offset for audio.
            if audio_pts_offset.is_none() {
                audio_pts_offset = packet.dts().or(packet.pts());
            }
            if let (Some(offset), Some(p)) = (audio_pts_offset, packet.pts()) {
                packet.set_pts(Some(p - offset + seg_audio_start));
            }
            if let (Some(offset), Some(d)) = (audio_pts_offset, packet.dts()) {
                packet.set_dts(Some(d - offset + seg_audio_start));
            }
        } else {
            continue;
        }

        let _ = packet.write_interleaved(&mut octx);
    }

    write_fmp4_trailer(&mut octx)?;

    Ok(keyframe_pts_secs)
}

// ── Hybrid path (video remux + audio transcode to stereo AAC) ───────────────

/// Copy video packets directly while decoding and re-encoding the audio
/// stream to stereo AAC.  This is used when the video is H.264 (browser-
/// compatible) but the audio cannot be remuxed — typically because it is
/// multi-channel (5.1/7.1) AAC which browsers don't support.
///
/// Gives the best quality+speed trade-off: lossless video copy (like remux)
/// with only lightweight audio transcoding.
fn hybrid_segment(
    ictx: &mut ffmpeg_next::format::context::Input,
    start_time: f64,
    tmp_path: &Path,
    kill: Option<&AtomicBool>,
) -> Result<f64, String> {
    let end_time = start_time + SEGMENT_DURATION;

    // Find best video/audio streams.
    let video_idx = ictx
        .streams()
        .best(ffmpeg_next::media::Type::Video)
        .map(|s| s.index());
    let audio_idx = ictx
        .streams()
        .best(ffmpeg_next::media::Type::Audio)
        .map(|s| s.index());

    if video_idx.is_none() {
        return Err("no video stream found".into());
    }

    // Seek to the nearest keyframe at or before start_time.
    let seek_ts = (start_time * f64::from(ffmpeg_next::ffi::AV_TIME_BASE)) as i64;
    let _ = ictx.seek(seek_ts, ..seek_ts);

    // Collect input video stream info.
    let in_video_tb = ictx.stream(video_idx.unwrap()).unwrap().time_base();
    let in_video_params = ictx.stream(video_idx.unwrap()).unwrap().parameters();

    // Collect input audio stream info.
    let in_audio_tb;
    let in_audio_params;
    if let Some(ai) = audio_idx {
        in_audio_tb = Some(ictx.stream(ai).unwrap().time_base());
        in_audio_params = Some(ictx.stream(ai).unwrap().parameters());
    } else {
        in_audio_tb = None;
        in_audio_params = None;
    }

    // Create output muxer — fMP4 for DASH.
    let mut octx = create_fmp4_output(tmp_path)?;

    // Add output video stream — parameters copied directly from input.
    let out_video = octx.add_stream(ffmpeg_next::encoder::find(ffmpeg_next::codec::Id::H264))
        .map_err(|e| format!("add video stream: {e}"))?;
    unsafe {
        ffmpeg_next::ffi::avcodec_parameters_copy(
            out_video.parameters().as_mut_ptr(),
            in_video_params.as_ptr(),
        );
    }
    let out_video_idx = out_video.index();

    // ── Audio decoder + encoder setup ──────────────────────────────────
    // The resampler is created lazily on the first decoded frame so it
    // uses the frame's actual format/layout — not the decoder's pre-decode
    // guess which often mismatches (e.g. default(6) gives "5.1(side)" but
    // the decoded frame carries "5.1" without "(side)").
    let mut audio_decoder: Option<ffmpeg_next::decoder::Audio> = None;
    let mut audio_encoder_handle: Option<ffmpeg_next::encoder::Audio> = None;
    let mut audio_resampler: Option<ffmpeg_next::software::resampling::Context> = None;
    let mut out_audio_idx: Option<usize> = None;
    let mut audio_sample_rate: u32 = 48000;
    let mut audio_time_base = ffmpeg_next::Rational::new(1, 48000);
    // Saved encoder output format/layout so we can create the resampler lazily.
    let mut enc_format_saved: Option<ffmpeg_next::format::Sample> = None;
    let mut enc_layout_saved: Option<ffmpeg_next::channel_layout::ChannelLayout> = None;

    if let Some(ref a_params) = in_audio_params {
        let aud_idx = audio_idx.unwrap();
        audio_time_base = in_audio_tb.unwrap();

        match ffmpeg_next::codec::context::Context::from_parameters(a_params.clone()) {
            Ok(aud_ctx) => match aud_ctx.decoder().audio() {
                Ok(dec) => {
                    audio_sample_rate = dec.rate();
                    let raw_dec_layout = dec.channel_layout();

                    // Determine output channel count from the decoder's
                    // pre-decode info (enough for MONO vs STEREO decision).
                    let approx_channels = if raw_dec_layout.channels() > 0 {
                        raw_dec_layout.channels()
                    } else if dec.channels() > 0 {
                        dec.channels() as i32
                    } else {
                        2
                    };

                    // Always output MONO or STEREO for browser compatibility.
                    let enc_layout = if approx_channels == 1 {
                        ffmpeg_next::channel_layout::ChannelLayout::MONO
                    } else {
                        ffmpeg_next::channel_layout::ChannelLayout::STEREO
                    };
                    let enc_format = ffmpeg_next::format::Sample::F32(
                        ffmpeg_next::format::sample::Type::Planar,
                    );

                    let aac_codec = ffmpeg_next::encoder::find_by_name("aac");
                    if let Some(aac) = aac_codec {
                        let aac_ctx = ffmpeg_next::codec::context::Context::new_with_codec(aac);
                        match aac_ctx.encoder().audio() {
                            Ok(mut aac_enc) => {
                                aac_enc.set_rate(dec.rate() as i32);
                                aac_enc.set_channel_layout(enc_layout);
                                aac_enc.set_format(enc_format);
                                aac_enc.set_bit_rate(AAC_ENCODE_BITRATE);
                                aac_enc.set_time_base(ffmpeg_next::Rational::new(1, dec.rate() as i32));

                                match aac_enc.open_as(aac) {
                                    Ok(opened) => {
                                        let mut out_aud_stream = octx.add_stream(aac)
                                            .map_err(|e| format!("add audio stream: {e}"))?;
                                        out_aud_stream.set_parameters(&opened);
                                        out_audio_idx = Some(out_aud_stream.index());

                                        // Save for lazy resampler creation.
                                        enc_format_saved = Some(enc_format);
                                        enc_layout_saved = Some(enc_layout);
                                        audio_encoder_handle = Some(opened);
                                        audio_decoder = Some(dec);
                                    }
                                    Err(e) => {
                                        eprintln!(
                                            "[hybrid] failed to open AAC encoder (channels={}, layout={:?}): {e}",
                                            enc_layout.channels(), enc_layout
                                        );
                                    }
                                }
                            }
                            Err(e) => {
                                eprintln!("[hybrid] failed to init AAC encoder context for stream {aud_idx}: {e}");
                            }
                        }
                    }
                }
                Err(e) => {
                    eprintln!("[hybrid] failed to open audio decoder for stream {aud_idx}: {e}");
                }
            },
            Err(e) => {
                eprintln!("[hybrid] failed to create audio decoder context for stream {aud_idx}: {e}");
            }
        }
    }

    write_fmp4_header(&mut octx)?;

    // Re-read output time bases after write_header (muxer may adjust them).
    let out_video_tb = octx.stream(out_video_idx).unwrap().time_base();
    let out_audio_tb = out_audio_idx.map(|i| octx.stream(i).unwrap().time_base());

    let mut got_video_keyframe = false;
    let mut keyframe_pts_secs: f64 = start_time;
    let mut video_pts_offset: Option<i64> = None;

    // Segment-start offset for video in the output time base so PTS is
    // continuous across segments (matches the audio_ts_offset below).
    let seg_video_start = (start_time * out_video_tb.1 as f64 / out_video_tb.0 as f64) as i64;

    // Audio synthetic PTS (in 1/sample_rate time base).
    let mut audio_sample_count: i64 = 0;
    let audio_ts_offset = (start_time * audio_sample_rate as f64) as i64;

    for (stream, mut packet) in ictx.packets() {
        if let Some(k) = kill {
            if k.load(Ordering::Relaxed) {
                let _ = std::fs::remove_file(tmp_path);
                return Err(CANCELLED.into());
            }
        }
        let si = stream.index();
        let is_video = Some(si) == video_idx;
        let is_audio = Some(si) == audio_idx;

        if !is_video && !is_audio {
            continue;
        }

        // Convert packet PTS to seconds for time-range filtering.
        let in_tb = if is_video { in_video_tb } else { in_audio_tb.unwrap_or(in_video_tb) };
        let pts = packet.pts().unwrap_or(0);
        let pts_secs = pts as f64 * f64::from(in_tb.0) / f64::from(in_tb.1);

        // Past segment end → done.
        if pts_secs >= end_time {
            break;
        }

        if is_video {
            // Wait for the first keyframe at or after start_time.
            // See the same comment in remux_segment — accepting a keyframe
            // from before start_time causes content overlap between segments
            // and the resulting duplicate frames appear as stuttering.
            if !got_video_keyframe {
                if !packet.is_key() || pts_secs < start_time {
                    continue;
                }
                got_video_keyframe = true;
                keyframe_pts_secs = pts_secs;
            }

            // Copy video packet directly (remux), adding continuous PTS.
            packet.set_stream(out_video_idx);
            packet.rescale_ts(in_video_tb, out_video_tb);
            if video_pts_offset.is_none() {
                video_pts_offset = packet.dts().or(packet.pts());
            }
            if let (Some(offset), Some(p)) = (video_pts_offset, packet.pts()) {
                packet.set_pts(Some(p - offset + seg_video_start));
            }
            if let (Some(offset), Some(d)) = (video_pts_offset, packet.dts()) {
                packet.set_dts(Some(d - offset + seg_video_start));
            }
            let _ = packet.write_interleaved(&mut octx);
        } else if is_audio {
            // Skip audio before the video keyframe.
            if !got_video_keyframe {
                continue;
            }

            // Decode → resample → encode audio to stereo AAC.
            if let (Some(adec), Some(aenc)) =
                (&mut audio_decoder, &mut audio_encoder_handle)
            {
                match adec.send_packet(&packet) {
                    Ok(()) => {},
                    Err(e) => {
                        eprintln!("[hybrid] send_packet error: {e}");
                        continue;
                    }
                }
                let mut audio_frame = ffmpeg_next::util::frame::Audio::empty();
                while adec.receive_frame(&mut audio_frame).is_ok() {
                    // Time-range filter.
                    if let Some(apts) = audio_frame.pts() {
                        let apts_secs = apts as f64
                            * f64::from(audio_time_base.0)
                            / f64::from(audio_time_base.1);
                        if apts_secs < start_time || apts_secs >= end_time {
                            continue;
                        }
                    }

                    // Ensure the decoded frame has a valid channel layout
                    // so the resampler doesn't reject it as "Input changed".
                    ensure_frame_channel_layout(&mut audio_frame);

                    // Lazy resampler creation: use the first decoded frame's
                    // actual format and channel layout so there's no mismatch.
                    if audio_resampler.is_none() {
                        if let (Some(ef), Some(el)) = (enc_format_saved, enc_layout_saved) {
                            let frame_layout = frame_channel_layout(&audio_frame);
                            let frame_format = audio_frame.format();
                            let frame_rate = if audio_frame.rate() > 0 {
                                audio_frame.rate()
                            } else {
                                audio_sample_rate
                            };
                            match ffmpeg_next::software::resampling::Context::get(
                                frame_format,
                                frame_layout,
                                frame_rate,
                                ef,
                                el,
                                frame_rate,
                            ) {
                                Ok(r) => {
                                    audio_resampler = Some(r);
                                }
                                Err(e) => {
                                    eprintln!(
                                        "[hybrid] failed to create resampler from decoded frame \
                                         (fmt={:?} ch={} rate={}): {e}",
                                        frame_format, frame_layout.channels(), frame_rate
                                    );
                                }
                            }
                        }
                    }

                    let frame_to_encode = if let Some(ref mut resampler) = audio_resampler {
                        let mut resampled = ffmpeg_next::frame::Audio::empty();
                        if resampler.run(&audio_frame, &mut resampled).is_err() {
                            continue;
                        }
                        resampled
                    } else {
                        audio_frame.clone()
                    };

                    if let Some(aud_out_idx) = out_audio_idx {
                        encode_audio_frame(
                            &frame_to_encode,
                            aenc,
                            &mut octx,
                            aud_out_idx,
                            audio_sample_rate,
                            out_audio_tb.unwrap_or(ffmpeg_next::Rational::new(1, 90000)),
                            &mut audio_sample_count,
                            audio_ts_offset,
                        );
                    }
                }
            }
        }
    }

    // Flush audio encoder.
    if let Some(ref mut aenc) = audio_encoder_handle {
        let _ = aenc.send_eof();
        let mut aenc_pkt = ffmpeg_next::Packet::empty();
        while aenc.receive_packet(&mut aenc_pkt).is_ok() {
            if let Some(aud_out_idx) = out_audio_idx {
                aenc_pkt.set_stream(aud_out_idx);
                aenc_pkt.rescale_ts(
                    ffmpeg_next::Rational::new(1, audio_sample_rate as i32),
                    out_audio_tb.unwrap_or(ffmpeg_next::Rational::new(1, 90000)),
                );
                let _ = aenc_pkt.write_interleaved(&mut octx);
            }
        }
    }

    write_fmp4_trailer(&mut octx)?;

    Ok(keyframe_pts_secs)
}

// ── Transcode path (re-encode — used for incompatible codecs or scaling) ────

fn transcode_segment_inprocess(
    ictx: &mut ffmpeg_next::format::context::Input,
    start_time: f64,
    hwaccel: &HwAccel,
    quality: Quality,
    tmp_path: &Path,
    kill: Option<&AtomicBool>,
) -> Result<(), String> {
    // Seek to the segment start position.
    let seek_ts = (start_time * f64::from(ffmpeg_next::ffi::AV_TIME_BASE)) as i64;
    let _ = ictx.seek(seek_ts, ..seek_ts);

    // Find video and audio streams.
    let video_stream_idx = ictx
        .streams()
        .best(ffmpeg_next::media::Type::Video)
        .map(|s| s.index());
    let audio_stream_idx = ictx
        .streams()
        .best(ffmpeg_next::media::Type::Audio)
        .map(|s| s.index());

    if video_stream_idx.is_none() {
        return Err("no video stream found".into());
    }
    let video_idx = video_stream_idx.unwrap();

    // Get video stream parameters.
    let video_stream = ictx.stream(video_idx).unwrap();
    let video_time_base = video_stream.time_base();
    let video_params = video_stream.parameters();

    // Determine output dimensions and frame rate.
    let (in_width, in_height, frame_rate) = unsafe {
        let p = video_params.as_ptr();
        let fr = (*p).framerate;
        let fps = if fr.den > 0 { fr.num as f64 / fr.den as f64 } else { 0.0 };
        ((*p).width as u32, (*p).height as u32, fps)
    };
    let effective_fps = if frame_rate > 0.0 && frame_rate.is_finite() { frame_rate } else { 30.0 };

    let (out_width, out_height, crf, preset) = match quality {
        Quality::Original | Quality::High => (in_width, in_height, "18", "veryslow"),
        Quality::Medium => {
            let max_w = 1280u32;
            if in_width <= max_w {
                (in_width, in_height, "26", "fast")
            } else {
                let ratio = max_w as f64 / in_width as f64;
                let h = ((in_height as f64 * ratio) as u32) & !1;
                (max_w, h, "26", "fast")
            }
        }
        Quality::Low => {
            let max_w = 854u32;
            if in_width <= max_w {
                (in_width, in_height, "30", "faster")
            } else {
                let ratio = max_w as f64 / in_width as f64;
                let h = ((in_height as f64 * ratio) as u32) & !1;
                (max_w, h, "30", "faster")
            }
        }
    };

    let use_hw = matches!(quality, Quality::Original | Quality::High) && *hwaccel != HwAccel::Software;
    let encoder_name = if use_hw { hwaccel.encoder() } else { "libx264" };

    // Set up video decoder.
    let video_decoder_ctx = ffmpeg_next::codec::context::Context::from_parameters(video_params)
        .map_err(|e| format!("video decoder context: {e}"))?;
    let mut video_decoder = video_decoder_ctx
        .decoder()
        .video()
        .map_err(|e| format!("video decoder: {e}"))?;

    // For hardware encoders, create device and frames contexts via FFI.
    let mut hw_device_ctx: *mut ffmpeg_next::ffi::AVBufferRef = std::ptr::null_mut();
    let mut hw_frames_ctx: *mut ffmpeg_next::ffi::AVBufferRef = std::ptr::null_mut();

    if use_hw {
        unsafe {
            let dev_type = super::hwaccel::hwdevice_type_for(hwaccel)
                .ok_or_else(|| "no hw device type for this backend".to_string())?;
            let device_path = super::hwaccel::default_device_path(hwaccel);
            hw_device_ctx = super::hwaccel::create_hw_device_ctx(dev_type, device_path.as_deref())?;
            hw_frames_ctx = super::hwaccel::create_hw_frames_ctx(
                hw_device_ctx,
                super::hwaccel::hw_pix_fmt_for(hwaccel),
                ffmpeg_next::ffi::AVPixelFormat::AV_PIX_FMT_NV12,
                out_width as i32,
                out_height as i32,
            )?;
        }
    }

    let result = transcode_segment_body(
        ictx,
        &mut video_decoder,
        video_idx,
        video_time_base,
        audio_stream_idx,
        in_width, in_height,
        out_width, out_height,
        effective_fps,
        start_time,
        use_hw,
        hw_frames_ctx,
        hwaccel,
        encoder_name,
        quality,
        preset, crf,
        tmp_path,
        kill,
    );

    unsafe {
        if !hw_frames_ctx.is_null() {
            ffmpeg_next::ffi::av_buffer_unref(&mut hw_frames_ctx);
        }
        if !hw_device_ctx.is_null() {
            ffmpeg_next::ffi::av_buffer_unref(&mut hw_device_ctx);
        }
    }

    result
}

/// Inner transcode body — decode, filter, encode, mux.
///
/// Audio handling uses a software resampler to convert the decoder's native
/// sample format to the AAC encoder's required format (FLTP), and generates
/// synthetic PTS aligned with the segment start time so that timestamps are
/// correct regardless of the source container's time base.
#[allow(clippy::too_many_arguments)]
fn transcode_segment_body(
    ictx: &mut ffmpeg_next::format::context::Input,
    video_decoder: &mut ffmpeg_next::decoder::Video,
    video_idx: usize,
    video_time_base: ffmpeg_next::Rational,
    audio_stream_idx: Option<usize>,
    in_width: u32, in_height: u32,
    out_width: u32, out_height: u32,
    effective_fps: f64,
    start_time: f64,
    use_hw: bool,
    hw_frames_ctx: *mut ffmpeg_next::ffi::AVBufferRef,
    hwaccel: &HwAccel,
    encoder_name: &str,
    quality: Quality,
    preset: &str, crf: &str,
    tmp_path: &Path,
    kill: Option<&AtomicBool>,
) -> Result<(), String> {
    // ── Video encoder setup ──────────────────────────────────────────────
    let video_encoder_codec = ffmpeg_next::encoder::find_by_name(encoder_name)
        .ok_or_else(|| format!("encoder '{}' not found", encoder_name))?;
    let video_encoder_ctx = ffmpeg_next::codec::context::Context::new_with_codec(video_encoder_codec);
    {
        let mut enc = video_encoder_ctx.encoder().video().map_err(|e| format!("video encoder setup: {e}"))?;
        enc.set_width(out_width);
        enc.set_height(out_height);
        enc.set_time_base(ffmpeg_next::Rational::new(1, 90000));
        enc.set_gop(250);
        enc.set_max_b_frames(0);

        if use_hw && !hw_frames_ctx.is_null() {
            enc.set_format(ffmpeg_next::format::Pixel::NV12);
            unsafe {
                let ctx_ptr = enc.as_mut_ptr();
                (*ctx_ptr).hw_frames_ctx = ffmpeg_next::ffi::av_buffer_ref(hw_frames_ctx);
                (*ctx_ptr).pix_fmt = super::hwaccel::hw_pix_fmt_for(hwaccel);
            }
        } else {
            enc.set_format(ffmpeg_next::format::Pixel::YUV420P);
        }

        let mut opts = ffmpeg_next::Dictionary::new();
        if use_hw {
            let quality_args = hwaccel.encoder_quality_args();
            let mut i = 0;
            while i + 1 < quality_args.len() {
                let key = quality_args[i].trim_start_matches('-');
                let val = quality_args[i + 1];
                opts.set(key, val);
                i += 2;
            }
        } else {
            opts.set("preset", preset);
            opts.set("crf", crf);
            opts.set("profile", "high");
            opts.set("level", if matches!(quality, Quality::Original | Quality::High) { "4.2" } else { "4.1" });
        }

        let mut video_encoder = enc.open_with(opts).map_err(|e| format!("open video encoder: {e}"))?;

        // ── Output muxer — fMP4 for DASH ─────────────────────────────────
        let mut octx = create_fmp4_output(tmp_path)?;

        let mut out_video_stream = octx.add_stream(video_encoder_codec)
            .map_err(|e| format!("add video stream: {e}"))?;
        out_video_stream.set_parameters(&video_encoder);
        let out_video_idx = out_video_stream.index();

        // ── Audio encoder + resampler setup ──────────────────────────────
        // The resampler is created lazily on the first decoded audio frame
        // so it uses the frame's actual format/layout (not the decoder's
        // pre-decode guess which can mismatch, e.g. "5.1(side)" vs "5.1").
        let mut audio_decoder: Option<ffmpeg_next::decoder::Audio> = None;
        let mut audio_encoder_handle: Option<ffmpeg_next::encoder::Audio> = None;
        let mut audio_resampler: Option<ffmpeg_next::software::resampling::Context> = None;
        let mut out_audio_idx: Option<usize> = None;
        let mut audio_time_base = ffmpeg_next::Rational::new(1, 44100);
        let mut audio_sample_rate: u32 = 44100;
        // Saved encoder output format/layout for lazy resampler creation.
        let mut tc_enc_format_saved: Option<ffmpeg_next::format::Sample> = None;
        let mut tc_enc_layout_saved: Option<ffmpeg_next::channel_layout::ChannelLayout> = None;

        if let Some(aud_idx) = audio_stream_idx {
            let aud_stream = ictx.stream(aud_idx).unwrap();
            audio_time_base = aud_stream.time_base();
            let aud_params = aud_stream.parameters();

            match ffmpeg_next::codec::context::Context::from_parameters(aud_params) {
                Ok(aud_ctx) => match aud_ctx.decoder().audio() {
                    Ok(dec) => {
                        audio_sample_rate = dec.rate();
                        let raw_dec_layout = dec.channel_layout();

                        // Determine approx channel count for MONO/STEREO decision.
                        let approx_channels = if raw_dec_layout.channels() > 0 {
                            raw_dec_layout.channels()
                        } else if dec.channels() > 0 {
                            dec.channels() as i32
                        } else {
                            2
                        };

                        // Downmix to stereo if source is multi-channel because
                        // the native AAC encoder only reliably supports mono and stereo.
                        let enc_layout = if approx_channels == 1 {
                            ffmpeg_next::channel_layout::ChannelLayout::MONO
                        } else {
                            ffmpeg_next::channel_layout::ChannelLayout::STEREO
                        };

                        let aac_codec = ffmpeg_next::encoder::find_by_name("aac");
                        if let Some(aac) = aac_codec {
                            let enc_format = ffmpeg_next::format::Sample::F32(
                                ffmpeg_next::format::sample::Type::Planar,
                            );
                            let aac_ctx = ffmpeg_next::codec::context::Context::new_with_codec(aac);
                            match aac_ctx.encoder().audio() {
                                Ok(mut aac_enc) => {
                                    aac_enc.set_rate(dec.rate() as i32);
                                    aac_enc.set_channel_layout(enc_layout);
                                    aac_enc.set_format(enc_format);
                                    aac_enc.set_bit_rate(AAC_ENCODE_BITRATE);
                                    aac_enc.set_time_base(ffmpeg_next::Rational::new(1, dec.rate() as i32));

                                    match aac_enc.open_as(aac) {
                                        Ok(opened) => {
                                            let mut out_aud_stream = octx.add_stream(aac)
                                                .map_err(|e| format!("add audio stream: {e}"))?;
                                            out_aud_stream.set_parameters(&opened);
                                            out_audio_idx = Some(out_aud_stream.index());

                                            // Save for lazy resampler creation.
                                            tc_enc_format_saved = Some(enc_format);
                                            tc_enc_layout_saved = Some(enc_layout);
                                            audio_encoder_handle = Some(opened);
                                            audio_decoder = Some(dec);
                                        }
                                        Err(e) => {
                                            eprintln!(
                                                "[transcode] failed to open AAC encoder (channels={}, layout={:?}): {e}",
                                                enc_layout.channels(),
                                                enc_layout
                                            );
                                        }
                                    }
                                }
                                Err(e) => {
                                    eprintln!("[transcode] failed to initialize AAC encoder context for stream {aud_idx}: {e}");
                                }
                            }
                        }
                    }
                    Err(e) => {
                        eprintln!("[transcode] failed to open audio decoder for stream {aud_idx}: {e}");
                    }
                },
                Err(e) => {
                    eprintln!("[transcode] failed to create audio decoder context for stream {aud_idx}: {e}");
                }
            }
        }

        write_fmp4_header(&mut octx)?;

        // Re-read output time bases after write_header.
        let out_video_tb = octx.stream(out_video_idx).unwrap().time_base();
        let out_audio_tb = out_audio_idx.map(|i| octx.stream(i).unwrap().time_base());

        // ── Video scaler ─────────────────────────────────────────────────
        let sw_out_fmt = if use_hw {
            ffmpeg_next::format::Pixel::NV12
        } else {
            ffmpeg_next::format::Pixel::YUV420P
        };

        let mut scaler: Option<ffmpeg_next::software::scaling::Context> = None;
        if out_width != in_width || out_height != in_height || (use_hw && !hw_frames_ctx.is_null()) {
            scaler = ffmpeg_next::software::scaling::Context::get(
                ffmpeg_next::format::Pixel::YUV420P,
                in_width,
                in_height,
                sw_out_fmt,
                out_width,
                out_height,
                ffmpeg_next::software::scaling::Flags::BILINEAR,
            )
            .ok();
        }

        // ── Main encode loop ─────────────────────────────────────────────
        let end_time = start_time + SEGMENT_DURATION;
        let ts_offset_90k = (start_time * 90000.0) as i64;
        let mut video_frame_count: i64 = 0;
        let mut audio_sample_count: i64 = 0;
        let audio_ts_offset = (start_time * audio_sample_rate as f64) as i64;
        let mut done = false;

        for (pkt_stream, packet) in ictx.packets() {
            if done { break; }
            if let Some(k) = kill {
                if k.load(Ordering::Relaxed) {
                    let _ = std::fs::remove_file(tmp_path);
                    return Err(CANCELLED.into());
                }
            }

            if pkt_stream.index() == video_idx {
                if video_decoder.send_packet(&packet).is_err() {
                    continue;
                }

                let mut decoded = ffmpeg_next::util::frame::Video::empty();
                while video_decoder.receive_frame(&mut decoded).is_ok() {
                    let pts = decoded.pts().unwrap_or(0);
                    let pts_secs = pts as f64 * f64::from(video_time_base.0) / f64::from(video_time_base.1);

                    if pts_secs >= end_time {
                        done = true;
                        break;
                    }
                    if pts_secs < start_time {
                        continue;
                    }

                    let pts_increment = (90000.0 / effective_fps) as i64;
                    let new_pts = video_frame_count * pts_increment + ts_offset_90k;
                    let sw_frame = if let Some(ref mut sws) = scaler {
                        let mut scaled = ffmpeg_next::util::frame::Video::empty();
                        if sws.run(&decoded, &mut scaled).is_err() {
                            continue;
                        }
                        scaled.set_pts(Some(new_pts));
                        scaled
                    } else {
                        decoded.set_pts(Some(new_pts));
                        decoded.clone()
                    };

                    let send_ok = if use_hw && !hw_frames_ctx.is_null() {
                        unsafe {
                            let mut hw_frame = ffmpeg_next::ffi::av_frame_alloc();
                            if hw_frame.is_null() {
                                continue;
                            }
                            let ret = ffmpeg_next::ffi::av_hwframe_get_buffer(
                                hw_frames_ctx,
                                hw_frame,
                                0,
                            );
                            if ret < 0 {
                                ffmpeg_next::ffi::av_frame_free(&mut hw_frame);
                                continue;
                            }
                            let ret = ffmpeg_next::ffi::av_hwframe_transfer_data(
                                hw_frame,
                                sw_frame.as_ptr() as *const _,
                                0,
                            );
                            if ret < 0 {
                                ffmpeg_next::ffi::av_frame_free(&mut hw_frame);
                                continue;
                            }
                            (*hw_frame).pts = new_pts;
                            let gpu_frame = ffmpeg_next::frame::Video::wrap(hw_frame);
                            video_encoder.send_frame(&gpu_frame).is_ok()
                        }
                    } else {
                        video_encoder.send_frame(&sw_frame).is_ok()
                    };

                    if send_ok {
                        let mut encoded = ffmpeg_next::Packet::empty();
                        while video_encoder.receive_packet(&mut encoded).is_ok() {
                            encoded.set_stream(out_video_idx);
                            encoded.rescale_ts(
                                ffmpeg_next::Rational::new(1, 90000),
                                out_video_tb,
                            );
                            let _ = encoded.write_interleaved(&mut octx);
                        }
                    }
                    video_frame_count += 1;
                }
            } else if Some(pkt_stream.index()) == audio_stream_idx {
                if let (Some(adec), Some(aenc)) =
                    (&mut audio_decoder, &mut audio_encoder_handle)
                {
                    if adec.send_packet(&packet).is_ok() {
                        let mut audio_frame = ffmpeg_next::util::frame::Audio::empty();
                        while adec.receive_frame(&mut audio_frame).is_ok() {
                            // Time-range filter using input time base.
                            if let Some(apts) = audio_frame.pts() {
                                let apts_secs = apts as f64
                                    * f64::from(audio_time_base.0)
                                    / f64::from(audio_time_base.1);
                                if apts_secs < start_time || apts_secs >= end_time {
                                    continue;
                                }
                            }

                            // Ensure the decoded frame has a valid channel
                            // layout so the resampler doesn't reject it.
                            ensure_frame_channel_layout(&mut audio_frame);

                            // Lazy resampler creation from the first decoded
                            // frame's actual format/layout.
                            if audio_resampler.is_none() {
                                if let (Some(ef), Some(el)) = (tc_enc_format_saved, tc_enc_layout_saved) {
                                    let frame_layout = frame_channel_layout(&audio_frame);
                                    let frame_format = audio_frame.format();
                                    let frame_rate = if audio_frame.rate() > 0 {
                                        audio_frame.rate()
                                    } else {
                                        audio_sample_rate
                                    };
                                    match ffmpeg_next::software::resampling::Context::get(
                                        frame_format,
                                        frame_layout,
                                        frame_rate,
                                        ef,
                                        el,
                                        frame_rate,
                                    ) {
                                        Ok(r) => {
                                            audio_resampler = Some(r);
                                        }
                                        Err(e) => {
                                            eprintln!(
                                                "[transcode] failed to create resampler from decoded frame \
                                                 (fmt={:?} ch={} rate={}): {e}",
                                                frame_format, frame_layout.channels(), frame_rate
                                            );
                                        }
                                    }
                                }
                            }

                            // Resample to AAC-compatible format (FLTP) if
                            // needed, then set synthetic PTS aligned with
                            // the segment start.
                            let frame_to_encode = if let Some(ref mut resampler) = audio_resampler {
                                let mut resampled = ffmpeg_next::frame::Audio::empty();
                                if resampler.run(&audio_frame, &mut resampled).is_err() {
                                    continue;
                                }
                                resampled
                            } else {
                                audio_frame.clone()
                            };

                            if let Some(aud_out_idx) = out_audio_idx {
                                encode_audio_frame(
                                    &frame_to_encode,
                                    aenc,
                                    &mut octx,
                                    aud_out_idx,
                                    audio_sample_rate,
                                    out_audio_tb.unwrap_or(ffmpeg_next::Rational::new(1, 90000)),
                                    &mut audio_sample_count,
                                    audio_ts_offset,
                                );
                            }
                        }
                    }
                }
            }
        }

        // ── Flush encoders ───────────────────────────────────────────────
        let _ = video_encoder.send_eof();
        let mut encoded = ffmpeg_next::Packet::empty();
        while video_encoder.receive_packet(&mut encoded).is_ok() {
            encoded.set_stream(out_video_idx);
            encoded.rescale_ts(
                ffmpeg_next::Rational::new(1, 90000),
                out_video_tb,
            );
            let _ = encoded.write_interleaved(&mut octx);
        }

        if let Some(ref mut aenc) = audio_encoder_handle {
            let _ = aenc.send_eof();
            let mut aenc_pkt = ffmpeg_next::Packet::empty();
            while aenc.receive_packet(&mut aenc_pkt).is_ok() {
                if let Some(aud_out_idx) = out_audio_idx {
                    aenc_pkt.set_stream(aud_out_idx);
                    aenc_pkt.rescale_ts(
                        ffmpeg_next::Rational::new(1, audio_sample_rate as i32),
                        out_audio_tb.unwrap_or(ffmpeg_next::Rational::new(1, 90000)),
                    );
                    let _ = aenc_pkt.write_interleaved(&mut octx);
                }
            }
        }

        write_fmp4_trailer(&mut octx)?;
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Helper: verify a segment has video and optionally audio.
    fn verify_segment(path: &Path, expect_audio: bool) {
        let octx = ffmpeg_next::format::input(path).unwrap();
        assert!(octx.streams().best(ffmpeg_next::media::Type::Video).is_some(), "output must have video");
        if expect_audio {
            let audio = octx.streams().best(ffmpeg_next::media::Type::Audio);
            assert!(audio.is_some(), "output must have audio");
            if let Some(a) = audio {
                let channels = unsafe { (*a.parameters().as_ptr()).ch_layout.nb_channels };
                assert!(channels > 0 && channels <= 2, "audio should be mono/stereo, got {channels} channels");
            }
        }
    }

    #[test]
    fn test_hybrid_6ch_aac() {
        let test_file = "/tmp/test_media/test_6ch_aac.mkv";
        if !Path::new(test_file).exists() { return; }
        super::super::ensure_init();

        let mut ictx = ffmpeg_next::format::input(&test_file).unwrap();
        assert!(video_is_remuxable(&ictx));
        assert!(!audio_is_remuxable(&ictx), "6ch audio should not be directly remuxable");

        let out = Path::new("/tmp/test_media/test_hybrid_6ch.m4s");
        hybrid_segment(&mut ictx, 0.0, out, None).expect("hybrid segment failed");
        verify_segment(out, true);
    }

    #[test]
    fn test_hybrid_6ch_aac_nonzero_start() {
        let test_file = "/tmp/test_media/test_6ch_aac.mkv";
        if !Path::new(test_file).exists() { return; }
        super::super::ensure_init();

        let mut ictx = ffmpeg_next::format::input(&test_file).unwrap();
        let out = Path::new("/tmp/test_media/test_hybrid_6ch_seg1.m4s");
        hybrid_segment(&mut ictx, 6.0, out, None).expect("hybrid segment at t=6s failed");
        verify_segment(out, true);
    }

    #[test]
    fn test_remux_stereo_aac() {
        let test_file = "/tmp/test_media/test_stereo_aac.mkv";
        if !Path::new(test_file).exists() { return; }
        super::super::ensure_init();

        let mut ictx = ffmpeg_next::format::input(&test_file).unwrap();
        assert!(source_is_remuxable(&ictx));

        let out = Path::new("/tmp/test_media/test_remux_stereo.m4s");
        remux_segment(&mut ictx, 0.0, out, None).expect("remux segment failed");
        verify_segment(out, true);
    }

    #[test]
    fn test_hybrid_mono_aac() {
        let test_file = "/tmp/test_media/test_mono_aac.mkv";
        if !Path::new(test_file).exists() { return; }
        super::super::ensure_init();

        let mut ictx = ffmpeg_next::format::input(&test_file).unwrap();
        // Mono AAC is remuxable (1 channel ≤ 2).
        assert!(audio_is_remuxable(&ictx));
    }

    #[test]
    fn test_hybrid_flac_audio() {
        let test_file = "/tmp/test_media/test_flac.mkv";
        if !Path::new(test_file).exists() { return; }
        super::super::ensure_init();

        let mut ictx = ffmpeg_next::format::input(&test_file).unwrap();
        assert!(video_is_remuxable(&ictx));
        assert!(!audio_is_remuxable(&ictx), "FLAC is not browser-remuxable");

        let out = Path::new("/tmp/test_media/test_hybrid_flac.m4s");
        hybrid_segment(&mut ictx, 0.0, out, None).expect("hybrid segment with FLAC failed");
        verify_segment(out, true);
    }

    /// Test that ensure_frame_channel_layout correctly handles a frame where
    /// ch_layout.order == UNSPEC but ch_layout.nb_channels > 0.  This is the
    /// exact scenario seen with Lavc62 (FFmpeg 8.x) encoded AAC files when
    /// decoded on a system with the ffmpeg_7_0 feature active — `channels()`
    /// returns 6 (from nb_channels) but the layout is still UNSPEC.
    ///
    /// Note: av_channel_layout_check returns 1 (valid) for UNSPEC with
    /// nb_channels>0, because FFmpeg considers it sufficient to know the
    /// channel count.  However, some versions of swr_alloc_set_opts2
    /// internally convert UNSPEC to a default layout during swr_init; when
    /// frames then arrive still UNSPEC, av_channel_layout_compare detects a
    /// mismatch and returns AVERROR_INPUT_CHANGED.  Our fix explicitly upgrades
    /// UNSPEC→NATIVE to prevent this and enable proper mixing matrices.
    #[test]
    fn test_ensure_frame_channel_layout_unspec_with_nb_channels() {
        super::super::ensure_init();

        // Build a minimal audio frame that has nb_channels=6 but order=UNSPEC.
        let mut frame = ffmpeg_next::frame::Audio::empty();
        unsafe {
            use ffmpeg_next::ffi::*;
            let raw = frame.as_mut_ptr();
            (*raw).format = AVSampleFormat::AV_SAMPLE_FMT_FLTP as i32;
            (*raw).sample_rate = 48000;
            (*raw).nb_samples = 1024;
            // Simulate Lavc62 style: UNSPEC order but nb_channels set.
            (*raw).ch_layout.order = AVChannelOrder::AV_CHANNEL_ORDER_UNSPEC;
            (*raw).ch_layout.nb_channels = 6;
            (*raw).ch_layout.u.mask = 0;
            av_frame_get_buffer(raw, 0);
        }

        // Before fix: order should be UNSPEC (= 0).
        let before_order = unsafe { (*frame.as_ptr()).ch_layout.order as u32 };
        assert_eq!(before_order, 0, "order should be UNSPEC (0) before fix");

        ensure_frame_channel_layout(&mut frame);

        // After fix: order should be non-UNSPEC (NATIVE = 1 or similar).
        let after_order = unsafe { (*frame.as_ptr()).ch_layout.order as u32 };
        assert_ne!(after_order, 0, "order should be non-UNSPEC after fix");

        // nb_channels should still be 6 (not collapsed to stereo).
        let nb = unsafe { (*frame.as_ptr()).ch_layout.nb_channels };
        assert_eq!(nb, 6, "nb_channels should remain 6 after fix");

        // The layout should now pass av_channel_layout_check.
        let check = unsafe {
            ffmpeg_next::ffi::av_channel_layout_check(&(*frame.as_ptr()).ch_layout)
        };
        assert_ne!(check, 0, "layout should pass av_channel_layout_check after fix");
    }
}
