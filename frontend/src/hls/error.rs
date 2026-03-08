//! HLS error types and handling

use std::fmt;

/// Result type for HLS operations
pub type HlsResult<T> = Result<T, HlsError>;

/// Categories of HLS errors for recovery handling
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ErrorCategory {
    /// Network-related errors (retryable)
    Network,
    /// Media/codec errors
    Media,
    /// Manifest/playlist parsing errors
    Manifest,
    /// Internal errors
    Internal,
    /// Browser/platform capability errors
    Capability,
}

/// Severity of the error
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ErrorSeverity {
    /// Warning - playback may continue
    Warning,
    /// Error - requires recovery action
    Error,
    /// Fatal - playback cannot continue
    Fatal,
}

/// HLS error types
#[derive(Clone, Debug)]
pub struct HlsError {
    /// Error category
    pub category: ErrorCategory,
    /// Error severity
    pub severity: ErrorSeverity,
    /// Error code for identification
    pub code: ErrorCode,
    /// Human-readable message
    pub message: String,
    /// Additional details
    pub details: Option<String>,
    /// Whether this error is fatal
    pub fatal: bool,
    /// Number of retry attempts made
    pub retry_count: u32,
}

/// Error codes for specific error types
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ErrorCode {
    // Network errors (100-199)
    NetworkError = 100,
    ManifestLoadError = 101,
    ManifestLoadTimeout = 102,
    ManifestParsingError = 103,
    LevelLoadError = 110,
    LevelLoadTimeout = 111,
    FragLoadError = 120,
    FragLoadTimeout = 121,
    FragParsingError = 122,
    KeyLoadError = 130,
    KeyLoadTimeout = 131,
    
    // Media errors (200-299)
    MediaAppendError = 200,
    MediaDecodeError = 201,
    MediaSourceError = 202,
    BufferAppendError = 210,
    BufferRemoveError = 211,
    BufferFullError = 212,
    BufferStalledError = 213,
    
    // Manifest/level errors (300-399)
    ManifestIncompatible = 300,
    LevelSwitchError = 301,
    NoLevelFound = 302,
    InvalidLevelIndex = 303,
    PlaylistUnchanged = 304,
    
    // Internal errors (400-499)
    InternalError = 400,
    InternalException = 401,
    
    // Capability errors (500-599)
    MseNotSupported = 500,
    CodecNotSupported = 501,
    DrmNotSupported = 502,
}

impl HlsError {
    /// Create a new network error
    pub fn network(code: ErrorCode, message: impl Into<String>) -> Self {
        Self {
            category: ErrorCategory::Network,
            severity: ErrorSeverity::Error,
            code,
            message: message.into(),
            details: None,
            fatal: false,
            retry_count: 0,
        }
    }
    
    /// Create a new media error
    pub fn media(code: ErrorCode, message: impl Into<String>) -> Self {
        Self {
            category: ErrorCategory::Media,
            severity: ErrorSeverity::Error,
            code,
            message: message.into(),
            details: None,
            fatal: false,
            retry_count: 0,
        }
    }
    
    /// Create a new manifest error
    pub fn manifest(code: ErrorCode, message: impl Into<String>) -> Self {
        Self {
            category: ErrorCategory::Manifest,
            severity: ErrorSeverity::Error,
            code,
            message: message.into(),
            details: None,
            fatal: false,
            retry_count: 0,
        }
    }
    
    /// Create a new capability error (typically fatal)
    pub fn capability(code: ErrorCode, message: impl Into<String>) -> Self {
        Self {
            category: ErrorCategory::Capability,
            severity: ErrorSeverity::Fatal,
            code,
            message: message.into(),
            details: None,
            fatal: true,
            retry_count: 0,
        }
    }
    
    /// Create an internal error
    pub fn internal(message: impl Into<String>) -> Self {
        Self {
            category: ErrorCategory::Internal,
            severity: ErrorSeverity::Error,
            code: ErrorCode::InternalError,
            message: message.into(),
            details: None,
            fatal: false,
            retry_count: 0,
        }
    }
    
    /// Mark error as fatal
    pub fn with_fatal(mut self, fatal: bool) -> Self {
        self.fatal = fatal;
        if fatal {
            self.severity = ErrorSeverity::Fatal;
        }
        self
    }
    
    /// Add details to the error
    pub fn with_details(mut self, details: impl Into<String>) -> Self {
        self.details = Some(details.into());
        self
    }
    
    /// Set retry count
    pub fn with_retry_count(mut self, count: u32) -> Self {
        self.retry_count = count;
        self
    }
    
    /// Check if error is recoverable
    pub fn is_recoverable(&self) -> bool {
        !self.fatal && matches!(self.category, ErrorCategory::Network | ErrorCategory::Media)
    }
    
    /// Check if error should trigger retry
    pub fn should_retry(&self, max_retries: u32) -> bool {
        self.is_recoverable() && self.retry_count < max_retries
    }
}

impl fmt::Display for HlsError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "[HLS {:?}] {}", self.code, self.message)?;
        if let Some(details) = &self.details {
            write!(f, " ({})", details)?;
        }
        Ok(())
    }
}

impl std::error::Error for HlsError {}

/// Recovery action to take for an error
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum RecoveryAction {
    /// No recovery needed
    None,
    /// Retry the failed operation
    Retry,
    /// Switch to a different quality level
    SwitchLevel(i32),
    /// Seek to recover from stall
    SeekToRecover,
    /// Reset and restart playback
    ResetPlayback,
    /// Reload the manifest
    ReloadManifest,
    /// Destroy and recreate media source
    RecreateMediaSource,
}

impl HlsError {
    /// Get recommended recovery action for this error
    pub fn recommended_recovery(&self) -> RecoveryAction {
        match self.code {
            ErrorCode::FragLoadError | ErrorCode::FragLoadTimeout if !self.fatal => {
                RecoveryAction::Retry
            }
            ErrorCode::LevelLoadError | ErrorCode::LevelLoadTimeout if !self.fatal => {
                RecoveryAction::SwitchLevel(-1) // Auto level
            }
            ErrorCode::ManifestLoadError | ErrorCode::ManifestLoadTimeout if !self.fatal => {
                RecoveryAction::ReloadManifest
            }
            ErrorCode::BufferStalledError => RecoveryAction::SeekToRecover,
            ErrorCode::MediaDecodeError | ErrorCode::MediaAppendError => {
                RecoveryAction::RecreateMediaSource
            }
            ErrorCode::BufferFullError => RecoveryAction::SwitchLevel(-1),
            _ if self.fatal => RecoveryAction::None,
            _ => RecoveryAction::Retry,
        }
    }
}
