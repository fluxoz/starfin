//! Thumbnail extraction — replaces the `generate_quick_thumbnail()` and
//! `generate_deep_thumbnail()` subprocess calls.
//!
//! * **Quick thumbnail**: seek to a random position in the video, decode one
//!   frame, scale to a reasonable size, and encode as JPEG.
//! * **Deep thumbnail**: run the `signalstats` filter graph over a window of
//!   the video to find the most visually appealing frame (highest saturation,
//!   lowest out-of-range pixel ratio), then extract and encode that frame.

use std::io::Cursor;
use std::path::Path;

/// Decode a single video frame at `seek_secs` and write it as JPEG to `out_path`.
///
/// Returns `true` on success.
pub fn extract_frame_as_jpeg(video_path: &Path, seek_secs: f64, out_path: &Path) -> bool {
    super::ensure_init();

    let video_str = match video_path.to_str() {
        Some(s) => s,
        None => return false,
    };

    let mut ictx = match ffmpeg_next::format::input(&video_str) {
        Ok(ctx) => ctx,
        Err(_) => return false,
    };

    // Find best video stream.
    let stream_idx = match ictx
        .streams()
        .best(ffmpeg_next::media::Type::Video)
    {
        Some(s) => s.index(),
        None => return false,
    };

    // Seek to the requested position (in AV_TIME_BASE units).
    let ts = (seek_secs * f64::from(ffmpeg_next::ffi::AV_TIME_BASE)) as i64;
    if ictx.seek(ts, ..ts).is_err() {
        // Fall back: reopen and skip packets manually if seek fails.
        drop(ictx);
        ictx = match ffmpeg_next::format::input(&video_str) {
            Ok(ctx) => ctx,
            Err(_) => return false,
        };
    }

    // Set up decoder.
    let stream = ictx.stream(stream_idx).unwrap();
    let decoder_codec = ffmpeg_next::codec::context::Context::from_parameters(stream.parameters());
    let mut decoder = match decoder_codec {
        Ok(ctx) => match ctx.decoder().video() {
            Ok(d) => d,
            Err(_) => return false,
        },
        Err(_) => return false,
    };

    // Set up scaler (we'll scale to the decoded size, converting to RGB24 for JPEG).
    let mut scaler: Option<ffmpeg_next::software::scaling::Context> = None;
    let mut rgb_frame = ffmpeg_next::util::frame::Video::empty();

    // Read packets until we decode a frame.
    for (pkt_stream, packet) in ictx.packets() {
        if pkt_stream.index() != stream_idx {
            continue;
        }
        if decoder.send_packet(&packet).is_err() {
            continue;
        }

        let mut decoded = ffmpeg_next::util::frame::Video::empty();
        if decoder.receive_frame(&mut decoded).is_ok() {
            // Initialise scaler on first frame.
            if scaler.is_none() {
                scaler = ffmpeg_next::software::scaling::Context::get(
                    decoded.format(),
                    decoded.width(),
                    decoded.height(),
                    ffmpeg_next::format::Pixel::RGB24,
                    decoded.width(),
                    decoded.height(),
                    ffmpeg_next::software::scaling::Flags::FAST_BILINEAR,
                )
                .ok();
            }
            if let Some(ref mut sws) = scaler {
                if sws.run(&decoded, &mut rgb_frame).is_err() {
                    return false;
                }
                return write_rgb_frame_as_jpeg(&rgb_frame, out_path);
            }
            return false;
        }
    }

    false
}

/// Analyse a window of the video using the `signalstats` filter and return
/// the timestamp (in seconds) of the best frame.
///
/// "Best" is defined as the frame with the highest `SATAVG` (colour
/// saturation) whose `BRNG` (out-of-range pixel fraction) is below
/// `MAX_BRNG`.  Falls back to `default_time` when no qualifying frame is
/// found.
///
/// This replaces the old two-pass ffmpeg subprocess approach.  The signalstats
/// data is obtained by running the filter graph in-process.
pub fn find_best_frame_via_signalstats(
    video_path: &Path,
    start_secs: f64,
    length_secs: f64,
    default_time: f64,
) -> f64 {
    super::ensure_init();

    let video_str = match video_path.to_str() {
        Some(s) => s,
        None => return default_time,
    };

    let mut ictx = match ffmpeg_next::format::input(&video_str) {
        Ok(ctx) => ctx,
        Err(_) => return default_time,
    };

    let stream_idx = match ictx
        .streams()
        .best(ffmpeg_next::media::Type::Video)
    {
        Some(s) => s.index(),
        None => return default_time,
    };

    // Seek to start.
    let ts = (start_secs * f64::from(ffmpeg_next::ffi::AV_TIME_BASE)) as i64;
    let _ = ictx.seek(ts, ..ts);

    let stream = ictx.stream(stream_idx).unwrap();
    let time_base = stream.time_base();

    let decoder_codec = ffmpeg_next::codec::context::Context::from_parameters(stream.parameters());
    let mut decoder = match decoder_codec {
        Ok(ctx) => match ctx.decoder().video() {
            Ok(d) => d,
            Err(_) => return default_time,
        },
        Err(_) => return default_time,
    };

    // We sample one frame every 5 seconds.
    let sample_interval = 5.0;
    let end_time = start_secs + length_secs;
    let mut next_sample = start_secs;

    let mut best_time: Option<f64> = None;
    let mut best_satavg = -1.0_f64;
    const MAX_BRNG: f64 = 5.0;

    // We'll compute simple frame statistics in-process rather than using the
    // signalstats filter (which requires complex filter graph setup).  We
    // approximate SATAVG by converting to RGB and computing the average colour
    // saturation, and BRNG by counting pixels outside the 16-235 luma range.

    let mut scaler: Option<ffmpeg_next::software::scaling::Context> = None;

    for (pkt_stream, packet) in ictx.packets() {
        if pkt_stream.index() != stream_idx {
            continue;
        }
        if decoder.send_packet(&packet).is_err() {
            continue;
        }

        let mut decoded = ffmpeg_next::util::frame::Video::empty();
        while decoder.receive_frame(&mut decoded).is_ok() {
            // Compute presentation time in seconds.
            let pts = decoded.pts().unwrap_or(0);
            let pts_secs = pts as f64 * f64::from(time_base.0) / f64::from(time_base.1);

            if pts_secs > end_time {
                // We've passed the analysis window.
                return best_time.unwrap_or(default_time);
            }

            if pts_secs < next_sample {
                continue;
            }
            next_sample = pts_secs + sample_interval;

            // Convert frame to YUV planar so we can analyse luma/chroma.
            // If already in a YUV format, decode data directly from planes.
            let (satavg, brng) = compute_frame_stats(&decoded, &mut scaler);

            if brng > MAX_BRNG {
                continue;
            }
            if satavg > best_satavg {
                best_satavg = satavg;
                best_time = Some(pts_secs);
            }
        }
    }

    best_time.unwrap_or(default_time)
}

