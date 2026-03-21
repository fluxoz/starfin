//! Port of `dash.js/src/streaming/protection/`.
//!
//! DRM/EME protection stubs for future extension.

use serde::{Deserialize, Serialize};

/// Key system types supported by dash.js.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum KeySystem {
    Widevine,
    PlayReady,
    ClearKey,
    FairPlay,
    PrimeTime,
    Other(String),
}

/// Protection configuration.
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct ProtectionConfig {
    pub key_system: Option<String>,
    pub server_url: Option<String>,
    pub server_certificate: Option<Vec<u8>>,
    pub clear_keys: Option<std::collections::HashMap<String, String>>,
    pub robustness: Option<String>,
}

/// Protection controller stub.
#[derive(Clone, Debug, Default)]
pub struct ProtectionController {
    _initialized: bool,
}

impl ProtectionController {
    pub fn new() -> Self { Self::default() }
    pub fn initialize(&mut self) { self._initialized = true; }
    pub fn reset(&mut self) { self._initialized = false; }
    pub fn is_initialized(&self) -> bool { self._initialized }
}

/// Protection events (matches dash.js ProtectionEvents.js).
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ProtectionEvent {
    KeySystemSelected,
    KeySessionCreated,
    KeySessionClosed,
    KeySessionRemoved,
    KeyStatusesChanged,
    LicenseRequestComplete,
    NeedKey,
    ServerCertificateUpdated,
}
