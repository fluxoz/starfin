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
/// Creates `{sprite_dir}/sprite.jpg` using a single sequential pass through
/// the file.  Writes to a temp file first for atomicity.
///
/// ## Performance optimisation — GOP-level skip
///
/// Instead of decoding every frame in the file, we skip non-keyframe packets
/// that are clearly past the next thumbnail timestamp.  This limits decoding
/// to roughly one keyframe interval worth of frames per thumbnail rather than
/// the entire video, without any seeking (and the associated I/O overhead and
/// re-decoding of the same GOPs that seek-per-thumbnail would cause).
///
/// Concretely:
/// * Every I-frame packet is always sent to the decoder (it may be the key
///   frame that a thumbnail depends on).
/// * Non-I-frame packets whose DTS is more than one second past the next
///   thumbnail timestamp are skipped; the decoder is flushed at the next
///   I-frame so it does not try to use the missing reference frames.
///
/// This saves decoding the tail of each GOP that falls after the last
/// thumbnail in that GOP — roughly 20–40 % fewer decoded frames for typical
/// 10-second keyframe intervals.
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

    // Capture time_base and build decoder in a scoped block so the `stream`
    // borrow of `ictx` is released before we start the packet iterator.
    let (time_base_num, time_base_den, mut decoder) = {
        let stream = ictx.stream(stream_idx).unwrap();
        let tb = stream.time_base();
        let ctx = match ffmpeg_next::codec::context::Context::from_parameters(stream.parameters())
        {
            Ok(c) => c,
            Err(_) => return false,
        };
        let dec = match ctx.decoder().video() {
            Ok(d) => d,
            Err(_) => return false,
        };
        (tb.0 as f64, tb.1 as f64, dec)
        // `stream` borrow released here.
    };

    let mut scaler: Option<ffmpeg_next::software::scaling::Context> = None;
    let mut thumb_index: u32 = 0;
    let mut next_sample_time: f64 = 0.0;
    let mut rgb_frame = ffmpeg_next::util::frame::Video::empty();
    // Whether we are currently sending packets to the decoder.  Starts true
    // because the first thumbnail is at t = 0.
    let mut decoding = true;

    for (pkt_stream, packet) in ictx.packets() {
        if thumb_index >= num_thumbnails {
            break;
        }
        if pkt_stream.index() != stream_idx {
            continue;
        }

        // Compute the packet timestamp in seconds (prefer DTS for monotonicity).
        // Fall back to 0.0 so that packets without a usable timestamp are never
        // incorrectly skipped by the non-key-frame check below.
        let pkt_secs = packet
            .dts()
            .or_else(|| packet.pts())
            .map(|ts| ts as f64 * time_base_num / time_base_den)
            .unwrap_or(0.0);

        if packet.is_key() {
            // At every I-frame, (re-)enable decoding.  If we had been skipping
            // non-key frames, flush the decoder first so it does not attempt to
            // use the reference frames we discarded.
            if !decoding {
                decoder.flush();
                decoding = true;
            }
        } else {
            // Non-key frame: skip if its timestamp is more than 1 s past the
            // next thumbnail.  The 1 s margin avoids off-by-one issues with
            // PTS/DTS jitter and ensures we get a frame *at* next_sample_time.
            if pkt_secs > next_sample_time + 1.0 {
                decoding = false;
                continue;
            }
        }

        if !decoding {
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

            // Use pts() first; fall back to timestamp() (best_effort_timestamp)
            // which ffmpeg estimates when the container PTS is missing.
            // Skip frames where no usable timestamp is available rather than
            // defaulting to 0, which could place them at the wrong position.
            let pts_secs = match decoded.pts().or_else(|| decoded.timestamp()) {
                Some(pts) => pts as f64 * time_base_num / time_base_den,
                None => continue,
            };

            if pts_secs < next_sample_time {
                continue;
            }
            next_sample_time = pts_secs + THUMBNAIL_INTERVAL;

            // Initialise scaler on the first usable frame.
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

            // Copy the scaled frame into the correct tile using row-wise bulk
            // copies for efficiency.
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