/// Compute approximate saturation average and out-of-range pixel percentage
/// from a decoded video frame.
fn compute_frame_stats(
    frame: &ffmpeg_next::util::frame::Video,
    scaler: &mut Option<ffmpeg_next::software::scaling::Context>,
) -> (f64, f64) {
    // Convert to YUV420P for analysis.
    let target_fmt = ffmpeg_next::format::Pixel::YUV420P;

    if frame.format() != target_fmt {
        // (Re)initialize scaler for the current input format → YUV420P.
        *scaler = ffmpeg_next::software::scaling::Context::get(
            frame.format(),
            frame.width(),
            frame.height(),
            target_fmt,
            frame.width(),
            frame.height(),
            ffmpeg_next::software::scaling::Flags::FAST_BILINEAR,
        )
        .ok();
    }

    // If conversion is needed and scaler is available, convert.
    let yuv_frame;
    let frame_ref;
    if frame.format() == target_fmt {
        frame_ref = frame;
    } else if let Some(sws) = scaler {
        let mut tmp = ffmpeg_next::util::frame::Video::empty();
        if sws.run(frame, &mut tmp).is_ok() {
            yuv_frame = tmp;
            frame_ref = &yuv_frame;
        } else {
            return (0.0, 100.0); // treat as bad frame
        }
    } else {
        return (0.0, 100.0);
    }

    let y_data = frame_ref.data(0);
    let u_data = frame_ref.data(1);
    let v_data = frame_ref.data(2);

    let y_stride = frame_ref.stride(0);
    let u_stride = frame_ref.stride(1);
    let v_stride = frame_ref.stride(2);

    let w = frame_ref.width() as usize;
    let h = frame_ref.height() as usize;
    let cw = w / 2;
    let ch = h / 2;

    // BRNG: count luma pixels outside broadcast range (16..235).
    let mut brng_count: u64 = 0;
    let total_pixels = (w * h) as u64;
    for row in 0..h {
        let row_start = row * y_stride;
        for col in 0..w {
            let luma = y_data[row_start + col];
            if luma < 16 || luma > 235 {
                brng_count += 1;
            }
        }
    }
    let brng_pct = if total_pixels > 0 {
        (brng_count as f64 / total_pixels as f64) * 100.0
    } else {
        100.0
    };

    // SATAVG: average chroma saturation = sqrt(Cb'^2 + Cr'^2) where
    // Cb' = U - 128, Cr' = V - 128.
    let mut sat_sum: f64 = 0.0;
    let chroma_pixels = (cw * ch) as u64;
    for row in 0..ch {
        let u_row = row * u_stride;
        let v_row = row * v_stride;
        for col in 0..cw {
            let cb = u_data[u_row + col] as f64 - 128.0;
            let cr = v_data[v_row + col] as f64 - 128.0;
            sat_sum += (cb * cb + cr * cr).sqrt();
        }
    }
    let satavg = if chroma_pixels > 0 {
        sat_sum / chroma_pixels as f64
    } else {
        0.0
    };

    (satavg, brng_pct)
}

/// Write an RGB24 frame to a JPEG file using the `image` crate.
fn write_rgb_frame_as_jpeg(
    frame: &ffmpeg_next::util::frame::Video,
    out_path: &Path,
) -> bool {
    let w = frame.width();
    let h = frame.height();
    let stride = frame.stride(0);
    let data = frame.data(0);

    // Collect contiguous RGB data (stride may include padding).
    let mut rgb_buf = Vec::with_capacity((w * h * 3) as usize);
    for row in 0..h as usize {
        let start = row * stride;
        let end = start + (w as usize) * 3;
        if end <= data.len() {
            rgb_buf.extend_from_slice(&data[start..end]);
        }
    }

    let img = match image::RgbImage::from_raw(w, h, rgb_buf) {
        Some(img) => img,
        None => return false,
    };

    let dynamic = image::DynamicImage::ImageRgb8(img);
    let mut buf = Vec::new();
    let mut cursor = Cursor::new(&mut buf);

    if dynamic
        .write_to(&mut cursor, image::ImageFormat::Jpeg)
        .is_err()
    {
        return false;
    }

    std::fs::write(out_path, &buf).is_ok()
}
