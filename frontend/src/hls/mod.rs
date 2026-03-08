//! HTTP Live Streaming (HLS) implementation for Yew/Rust
//!
//! This module provides a complete HLS player implementation with feature parity
//! with HLS.js. Features include:
//!
//! - Fragmented MP4 (fMP4/CMAF) container support
//! - AAC container for audio-only streams
//! - MPEG Audio container for audio-only streams
//! - Timed Metadata (ID3, Emsg, DATERANGE)
//! - Level capping based on resolution, dropped-frames, and HDCP
//! - Adaptive bitrate streaming with multiple quality switching modes
//! - Accurate seeking with buffer management
//! - Error resilience with retry mechanism and recovery actions

pub mod playlist;
pub mod loader;
pub mod abr;
pub mod buffer;
pub mod metadata;
pub mod error;
pub mod config;
pub mod controller;
pub mod events;

pub use config::HlsConfig;
pub use controller::HlsController;
pub use error::{HlsError, HlsResult};
pub use events::{HlsEvent, HlsEventHandler};
pub use playlist::{Level, MasterPlaylist, MediaPlaylist, Segment};
pub use config::QualitySwitchMode;
pub use abr::{AbrMode, AbrController};
