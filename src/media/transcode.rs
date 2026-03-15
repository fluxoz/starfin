//! HLS segment transcoding — fully in-process via `ffmpeg-next`.
//!
//! Each segment is a 6-second MPEG-TS chunk encoded with H.264 (hardware or
//! software) and AAC audio.  The implementation uses `ffmpeg-next`'s
//! transcoding primitives: open input, seek, decode, (optionally) filter for
//! scaling, encode, and mux into an mpegts output.
//!
//! Hardware-accelerated encoding (NVENC, VAAPI, QSV, etc.) is driven entirely
//! through the raw `ffmpeg-next::ffi` bindings — `av_hwdevice_ctx_create`,
//! `av_hwframe_ctx_alloc`, and `av_hwframe_transfer_data` — so no ffmpeg
//! subprocess is needed for any quality tier.

use std::path::Path;

use super::hwaccel::HwAccel;

/// Duration of each HLS segment in seconds.
pub const SEGMENT_DURATION: f64 = 6.0;

/// Transcode a single MPEG-TS segment.
///
/// All quality tiers (High, Medium, Low) and all encoder backends (GPU and
/// software) are handled in-process via ffmpeg-next.
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

    // Run the CPU/GPU-intensive in-process transcode on a blocking thread
    // so we don't starve the tokio runtime.
    let abs_path = abs_path.to_owned();
    let hls_dir = hls_dir.to_owned();
    let hwaccel = hwaccel.clone();
    tokio::task::spawn_blocking(move || {
        transcode_segment_inprocess(&abs_path, &hls_dir, seg_index, &hwaccel, quality)
    })
    .await
    .map_err(|e| format!("transcode task panicked: {e}"))?
}

/// Quality level for on-demand video transcoding.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Default, serde::Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Quality {
    #[default]
    High,
    Medium,
    Low,
}

impl Quality {
    pub fn as_str(self) -> &'static str {
        match self {
            Quality::High   => "high",
            Quality::Medium => "medium",
            Quality::Low    => "low",
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            Quality::High   => "High",
            Quality::Medium => "Medium",
            Quality::Low    => "Low",
        }
    }
}

// ── In-process transcode (all quality tiers) ─────────────────────────────────

