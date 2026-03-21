// Port of dash.js/src/core/Logger.js + Debug.js
//
// A thin wrapper around the `log` crate with dash.js log-level semantics.

use serde::{Deserialize, Serialize};

/// Log levels matching dash.js `Debug.LOG_LEVEL_*` constants.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[repr(u8)]
pub enum LogLevel {
    /// No logging at all.
    None = 0,
    /// Fatal errors only (playback failure).
    Fatal = 1,
    /// All errors.
    Error = 2,
    /// Warnings and above.
    Warning = 3,
    /// Informational messages and above.
    Info = 4,
    /// Debug (most verbose).
    Debug = 5,
}

impl LogLevel {
    /// Convert a numeric level to a [`LogLevel`].
    pub fn from_u8(n: u8) -> Self {
        match n {
            0 => Self::None,
            1 => Self::Fatal,
            2 => Self::Error,
            3 => Self::Warning,
            4 => Self::Info,
            _ => Self::Debug,
        }
    }
}

impl Default for LogLevel {
    fn default() -> Self {
        Self::Warning
    }
}

/// A simple component logger.
///
/// Each logger carries a tag (component name) and a current level.
/// Messages below the configured level are silently dropped.
#[derive(Clone, Debug)]
pub struct Logger {
    tag: String,
    level: LogLevel,
}

impl Logger {
    pub fn new(tag: impl Into<String>) -> Self {
        Self {
            tag: tag.into(),
            level: LogLevel::default(),
        }
    }

    pub fn with_level(mut self, level: LogLevel) -> Self {
        self.level = level;
        self
    }

    pub fn set_level(&mut self, level: LogLevel) {
        self.level = level;
    }

    pub fn level(&self) -> LogLevel {
        self.level
    }

    pub fn fatal(&self, msg: &str) {
        if self.level >= LogLevel::Fatal {
            log::error!("[{}] FATAL: {}", self.tag, msg);
        }
    }

    pub fn error(&self, msg: &str) {
        if self.level >= LogLevel::Error {
            log::error!("[{}] {}", self.tag, msg);
        }
    }

    pub fn warn(&self, msg: &str) {
        if self.level >= LogLevel::Warning {
            log::warn!("[{}] {}", self.tag, msg);
        }
    }

    pub fn info(&self, msg: &str) {
        if self.level >= LogLevel::Info {
            log::info!("[{}] {}", self.tag, msg);
        }
    }

    pub fn debug(&self, msg: &str) {
        if self.level >= LogLevel::Debug {
            log::debug!("[{}] {}", self.tag, msg);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn log_level_ordering() {
        assert!(LogLevel::Debug > LogLevel::Warning);
        assert!(LogLevel::None < LogLevel::Fatal);
    }

    #[test]
    fn log_level_from_u8() {
        assert_eq!(LogLevel::from_u8(0), LogLevel::None);
        assert_eq!(LogLevel::from_u8(3), LogLevel::Warning);
        assert_eq!(LogLevel::from_u8(99), LogLevel::Debug);
    }

    #[test]
    fn logger_creation() {
        let logger = Logger::new("TestComponent").with_level(LogLevel::Debug);
        assert_eq!(logger.level(), LogLevel::Debug);
    }
}
