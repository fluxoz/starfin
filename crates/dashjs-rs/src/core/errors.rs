// Port of dash.js/src/core/errors/Errors.js + ErrorsBase.js
//
// Strongly-typed error codes and a unified DashError type.

use serde::{Deserialize, Serialize};
use std::fmt;

/// Numeric error codes matching the dash.js `Errors` class.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[repr(u32)]
pub enum ErrorCode {
    // ── Manifest errors (10–12) ──────────────────────────────────────────
    /// Manifest parsing failed
    ManifestLoaderParsingFailure = 10,
    /// Manifest loading failed
    ManifestLoaderLoadingFailure = 11,
    /// XLink loading failed
    XlinkLoaderLoadingFailure = 12,

    // ── Segment / fragment errors (15–22) ────────────────────────────────
    /// No segment ranges could be determined from the sidx box
    SegmentBaseLoaderError = 15,
    /// Time synchronization failed
    TimeSyncFailed = 16,
    /// Fragment loading failed
    FragmentLoaderLoadingFailure = 17,
    /// FragmentLoader received a null request
    FragmentLoaderNullRequest = 18,
    /// BaseURL resolution failed
    UrlResolutionFailedGeneric = 19,
    /// SourceBuffer append operation failed
    AppendError = 20,
    /// SourceBuffer remove operation failed
    RemoveError = 21,
    /// Updating internal objects after MPD load failed
    DataUpdateFailed = 22,

    // ── Capability errors (23–24) ────────────────────────────────────────
    /// MediaSource is not supported
    CapabilityMediaSource = 23,
    /// Protected content (MediaKeys) is not supported
    CapabilityMediaKeys = 24,

    // ── Download errors (25–29) ──────────────────────────────────────────
    /// Manifest download failed
    DownloadErrorManifest = 25,
    /// SIDX download failed
    DownloadErrorSidx = 26,
    /// Content download failed
    DownloadErrorContent = 27,
    /// Initialization segment download failed
    DownloadErrorInitialization = 28,
    /// XLink content download failed
    DownloadErrorXlink = 29,

    // ── Parsing errors (31–34) ───────────────────────────────────────────
    /// MPD parsing resulted in a logical error
    ManifestErrorParse = 31,
    /// No stream (period) detected in manifest
    ManifestErrorNoStreams = 32,
    /// Subtitle (TTML/VTT) parsing or appending failed
    TimedTextErrorParse = 33,
    /// Muxed media type detected (unsupported)
    ManifestErrorMultiplexed = 34,

    // ── Type-unsupported errors (35–36) ──────────────────────────────────
    /// MediaSource type unsupported
    MediaSourceTypeUnsupported = 35,
    /// No usable key IDs (all keys have invalid status)
    NoSupportedKeyIds = 36,
}

impl ErrorCode {
    /// The default human-readable message for this error code.
    pub fn default_message(self) -> &'static str {
        match self {
            Self::ManifestLoaderParsingFailure => "parsing failed for ",
            Self::ManifestLoaderLoadingFailure => "Failed loading manifest: ",
            Self::XlinkLoaderLoadingFailure => "Failed loading Xlink element: ",
            Self::SegmentBaseLoaderError => "error loading segment ranges from sidx",
            Self::TimeSyncFailed => "Failed to synchronize client and server time",
            Self::FragmentLoaderLoadingFailure => "Fragment loading failed",
            Self::FragmentLoaderNullRequest => "request is null",
            Self::UrlResolutionFailedGeneric => "Failed to resolve a valid URL",
            Self::AppendError => "chunk is not defined",
            Self::RemoveError => "Removing data from the SourceBuffer",
            Self::DataUpdateFailed => "Data update failed",
            Self::CapabilityMediaSource => "mediasource is not supported",
            Self::CapabilityMediaKeys => "mediakeys is not supported",
            Self::DownloadErrorManifest => "Download error: manifest",
            Self::DownloadErrorSidx => "Download error: sidx",
            Self::DownloadErrorContent => "Download error: content",
            Self::DownloadErrorInitialization => "Download error: initialization",
            Self::DownloadErrorXlink => "Download error: xlink",
            Self::ManifestErrorParse => "Manifest parsing error",
            Self::ManifestErrorNoStreams => "No streams found in manifest",
            Self::TimedTextErrorParse => "parsing error :",
            Self::ManifestErrorMultiplexed => "Multiplexed content is not supported",
            Self::MediaSourceTypeUnsupported => "Error creating source buffer of type : ",
            Self::NoSupportedKeyIds => {
                "All possible Adaptation Sets have an invalid key status"
            }
        }
    }
}

impl fmt::Display for ErrorCode {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{} (code {})", self.default_message(), *self as u32)
    }
}

/// A structured error emitted by the player.
///
/// Mirrors the `{code, message, data}` shape from dash.js error events.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct DashError {
    /// Numeric error code.
    pub code: ErrorCode,
    /// Human-readable description.
    pub message: String,
    /// Optional serialised context data (JSON value).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub data: Option<serde_json::Value>,
}

impl DashError {
    /// Create a new error with the default message for the given code.
    pub fn new(code: ErrorCode) -> Self {
        Self {
            message: code.default_message().to_owned(),
            code,
            data: None,
        }
    }

    /// Create a new error with a custom message.
    pub fn with_message(code: ErrorCode, message: impl Into<String>) -> Self {
        Self {
            code,
            message: message.into(),
            data: None,
        }
    }

    /// Attach optional context data.
    pub fn with_data(mut self, data: serde_json::Value) -> Self {
        self.data = Some(data);
        self
    }
}

impl fmt::Display for DashError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "[{}] {}", self.code as u32, self.message)
    }
}

impl std::error::Error for DashError {}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn error_code_values() {
        assert_eq!(ErrorCode::ManifestLoaderParsingFailure as u32, 10);
        assert_eq!(ErrorCode::NoSupportedKeyIds as u32, 36);
    }

    #[test]
    fn dash_error_display() {
        let e = DashError::new(ErrorCode::TimeSyncFailed);
        assert!(e.to_string().contains("16"));
    }

    #[test]
    fn dash_error_serialization() {
        let e = DashError::new(ErrorCode::AppendError)
            .with_data(serde_json::json!({"detail": "quota exceeded"}));
        let json = serde_json::to_string(&e).unwrap();
        assert!(json.contains("\"code\""));
        assert!(json.contains("quota exceeded"));
    }
}