fn transcode_segment_inprocess(
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
    // Use detected frame rate, fall back to 30 fps if unavailable.
    let effective_fps = if frame_rate > 0.0 && frame_rate.is_finite() { frame_rate } else { 30.0 };

    let (out_width, out_height, crf, preset) = match quality {
        Quality::High => (in_width, in_height, "18", "veryslow"),
        Quality::Medium => {
            let max_w = 1280u32;
            if in_width <= max_w {
                (in_width, in_height, "26", "fast")
            } else {
                let ratio = max_w as f64 / in_width as f64;
                let h = ((in_height as f64 * ratio) as u32) & !1; // ensure even
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

    // Determine encoder: use the hardware encoder for High+GPU, libx264 for
    // software tiers.
    let use_hw = quality == Quality::High && *hwaccel != HwAccel::Software;
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

    // Run the main transcode logic; ensure hardware contexts are freed on
    // both success and error paths.
    let result = transcode_segment_body(
        &mut ictx,
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
        &tmp_path,
    );

    // Clean up hardware contexts.
    unsafe {
        if !hw_frames_ctx.is_null() {
            ffmpeg_next::ffi::av_buffer_unref(&mut hw_frames_ctx);
        }
        if !hw_device_ctx.is_null() {
            ffmpeg_next::ffi::av_buffer_unref(&mut hw_device_ctx);
        }
    }

    result?;

    // Atomic rename.
    std::fs::rename(&tmp_path, &seg_path)
        .map_err(|e| format!("failed to rename segment {seg_index}: {e}"))?;

    Ok(())
}

/// Inner body of transcode_segment_inprocess, extracted to simplify cleanup of
/// hardware device/frames contexts in the caller.
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
    // Set up video encoder.
    let video_encoder_codec = ffmpeg_next::encoder::find_by_name(encoder_name)
        .ok_or_else(|| format!("encoder '{}' not found", encoder_name))?;
    let video_encoder_ctx = ffmpeg_next::codec::context::Context::new_with_codec(video_encoder_codec);
    {
        let mut enc = video_encoder_ctx.encoder().video().map_err(|e| format!("video encoder setup: {e}"))?;
        enc.set_width(out_width);
        enc.set_height(out_height);
        enc.set_time_base(ffmpeg_next::Rational::new(1, 90000));
        enc.set_gop(250);
        enc.set_max_b_frames(0); // No B-frames for independent segment decoding

        if use_hw && !hw_frames_ctx.is_null() {
            // Hardware encoder: set pixel format and attach hw_frames_ctx.
            enc.set_format(ffmpeg_next::format::Pixel::NV12);
            unsafe {
                let ctx_ptr = enc.as_mut_ptr();
                (*ctx_ptr).hw_frames_ctx = ffmpeg_next::ffi::av_buffer_ref(hw_frames_ctx);
                (*ctx_ptr).pix_fmt = super::hwaccel::hw_pix_fmt_for(hwaccel);
            }
        } else {
            enc.set_format(ffmpeg_next::format::Pixel::YUV420P);
        }

        // Set encoder-specific options via the raw AVDictionary interface.
        let mut opts = ffmpeg_next::Dictionary::new();
        if use_hw {
            // Apply hardware-specific quality options.
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
            opts.set("level", if quality == Quality::High { "4.2" } else { "4.1" });
        }

        let mut video_encoder = enc.open_with(opts).map_err(|e| format!("open video encoder: {e}"))?;

        // Create the output muxer.
        let mut octx = ffmpeg_next::format::output_as(&tmp_path, "mpegts")
            .map_err(|e| format!("output context: {e}"))?;

        // Add video stream to output.
        let mut out_video_stream = octx.add_stream(video_encoder_codec)
            .map_err(|e| format!("add video stream: {e}"))?;
        out_video_stream.set_parameters(&video_encoder);
        let out_video_idx = out_video_stream.index();
        let out_video_tb = out_video_stream.time_base();

        // Optionally set up audio.
        let mut audio_decoder: Option<ffmpeg_next::decoder::Audio> = None;
        let mut audio_encoder_handle: Option<ffmpeg_next::encoder::Audio> = None;
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
                    let aac_codec = ffmpeg_next::encoder::find_by_name("aac");
                    if let Some(aac) = aac_codec {
                        let aac_ctx = ffmpeg_next::codec::context::Context::new_with_codec(aac);
                        if let Ok(mut aac_enc) = aac_ctx.encoder().audio() {
                            aac_enc.set_rate(dec.rate() as i32);
                            aac_enc.set_channel_layout(dec.channel_layout());
                            aac_enc.set_format(ffmpeg_next::format::Sample::F32(ffmpeg_next::format::sample::Type::Planar));
                            aac_enc.set_bit_rate(128_000);
                            aac_enc.set_time_base(ffmpeg_next::Rational::new(1, dec.rate() as i32));

                            if let Ok(opened) = aac_enc.open_as(aac) {
                                let mut out_aud_stream = octx.add_stream(aac)
                                    .map_err(|e| format!("add audio stream: {e}"))?;
                                out_aud_stream.set_parameters(&opened);
                                out_audio_idx = Some(out_aud_stream.index());
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

        // Set up scaler if needed.
        // For hardware encoding we scale to NV12; for software we stay in
        // YUV420P.
        let sw_out_fmt = if use_hw {
            ffmpeg_next::format::Pixel::NV12
        } else {
            ffmpeg_next::format::Pixel::YUV420P
        };

        let mut scaler: Option<ffmpeg_next::software::scaling::Context> = None;
        if out_width != in_width || out_height != in_height || (use_hw && !hw_frames_ctx.is_null()) {
            // Always set up a scaler for hw path (to convert YUV420P -> NV12),
            // or when dimensions differ.
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

        let end_time = start_time + SEGMENT_DURATION;
        let ts_offset_90k = (start_time * 90000.0) as i64;
        let mut frame_count: i64 = 0;
        let mut done = false;

        // Process packets.
        for (pkt_stream, packet) in ictx.packets() {
            if done { break; }

            if pkt_stream.index() == video_idx {
                if video_decoder.send_packet(&packet).is_err() {
                    continue;
                }

                let mut decoded = ffmpeg_next::util::frame::Video::empty();
                while video_decoder.receive_frame(&mut decoded).is_ok() {
                    // Check if we've passed the segment end.
                    let pts = decoded.pts().unwrap_or(0);
                    let pts_secs = pts as f64 * f64::from(video_time_base.0) / f64::from(video_time_base.1);

                    if pts_secs >= end_time {
                        done = true;
                        break;
                    }

                    // Skip frames before the segment start.  After seeking,
                    // the decoder produces frames from the nearest keyframe
                    // (which may be well before start_time).  These reference
                    // frames are needed internally by the decoder but must
                    // not be encoded into the segment — otherwise the segment
                    // would be longer than declared in the playlist and its
                    // PTS range would overlap with the adjacent segment.
                    if pts_secs < start_time {
                        continue;
                    }

                    // Scale if needed (also converts to NV12 for hw path).
                    let pts_increment = (90000.0 / effective_fps) as i64;
                    let new_pts = frame_count * pts_increment + ts_offset_90k;
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

                    // For hardware encoding, upload the software frame to the
                    // GPU surface before sending to the encoder.
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
                    frame_count += 1;
                }
            } else if Some(pkt_stream.index()) == audio_stream_idx {
                if let (Some(adec), Some(aenc)) =
                    (&mut audio_decoder, &mut audio_encoder_handle)
                {
                    if adec.send_packet(&packet).is_ok() {
                        let mut audio_frame = ffmpeg_next::util::frame::Audio::empty();
                        while adec.receive_frame(&mut audio_frame).is_ok() {
                            // Skip audio outside the segment's time range,
                            // mirroring the video frame filter above.
                            if let Some(apts) = audio_frame.pts() {
                                let apts_secs = apts as f64
                                    * f64::from(audio_time_base.0)
                                    / f64::from(audio_time_base.1);
                                if apts_secs < start_time || apts_secs >= end_time {
                                    continue;
                                }
                            }
                            if aenc.send_frame(&audio_frame).is_ok() {
                                let mut encoded = ffmpeg_next::Packet::empty();
                                while aenc.receive_packet(&mut encoded).is_ok() {
                                    if let Some(aud_out_idx) = out_audio_idx {
                                        encoded.set_stream(aud_out_idx);
                                        // Rescale from encoder timebase
                                        // (1/sample_rate) to the mpegts
                                        // stream timebase (1/90 000).
                                        encoded.rescale_ts(
                                            ffmpeg_next::Rational::new(1, audio_sample_rate as i32),
                                            ffmpeg_next::Rational::new(1, 90000),
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

        // Flush encoders.
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
                        ffmpeg_next::Rational::new(1, 90000),
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
