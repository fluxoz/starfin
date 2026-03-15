//! Sprite sheet generation via `ffmpeg` subprocess.
//!
//! A sprite sheet is a single JPEG that tiles many small thumbnails in a grid.
//! The scrubber overlay reads tile coordinates to show a preview at any
//! position in the timeline.
//!
//! The actual subprocess invocation lives in `generate_sprite()` in `main.rs`
//! which uses `tokio::process::Command` for async management (interruptible
//! via kill signal, proper child-process cleanup).
//!
//! The `ffmpeg` filter chain `fps → scale → tile` is the approach used by
//! Jellyfin, Plex, and other media servers, and is dramatically faster than
//! in-process frame-by-frame decoding because ffmpeg internally uses:
//!
//! * Multi-threaded frame-level decoding
//! * SIMD-optimised scale and colour-conversion kernels
//! * Efficient filter-graph frame dropping (the `fps` filter discards frames
//!   at the codec level — much cheaper than decode-then-discard)
//! * Native `tile` compositing without intermediate RGB copies
//!
//! A 2-hour 1080p movie typically completes in 15–60 seconds.

/// Thumbnail interval in seconds (one thumbnail every N seconds).
pub const THUMBNAIL_INTERVAL: f64 = 10.0;
/// Width of each individual thumbnail in the sprite.
pub const THUMBNAIL_WIDTH: u32 = 640;
/// Height of each individual thumbnail in the sprite.
pub const THUMBNAIL_HEIGHT: u32 = 360;
/// Maximum thumbnails per row in the sprite grid.
pub const THUMBNAILS_PER_ROW: u32 = 10;
