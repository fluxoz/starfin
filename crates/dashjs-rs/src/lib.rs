//! # dashjs-rs
//!
//! A feature-complete Rust port of [Dash.js](https://github.com/Dash-Industry-Forum/dash.js) —
//! the MPEG-DASH adaptive streaming reference player.

pub mod core;
pub mod dash;
pub mod mss;
pub mod offline;
pub mod streaming;

pub use crate::core::errors::{DashError, ErrorCode};
pub use crate::core::event_bus::EventBus;
pub use crate::core::events::Event;
pub use crate::core::logger::LogLevel;
pub use crate::core::settings::Settings;
