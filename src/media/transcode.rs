//! HLS segment creation — direct remux when possible, transcode as fallback.
//!
//! Each segment is a 6-second MPEG-TS chunk.  For **Original** quality with
//! browser-compatible codecs (H.264 video + stereo AAC/MP3 audio) the segment
//! is created by **remuxing** — copying compressed packets directly from the
//! source file without decoding or re-encoding.  This is near-instant (pure
//! I/O, like VLC playback) and gives performance parity with direct file
//! access.
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

use super::hwaccel::HwAccel;

/// Duration of each HLS segment in seconds.
pub const SEGMENT_DURATION: f64 = 6.0;

/// Bitrate for AAC audio encoding in the transcode and hybrid paths.
/// 256 kbps stereo AAC-LC is transparent quality for music and dialogue.
const AAC_ENCODE_BITRATE: usize = 256_000;

/// Create a single MPEG-TS segment — remux if possible, transcode otherwise.
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
    hls_dir: &Path,
    seg_index: usize,
    hwaccel: &HwAccel,
    quality: super::transcode::Quality,
) -> Result<(), String> {
    let filename = format!("seg_{:05}.ts", seg_index);
    let seg_path = hls_dir.join(&filename);

    if seg_path.exists() {
        return Ok(());
    }

    let start_time = seg_index as f64 * SEGMENT_DURATION;
    debug_assert!(start_time >= 0.0 && start_time.is_finite());

    // Run the I/O or CPU-intensive work on a blocking thread so we don't
    // starve the tokio runtime.
    let abs_path = abs_path.to_owned();
    let hls_dir = hls_dir.to_owned();
    let hwaccel = hwaccel.clone();
    tokio::task::spawn_blocking(move || {
        create_segment(&abs_path, &hls_dir, seg_index, &hwaccel, quality)
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

// ── Segment creation: decide remux vs transcode ─────────────────────────────

fn create_segment(
    abs_path: &str,
    hls_dir: &Path,
    seg_index: usize,
    hwaccel: &HwAccel,
    quality: Quality,
) -> Result<(), String> {
    super::ensure_init();

    let start_time = seg_index as f64 * SEGMENT_DURATION;
    let tmp_filename = format!(".seg_{:05}.ts.tmp", seg_index);
    let tmp_path = hls_dir.join(&tmp_filename);
    let filename = format!("seg_{:05}.ts", seg_index);
    let seg_path = hls_dir.join(&filename);

    // Open input.
    let mut ictx = ffmpeg_next::format::input(&abs_path)
        .map_err(|e| format!("failed to open input: {e}"))?;

    // Decide: remux (fast copy) or transcode (re-encode).
    if quality.can_remux() && source_is_remuxable(&ictx) {
        // Pure remux — both video and audio packets copied directly.
        remux_segment(&mut ictx, start_time, &tmp_path)?;
    } else if quality.can_remux() && video_is_remuxable(&ictx) {
        // Hybrid — video packets copied, audio transcoded to stereo AAC.
        // This handles multi-channel AAC, non-AAC/MP3 codecs, etc.
        hybrid_segment(&mut ictx, start_time, &tmp_path)?;
    } else {
        // For Original quality with incompatible codecs, fall back to the
        // same settings as High (native resolution, best quality).
        let effective_quality = if quality == Quality::Original { Quality::High } else { quality };
        transcode_segment_inprocess(&mut ictx, start_time, hwaccel, effective_quality, &tmp_path)?;
    }

    // Atomic rename.
    std::fs::rename(&tmp_path, &seg_path)
        .map_err(|e| format!("failed to rename segment {seg_index}: {e}"))?;

    Ok(())
}

// ── Remux path (direct packet copy — near-instant) ──────────────────────────

/// Copy compressed packets from the source into an MPEG-TS segment without
/// decoding or re-encoding.  This is the equivalent of
/// `ffmpeg -ss <t> -i input -t 6 -c copy -f mpegts output.ts`
/// and gives VLC-like performance.
fn remux_segment(
    ictx: &mut ffmpeg_next::format::context::Input,
    start_time: f64,
    tmp_path: &Path,
) -> Result<(), String> {
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

    // Create output muxer.
    let mut octx = ffmpeg_next::format::output_as(tmp_path, "mpegts")
        .map_err(|e| format!("output context: {e}"))?;

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

    octx.write_header()
        .map_err(|e| format!("write header: {e}"))?;

    // Re-read output time bases after write_header (muxer may adjust them).
    let out_video_tb = octx.stream(out_video_idx).unwrap().time_base();
    let out_audio_tb = out_audio_idx_val.map(|i| octx.stream(i).unwrap().time_base()).unwrap_or(out_audio_tb);

    let mut got_video_keyframe = false;
    // PTS offsets (in output time base) used to rebase timestamps so each
    // segment starts near PTS 0, preventing A/V desync in HLS players.
    let mut video_pts_offset: Option<i64> = None;
    let mut audio_pts_offset: Option<i64> = None;

    for (stream, mut packet) in ictx.packets() {
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
            if !got_video_keyframe {
                if !packet.is_key() || pts_secs < start_time - SEGMENT_DURATION {
                    continue;
                }
                got_video_keyframe = true;
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
            // Record the first video DTS (or PTS) as the offset to rebase to
            // zero.  Using DTS prevents negative DTS values for B-frame
            // content where DTS < PTS on the first packet.
            if video_pts_offset.is_none() {
                video_pts_offset = packet.dts().or(packet.pts());
            }
            if let (Some(offset), Some(p)) = (video_pts_offset, packet.pts()) {
                packet.set_pts(Some(p - offset));
            }
            if let (Some(offset), Some(d)) = (video_pts_offset, packet.dts()) {
                packet.set_dts(Some(d - offset));
            }
        } else if let Some(out_ai) = out_audio_idx_val {
            packet.set_stream(out_ai);
            packet.rescale_ts(in_audio_tb.unwrap(), out_audio_tb);
            // Record the first audio DTS (or PTS) as the offset to rebase to zero.
            if audio_pts_offset.is_none() {
                audio_pts_offset = packet.dts().or(packet.pts());
            }
            if let (Some(offset), Some(p)) = (audio_pts_offset, packet.pts()) {
                packet.set_pts(Some(p - offset));
            }
            if let (Some(offset), Some(d)) = (audio_pts_offset, packet.dts()) {
                packet.set_dts(Some(d - offset));
            }
        } else {
            continue;
        }

        let _ = packet.write_interleaved(&mut octx);
    }

    octx.write_trailer()
        .map_err(|e| format!("write trailer: {e}"))?;

    Ok(())
}

// ── Hybrid path (video remux + audio transcode to stereo AAC) ───────────────

/// Copy video packets directly while decoding and re-encoding the audio
/// stream to stereo AAC.  This is used when the video is H.264 (browser-
/// compatible) but the audio cannot be remuxed — typically because it is
/// multi-channel (5.1/7.1) AAC which browsers don't support in HLS/MPEG-TS.
///
/// Gives the best quality+speed trade-off: lossless video copy (like remux)
/// with only lightweight audio transcoding.
fn hybrid_segment(
    ictx: &mut ffmpeg_next::format::context::Input,
    start_time: f64,
    tmp_path: &Path,
) -> Result<(), String> {
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

    // Create output muxer.
    let mut octx = ffmpeg_next::format::output_as(tmp_path, "mpegts")
        .map_err(|e| format!("output context: {e}"))?;

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

    // ── Audio decoder + encoder + resampler setup ────────────────────────
    let mut audio_decoder: Option<ffmpeg_next::decoder::Audio> = None;
    let mut audio_encoder_handle: Option<ffmpeg_next::encoder::Audio> = None;
    let mut audio_resampler: Option<ffmpeg_next::software::resampling::Context> = None;
    let mut out_audio_idx: Option<usize> = None;
    let mut audio_sample_rate: u32 = 48000;
    let mut audio_time_base = ffmpeg_next::Rational::new(1, 48000);

    if let Some(ref a_params) = in_audio_params {
        let aud_idx = audio_idx.unwrap();
        audio_time_base = in_audio_tb.unwrap();

        match ffmpeg_next::codec::context::Context::from_parameters(a_params.clone()) {
            Ok(aud_ctx) => match aud_ctx.decoder().audio() {
                Ok(dec) => {
                    audio_sample_rate = dec.rate();
                    let dec_format = dec.format();
                    let raw_dec_layout = dec.channel_layout();

                    // Handle empty/unspecified channel layout.
                    let dec_layout = if raw_dec_layout.channels() > 0 {
                        raw_dec_layout
                    } else {
                        let ch = dec.channels() as i32;
                        let ch = if ch > 0 { ch } else { 2 };
                        eprintln!(
                            "[hybrid] audio stream has no channel layout, \
                             defaulting to standard layout for {ch} channel(s)"
                        );
                        ffmpeg_next::channel_layout::ChannelLayout::default(ch)
                    };

                    // Always output MONO or STEREO for browser compatibility.
                    let enc_layout = if dec_layout.channels() == 1 {
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

                                        match ffmpeg_next::software::resampling::Context::get(
                                            dec_format,
                                            dec_layout,
                                            dec.rate(),
                                            enc_format,
                                            enc_layout,
                                            dec.rate(),
                                        ) {
                                            Ok(r) => {
                                                audio_resampler = Some(r);
                                                audio_encoder_handle = Some(opened);
                                                audio_decoder = Some(dec);
                                            }
                                            Err(e) => {
                                                eprintln!("[hybrid] failed to create audio resampler: {e}");
                                            }
                                        }
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

    octx.write_header()
        .map_err(|e| format!("write header: {e}"))?;

    // Re-read output time bases after write_header (muxer may adjust them).
    let out_video_tb = octx.stream(out_video_idx).unwrap().time_base();
    let out_audio_tb = out_audio_idx.map(|i| octx.stream(i).unwrap().time_base());

    let mut got_video_keyframe = false;
    let mut video_pts_offset: Option<i64> = None;

    // Audio synthetic PTS (in 1/sample_rate time base).
    let mut audio_sample_count: i64 = 0;
    let audio_ts_offset = (start_time * audio_sample_rate as f64) as i64;

    for (stream, mut packet) in ictx.packets() {
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
            if !got_video_keyframe {
                if !packet.is_key() || pts_secs < start_time - SEGMENT_DURATION {
                    continue;
                }
                got_video_keyframe = true;
            }

            // Copy video packet directly (remux).
            packet.set_stream(out_video_idx);
            packet.rescale_ts(in_video_tb, out_video_tb);
            if video_pts_offset.is_none() {
                video_pts_offset = packet.dts().or(packet.pts());
            }
            if let (Some(offset), Some(p)) = (video_pts_offset, packet.pts()) {
                packet.set_pts(Some(p - offset));
            }
            if let (Some(offset), Some(d)) = (video_pts_offset, packet.dts()) {
                packet.set_dts(Some(d - offset));
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

                    let frame_to_encode = if let Some(ref mut resampler) = audio_resampler {
                        let mut resampled = ffmpeg_next::frame::Audio::empty();
                        match resampler.run(&audio_frame, &mut resampled) {
                            Ok(_) => {},
                            Err(e) => {
                                eprintln!(
                                    "[hybrid] resampler.run error: {e} (in: samples={} ch={} fmt={:?}, out: samples={} ch={} fmt={:?})",
                                    audio_frame.samples(), audio_frame.channels(), audio_frame.format(),
                                    resampled.samples(), resampled.channels(), resampled.format(),
                                );
                                continue;
                            }
                        }
                        let new_pts = audio_sample_count + audio_ts_offset;
                        resampled.set_pts(Some(new_pts));
                        audio_sample_count += resampled.samples() as i64;
                        resampled
                    } else {
                        let new_pts = audio_sample_count + audio_ts_offset;
                        audio_frame.set_pts(Some(new_pts));
                        audio_sample_count += audio_frame.samples() as i64;
                        audio_frame.clone()
                    };

                    match aenc.send_frame(&frame_to_encode) {
                        Ok(()) => {
                            let mut encoded = ffmpeg_next::Packet::empty();
                            while aenc.receive_packet(&mut encoded).is_ok() {
                                if let Some(aud_out_idx) = out_audio_idx {
                                    encoded.set_stream(aud_out_idx);
                                    encoded.rescale_ts(
                                        ffmpeg_next::Rational::new(1, audio_sample_rate as i32),
                                        out_audio_tb.unwrap_or(ffmpeg_next::Rational::new(1, 90000)),
                                    );
                                    let _ = encoded.write_interleaved(&mut octx);
                                }
                            }
                        }
                        Err(e) => {
                            eprintln!(
                                "[hybrid] encoder.send_frame error: {e} (samples={} ch={} fmt={:?} pts={:?})",
                                frame_to_encode.samples(), frame_to_encode.channels(),
                                frame_to_encode.format(), frame_to_encode.pts(),
                            );
                        }
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

    octx.write_trailer()
        .map_err(|e| format!("write trailer: {e}"))?;

    Ok(())
}

// ── Transcode path (re-encode — used for incompatible codecs or scaling) ────

fn transcode_segment_inprocess(
    ictx: &mut ffmpeg_next::format::context::Input,
    start_time: f64,
    hwaccel: &HwAccel,
    quality: Quality,
    tmp_path: &Path,
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

        // ── Output muxer ─────────────────────────────────────────────────
        let mut octx = ffmpeg_next::format::output_as(tmp_path, "mpegts")
            .map_err(|e| format!("output context: {e}"))?;

        let mut out_video_stream = octx.add_stream(video_encoder_codec)
            .map_err(|e| format!("add video stream: {e}"))?;
        out_video_stream.set_parameters(&video_encoder);
        let out_video_idx = out_video_stream.index();

        // ── Audio encoder + resampler setup ──────────────────────────────
        let mut audio_decoder: Option<ffmpeg_next::decoder::Audio> = None;
        let mut audio_encoder_handle: Option<ffmpeg_next::encoder::Audio> = None;
        let mut audio_resampler: Option<ffmpeg_next::software::resampling::Context> = None;
        let mut out_audio_idx: Option<usize> = None;
        let mut audio_time_base = ffmpeg_next::Rational::new(1, 44100);
        let mut audio_sample_rate: u32 = 44100;

        if let Some(aud_idx) = audio_stream_idx {
            let aud_stream = ictx.stream(aud_idx).unwrap();
            audio_time_base = aud_stream.time_base();
            let aud_params = aud_stream.parameters();

            match ffmpeg_next::codec::context::Context::from_parameters(aud_params) {
                Ok(aud_ctx) => match aud_ctx.decoder().audio() {
                    Ok(dec) => {
                        audio_sample_rate = dec.rate();
                        let dec_format = dec.format();
                        let raw_dec_layout = dec.channel_layout();

                        // Many containers don't store channel layout metadata,
                        // only the channel count.  When the layout is empty
                        // (channels() == 0 in the bitflags representation) the
                        // resampler and AAC encoder will fail.  Derive a
                        // concrete native-order layout from the decoder's
                        // channel count so both always get valid input.
                        let dec_layout = if raw_dec_layout.channels() > 0 {
                            raw_dec_layout
                        } else {
                            let ch = dec.channels() as i32;
                            // Fall back to stereo if the decoder also reports 0 channels
                            // (e.g. before the first frame is decoded).
                            let ch = if ch > 0 { ch } else { 2 /* stereo */ };
                            eprintln!(
                                "[transcode] audio stream has no channel layout, \
                                 defaulting to standard layout for {ch} channel(s)"
                            );
                            ffmpeg_next::channel_layout::ChannelLayout::default(ch)
                        };

                        // Downmix to stereo if source is multi-channel because
                        // the native AAC encoder only reliably supports mono and stereo.
                        let enc_layout = if dec_layout.channels() > 2 {
                            ffmpeg_next::channel_layout::ChannelLayout::STEREO
                        } else if dec_layout.channels() == 1 {
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

                                            // Create resampler: decoder format → AAC's required FLTP.
                                            // Also handles channel layout downmix and sample rate normalization.
                                            match ffmpeg_next::software::resampling::Context::get(
                                                dec_format,
                                                dec_layout,
                                                dec.rate(),
                                                enc_format,
                                                enc_layout,
                                                dec.rate(),
                                            ) {
                                                Ok(r) => {
                                                    audio_resampler = Some(r);
                                                    audio_encoder_handle = Some(opened);
                                                    audio_decoder = Some(dec);
                                                }
                                                Err(e) => {
                                                    eprintln!("[transcode] failed to create audio resampler: {e}");
                                                }
                                            }
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

        octx.write_header()
            .map_err(|e| format!("write header: {e}"))?;

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

                            // Resample to AAC-compatible format (FLTP) if
                            // needed, then set synthetic PTS aligned with
                            // the segment start.
                            let frame_to_encode = if let Some(ref mut resampler) = audio_resampler {
                                let mut resampled = ffmpeg_next::frame::Audio::empty();
                                if resampler.run(&audio_frame, &mut resampled).is_err() {
                                    continue;
                                }
                                let new_pts = audio_sample_count + audio_ts_offset;
                                resampled.set_pts(Some(new_pts));
                                audio_sample_count += resampled.samples() as i64;
                                resampled
                            } else {
                                let new_pts = audio_sample_count + audio_ts_offset;
                                audio_frame.set_pts(Some(new_pts));
                                audio_sample_count += audio_frame.samples() as i64;
                                audio_frame.clone()
                            };

                            if aenc.send_frame(&frame_to_encode).is_ok() {
                                let mut encoded = ffmpeg_next::Packet::empty();
                                while aenc.receive_packet(&mut encoded).is_ok() {
                                    if let Some(aud_out_idx) = out_audio_idx {
                                        encoded.set_stream(aud_out_idx);
                                        encoded.rescale_ts(
                                            ffmpeg_next::Rational::new(1, audio_sample_rate as i32),
                                            out_audio_tb.unwrap_or(ffmpeg_next::Rational::new(1, 90000)),
                                        );
                                        let _ = encoded.write_interleaved(&mut octx);
                                    }
                                }
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

        octx.write_trailer()
            .map_err(|e| format!("write trailer: {e}"))?;
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    
    #[test]
    fn test_hybrid_6ch_aac() {
        // Create a test 6ch AAC file first
        let test_file = "/tmp/test_media/test_6ch_aac.mkv";
        if !std::path::Path::new(test_file).exists() {
            eprintln!("Skipping test - test file not found");
            return;
        }
        
        super::super::ensure_init();
        
        let mut ictx = ffmpeg_next::format::input(&test_file).unwrap();
        
        // Verify it should go through hybrid path
        assert!(video_is_remuxable(&ictx), "Video should be remuxable (H.264)");
        assert!(!audio_is_remuxable(&ictx), "Audio should NOT be remuxable (6ch)");
        
        // Try hybrid segment creation
        let out_path = std::path::Path::new("/tmp/test_media/test_hybrid_seg.ts");
        match hybrid_segment(&mut ictx, 0.0, out_path) {
            Ok(()) => {
                eprintln!("Hybrid segment created successfully");
                // Verify the output
                let octx = ffmpeg_next::format::input(&out_path).unwrap();
                let has_video = octx.streams().best(ffmpeg_next::media::Type::Video).is_some();
                let has_audio = octx.streams().best(ffmpeg_next::media::Type::Audio).is_some();
                eprintln!("Output has video: {}, audio: {}", has_video, has_audio);
                assert!(has_video, "Output must have video");
                assert!(has_audio, "Output must have audio");
                
                // Check audio properties
                let audio_stream = octx.streams().best(ffmpeg_next::media::Type::Audio).unwrap();
                let channels = unsafe { (*audio_stream.parameters().as_ptr()).ch_layout.nb_channels };
                eprintln!("Output audio channels: {}", channels);
                assert!(channels <= 2, "Output audio should be mono or stereo, got {} channels", channels);
            }
            Err(e) => {
                panic!("Hybrid segment creation failed: {}", e);
            }
        }
    }
    
    #[test]
    fn test_remux_stereo_aac() {
        let test_file = "/tmp/test_media/test_stereo_aac.mkv";
        if !std::path::Path::new(test_file).exists() {
            eprintln!("Skipping test - test file not found");
            return;
        }
        
        super::super::ensure_init();
        
        let mut ictx = ffmpeg_next::format::input(&test_file).unwrap();
        
        // Verify it should go through remux path
        assert!(video_is_remuxable(&ictx), "Video should be remuxable (H.264)");
        assert!(audio_is_remuxable(&ictx), "Audio should be remuxable (stereo AAC)");
        assert!(source_is_remuxable(&ictx), "Source should be fully remuxable");
        
        let out_path = std::path::Path::new("/tmp/test_media/test_remux_seg.ts");
        match remux_segment(&mut ictx, 0.0, out_path) {
            Ok(()) => {
                let octx = ffmpeg_next::format::input(&out_path).unwrap();
                let has_video = octx.streams().best(ffmpeg_next::media::Type::Video).is_some();
                let has_audio = octx.streams().best(ffmpeg_next::media::Type::Audio).is_some();
                assert!(has_video, "Remux output must have video");
                assert!(has_audio, "Remux output must have audio");
            }
            Err(e) => {
                panic!("Remux segment creation failed: {}", e);
            }
        }
    }
}
