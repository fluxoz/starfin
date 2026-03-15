//! HLS segment creation — direct remux when possible, transcode as fallback.
//!
//! Each segment is a 6-second MPEG-TS chunk.  For **Original** quality with
//! browser-compatible codecs (H.264 video + AAC/MP3 audio) the segment is
//! created by **remuxing** — copying compressed packets directly from the
//! source file without decoding or re-encoding.  This is near-instant (pure
//! I/O, like VLC playback) and gives performance parity with direct file
//! access.
//!
//! When remuxing is not possible (incompatible codec, or High/Medium/Low
//! quality that requires re-encoding or resolution scaling) the segment is
//! **transcoded** in-process via `ffmpeg-next`.
//!
//! Hardware-accelerated encoding (NVENC, VAAPI, QSV, etc.) is available for
//! the transcode fallback path via the raw FFI bindings.

use std::path::Path;

use super::hwaccel::HwAccel;

/// Duration of each HLS segment in seconds.
pub const SEGMENT_DURATION: f64 = 6.0;

/// Create a single MPEG-TS segment — remux if possible, transcode otherwise.
///
/// For **Original** quality with H.264 + AAC/MP3 source, packets are copied
/// directly (remux).  For incompatible codecs or High/Medium/Low quality that
/// requires re-encoding, the full transcode path is used.
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

/// Return `true` if the source codecs are browser-compatible and can be
/// remuxed directly into MPEG-TS without re-encoding.
fn source_is_remuxable(ictx: &ffmpeg_next::format::context::Input) -> bool {
    use ffmpeg_next::codec::Id;

    let video_ok = ictx
        .streams()
        .best(ffmpeg_next::media::Type::Video)
        .map(|s| s.parameters().id() == Id::H264)
        .unwrap_or(false);

    let audio_ok = ictx
        .streams()
        .best(ffmpeg_next::media::Type::Audio)
        .map(|s| {
            let id = s.parameters().id();
            id == Id::AAC || id == Id::MP3
        })
        // No audio stream is fine — video-only remux is valid.
        .unwrap_or(true);

    video_ok && audio_ok
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
        remux_segment(&mut ictx, start_time, &tmp_path)?;
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
        if let Some(out_audio) = octx.add_stream(enc).ok() {
            unsafe {
                ffmpeg_next::ffi::avcodec_parameters_copy(
                    out_audio.parameters().as_mut_ptr(),
                    a_params.as_ptr(),
                );
            }
            out_audio_tb = out_audio.time_base();
            out_audio_idx_val = Some(out_audio.index());
        }
    }

    octx.write_header()
        .map_err(|e| format!("write header: {e}"))?;

    // Re-read output time bases after write_header (muxer may adjust them).
    let out_video_tb = octx.stream(out_video_idx).unwrap().time_base();
    let out_audio_tb = out_audio_idx_val.map(|i| octx.stream(i).unwrap().time_base()).unwrap_or(out_audio_tb);

    let mut got_video_keyframe = false;

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
        } else if let Some(out_ai) = out_audio_idx_val {
            packet.set_stream(out_ai);
            packet.rescale_ts(in_audio_tb.unwrap(), out_audio_tb);
        } else {
            continue;
        }

        let _ = packet.write_interleaved(&mut octx);
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

            if let Ok(aud_ctx) = ffmpeg_next::codec::context::Context::from_parameters(aud_params) {
                if let Ok(dec) = aud_ctx.decoder().audio() {
                    audio_sample_rate = dec.rate();
                    let dec_format = dec.format();
                    let dec_layout = dec.channel_layout();

                    let aac_codec = ffmpeg_next::encoder::find_by_name("aac");
                    if let Some(aac) = aac_codec {
                        let enc_format = ffmpeg_next::format::Sample::F32(
                            ffmpeg_next::format::sample::Type::Planar,
                        );
                        let aac_ctx = ffmpeg_next::codec::context::Context::new_with_codec(aac);
                        if let Ok(mut aac_enc) = aac_ctx.encoder().audio() {
                            aac_enc.set_rate(dec.rate() as i32);
                            aac_enc.set_channel_layout(dec_layout);
                            aac_enc.set_format(enc_format);
                            aac_enc.set_bit_rate(128_000);
                            aac_enc.set_time_base(ffmpeg_next::Rational::new(1, dec.rate() as i32));

                            if let Ok(opened) = aac_enc.open_as(aac) {
                                let mut out_aud_stream = octx.add_stream(aac)
                                    .map_err(|e| format!("add audio stream: {e}"))?;
                                out_aud_stream.set_parameters(&opened);
                                out_audio_idx = Some(out_aud_stream.index());

                                // Create resampler: decoder format → AAC's
                                // required FLTP.  Also handles channel layout
                                // and sample rate normalization.
                                let resampler = ffmpeg_next::software::resampling::Context::get(
                                    dec_format,
                                    dec_layout,
                                    dec.rate(),
                                    enc_format,
                                    dec_layout,
                                    dec.rate(),
                                ).ok();

                                audio_resampler = resampler;
                                audio_encoder_handle = Some(opened);
                                audio_decoder = Some(dec);
                            }
                        }
                    }
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
