//! In-process media handling powered by `ffmpeg-next` (Rust FFI bindings to
//! libavcodec / libavformat / libavfilter / libswscale).
//!
//! This module replaces every `Command::new("ffmpeg")` and
//! `Command::new("ffprobe")` subprocess call that the old backend used, giving
//! us unified Rust-native error handling, zero subprocess overhead, and the
//! ability to produce fully static binaries.

pub mod hwaccel;
pub mod probe;
pub mod sprite;
pub mod subtitle;
pub mod thumbnail;
pub mod transcode;

use std::sync::Once;

static FFMPEG_INIT: Once = Once::new();

/// Initialise the ffmpeg libraries exactly once.  Safe to call from multiple
/// threads — the inner `Once` guard prevents double-init.
pub fn ensure_init() {
    FFMPEG_INIT.call_once(|| {
        ffmpeg_next::init().expect("failed to initialise ffmpeg libraries");
    });
}

/// Return the version string baked into the linked libavcodec (e.g.
/// "60.31.102").  Useful for startup healthcheck logging.
pub fn libavcodec_version_string() -> String {
    let v = ffmpeg_next::codec::version();
    format!("{}.{}.{}", (v >> 16) & 0xFF, (v >> 8) & 0xFF, v & 0xFF)
}

/// Return the version string baked into the linked libavformat.
pub fn libavformat_version_string() -> String {
    let v = ffmpeg_next::format::version();
    format!("{}.{}.{}", (v >> 16) & 0xFF, (v >> 8) & 0xFF, v & 0xFF)
}

/// Return the version string of the linked libavfilter.
pub fn libavfilter_version_string() -> String {
    let v = unsafe { ffmpeg_next::ffi::avfilter_version() };
    format!("{}.{}.{}", (v >> 16) & 0xFF, (v >> 8) & 0xFF, v & 0xFF)
}
