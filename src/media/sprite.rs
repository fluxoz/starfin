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
/// Creates `{sprite_dir}/sprite.jpg` by decoding frames at regular intervals,
/// scaling them, and compositing into a tiled grid.  Writes to a temp file
/// first for atomicity.
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

    let stream = ictx.stream(stream_idx).unwrap();
    let time_base = stream.time_base();

    let decoder_ctx = match ffmpeg_next::codec::context::Context::from_parameters(stream.parameters()) {
        Ok(ctx) => ctx,
        Err(_) => return false,
    };
    let mut decoder = match decoder_ctx.decoder().video() {
        Ok(d) => d,
        Err(_) => return false,
    };

    let mut scaler: Option<ffmpeg_next::software::scaling::Context> = None;
    let mut thumb_index: u32 = 0;
    let mut next_sample_time: f64 = 0.0;
    let mut rgb_frame = ffmpeg_next::util::frame::Video::empty();

    for (pkt_stream, packet) in ictx.packets() {
        if thumb_index >= num_thumbnails {
            break;
        }
        if pkt_stream.index() != stream_idx {
            continue;
        }
        if decoder.send_packet(&packet).is_err() {
            continue;
        }

        let mut decoded = ffmpeg_next::util::frame::Video::empty();
        while decoder.receive_frame(&mut decoded).is_ok() {
            if thumb_index >= num_thumbnails {
                break;
            }

            let pts = decoded.pts().unwrap_or(0);
            let pts_secs = pts as f64 * f64::from(time_base.0) / f64::from(time_base.1);

            if pts_secs < next_sample_time {
                continue;
            }
            next_sample_time = pts_secs + THUMBNAIL_INTERVAL;

            // Initialise scaler on first usable frame.
            if scaler.is_none() {
                scaler = ffmpeg_next::software::scaling::Context::get(
                    decoded.format(),
                    decoded.width(),
                    decoded.height(),
                    ffmpeg_next::format::Pixel::RGB24,
                    THUMBNAIL_WIDTH,
                    THUMBNAIL_HEIGHT,
                    ffmpeg_next::software::scaling::Flags::FAST_BILINEAR,
                )
                .ok();
            }

            let Some(ref mut sws) = scaler else {
                continue;
            };

            if sws.run(&decoded, &mut rgb_frame).is_err() {
                continue;
            }

            // Copy the scaled frame into the sprite at the correct tile position.
            let col = thumb_index % columns;
            let row = thumb_index / columns;
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

            thumb_index += 1;
        }
    }

    if thumb_index == 0 {
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
