//! Sprite sheet generation — replaces the `generate_sprite()` subprocess call
//! with in-process frame extraction and compositing via `ffmpeg-next` and the
//! `image` crate.
//!
//! A sprite sheet is a single JPEG that tiles many small thumbnails in a grid.
//! The scrubber overlay reads tile coordinates to show a preview at any
//! position in the timeline.

use std::io::Cursor;
use std::path::Path;

/// Thumbnail interval in seconds (one thumbnail every N seconds).
pub const THUMBNAIL_INTERVAL: f64 = 10.0;
/// Width of each individual thumbnail in the sprite.
pub const THUMBNAIL_WIDTH: u32 = 640;
/// Height of each individual thumbnail in the sprite.
pub const THUMBNAIL_HEIGHT: u32 = 360;
/// Maximum thumbnails per row in the sprite grid.
pub const THUMBNAILS_PER_ROW: u32 = 10;

/// Generate a thumbnail sprite sheet for a video.
///
/// Creates `{sprite_dir}/sprite.jpg` by **seeking** to each sample position
/// and decoding one frame per thumbnail, then scaling and compositing into a
/// tiled grid.  Writes to a temp file first for atomicity.
///
/// Seeking rather than sequential packet reading matches what the old
/// `ffmpeg -vf fps=…,tile=…` subprocess did, and avoids the multi-minute
/// decode-all-frames path that previously stalled sprite generation for any
/// video longer than a few minutes.
///
/// Returns `true` on success.
pub fn generate_sprite_sheet(
    video_path: &Path,
    duration_secs: u32,
    sprite_dir: &Path,
) -> bool {
    super::ensure_init();

    if duration_secs == 0 {
        return false;
    }

    let sprite_path = sprite_dir.join("sprite.jpg");
    if sprite_path.exists() {
        return true;
    }

    let video_str = match video_path.to_str() {
        Some(s) => s,
        None => return false,
    };

    let duration = duration_secs as f64;
    let num_thumbnails = ((duration / THUMBNAIL_INTERVAL).ceil() as u32).max(1);
    let columns = THUMBNAILS_PER_ROW.min(num_thumbnails);
    let rows = ((num_thumbnails as f64) / (columns as f64)).ceil() as u32;

    // Total sprite dimensions.
    let sprite_w = columns * THUMBNAIL_WIDTH;
    let sprite_h = rows * THUMBNAIL_HEIGHT;

    // Create the composite image buffer (RGB).
    let mut sprite_img = image::RgbImage::new(sprite_w, sprite_h);

    // Open input and find video stream.
    let mut ictx = match ffmpeg_next::format::input(&video_str) {
        Ok(ctx) => ctx,
        Err(_) => return false,
    };

    let stream_idx = match ictx.streams().best(ffmpeg_next::media::Type::Video) {
        Some(s) => s.index(),
        None => return false,
    };

    // Build the decoder.  The stream borrow must be dropped before we can
    // seek/iterate on ictx below.
    let decoder_ctx = {
        let stream = ictx.stream(stream_idx).unwrap();
        match ffmpeg_next::codec::context::Context::from_parameters(stream.parameters()) {
            Ok(ctx) => ctx,
            Err(_) => return false,
        }
        // `stream` (and its borrow of `ictx`) is dropped here.
    };
    let mut decoder = match decoder_ctx.decoder().video() {
        Ok(d) => d,
        Err(_) => return false,
    };

    let mut scaler: Option<ffmpeg_next::software::scaling::Context> = None;
    let mut thumb_count: u32 = 0;
    let mut rgb_frame = ffmpeg_next::util::frame::Video::empty();

    for i in 0..num_thumbnails {
        let seek_secs = i as f64 * THUMBNAIL_INTERVAL;
        let ts = (seek_secs * f64::from(ffmpeg_next::ffi::AV_TIME_BASE)) as i64;

        // Seek to the target position.  Ignore seek errors — the demuxer will
        // just continue from its current position.
        let _ = ictx.seek(ts, ..ts);

        // Flush any frames buffered in the decoder from before the seek.
        decoder.flush();

        // Read packets until we successfully decode and blit one frame.
        'pkt: for (pkt_stream, packet) in ictx.packets() {
            if pkt_stream.index() != stream_idx {
                continue;
            }
            if decoder.send_packet(&packet).is_err() {
                continue;
            }

            let mut decoded = ffmpeg_next::util::frame::Video::empty();
            while decoder.receive_frame(&mut decoded).is_ok() {
                // Initialise (or re-check) the scaler on the first decoded frame.
                if scaler.is_none() {
                    scaler = ffmpeg_next::software::scaling::Context::get(
                        decoded.format(),
                        decoded.width(),
                        decoded.height(),
                        ffmpeg_next::format::Pixel::RGB24,
                        THUMBNAIL_WIDTH,
                        THUMBNAIL_HEIGHT,
                        ffmpeg_next::software::scaling::Flags::BILINEAR,
                    )
                    .ok();
                }

                let Some(ref mut sws) = scaler else {
                    continue;
                };

                if sws.run(&decoded, &mut rgb_frame).is_err() {
                    continue;
                }

                // Copy the scaled RGB frame into the correct tile in the sprite
                // using row-wise bulk copies for efficiency.
                let col = thumb_count % columns;
                let row = thumb_count / columns;
                let x_offset = col * THUMBNAIL_WIDTH;
                let y_offset = row * THUMBNAIL_HEIGHT;

                let stride = rgb_frame.stride(0);
                let data = rgb_frame.data(0);
                let row_bytes = THUMBNAIL_WIDTH as usize * 3;
                let sprite_row_bytes = sprite_w as usize * 3;
                let sprite_bytes = sprite_img.as_mut();
                let base_dst = y_offset as usize * sprite_row_bytes + x_offset as usize * 3;

                for y in 0..THUMBNAIL_HEIGHT {
                    let src_start = y as usize * stride;
                    if src_start + row_bytes > data.len() {
                        break;
                    }
                    let dst_start = base_dst + y as usize * sprite_row_bytes;
                    if dst_start + row_bytes > sprite_bytes.len() {
                        break;
                    }
                    sprite_bytes[dst_start..dst_start + row_bytes]
                        .copy_from_slice(&data[src_start..src_start + row_bytes]);
                }

                thumb_count += 1;
                break 'pkt;
            }
        }
        // If no frame was found for this position (e.g. seek past end of file),
        // leave the tile black and continue.
    }

    if thumb_count == 0 {
        return false;
    }

    // Encode the composite as JPEG and write atomically.
    let tmp_path = sprite_dir.join("sprite.tmp.jpg");
    let dynamic = image::DynamicImage::ImageRgb8(sprite_img);
    let mut buf = Vec::new();
    let mut cursor = Cursor::new(&mut buf);

    if dynamic
        .write_to(&mut cursor, image::ImageFormat::Jpeg)
        .is_err()
    {
        return false;
    }

    if std::fs::write(&tmp_path, &buf).is_err() {
        let _ = std::fs::remove_file(&tmp_path);
        return false;
    }

    if std::fs::rename(&tmp_path, &sprite_path).is_err() {
        let _ = std::fs::remove_file(&tmp_path);
        return false;
    }

    true
}
