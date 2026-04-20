//! Thumbnail extraction — fast single-frame grab at 20% of the video duration,
//! similar to the approach used by KDE's `ffmpegthumbs`.

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
