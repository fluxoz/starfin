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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn key_system_variants() {
        let ks = KeySystem::Widevine;
        assert_eq!(ks, KeySystem::Widevine);
        assert_ne!(ks, KeySystem::PlayReady);
        let other = KeySystem::Other("custom.drm".into());
        assert_eq!(other, KeySystem::Other("custom.drm".into()));
    }

    #[test]
    fn key_system_clone() {
        let ks = KeySystem::ClearKey;
        let ks2 = ks.clone();
        assert_eq!(ks, ks2);
    }

    #[test]
    fn protection_config_defaults() {
        let cfg = ProtectionConfig::default();
        assert!(cfg.key_system.is_none());
        assert!(cfg.server_url.is_none());
        assert!(cfg.server_certificate.is_none());
        assert!(cfg.clear_keys.is_none());
        assert!(cfg.robustness.is_none());
    }

    #[test]
    fn protection_controller_lifecycle() {
        let mut ctrl = ProtectionController::new();
        assert!(!ctrl.is_initialized());
        ctrl.initialize();
        assert!(ctrl.is_initialized());
        ctrl.reset();
        assert!(!ctrl.is_initialized());
    }

    #[test]
    fn protection_controller_double_initialize() {
        let mut ctrl = ProtectionController::new();
        ctrl.initialize();
        ctrl.initialize();
        assert!(ctrl.is_initialized());
    }

    #[test]
    fn protection_event_variants_equality() {
        assert_eq!(ProtectionEvent::NeedKey, ProtectionEvent::NeedKey);
        assert_ne!(ProtectionEvent::NeedKey, ProtectionEvent::KeySessionCreated);
        assert_eq!(ProtectionEvent::LicenseRequestComplete, ProtectionEvent::LicenseRequestComplete);
    }

    #[test]
    fn protection_event_clone() {
        let evt = ProtectionEvent::KeySystemSelected;
        let evt2 = evt.clone();
        assert_eq!(evt, evt2);
    }

    #[test]
    fn protection_config_with_clear_keys() {
        let mut keys = std::collections::HashMap::new();
        keys.insert("kid1".into(), "key1".into());
        let cfg = ProtectionConfig { clear_keys: Some(keys), ..Default::default() };
        assert!(cfg.clear_keys.is_some());
        assert_eq!(cfg.clear_keys.unwrap().len(), 1);
    }
}
