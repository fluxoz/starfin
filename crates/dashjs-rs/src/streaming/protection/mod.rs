//! Port of `dash.js/src/streaming/protection/`.
//!
//! DRM/EME protection infrastructure matching dash.js protection module.
//! Provides key system detection, session management, license request/response
//! handling, and a trait-based protection model for EME integration.

use serde::{Deserialize, Serialize};
use std::collections::HashMap;

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

impl KeySystem {
    /// Returns the system string identifier matching W3C EME key system strings.
    pub fn system_string(&self) -> &str {
        match self {
            KeySystem::Widevine => "com.widevine.alpha",
            KeySystem::PlayReady => "com.microsoft.playready",
            KeySystem::ClearKey => "org.w3.clearkey",
            KeySystem::FairPlay => "com.apple.fps.1_0",
            KeySystem::PrimeTime => "com.adobe.primetime",
            KeySystem::Other(s) => s.as_str(),
        }
    }

    /// Parse a key system from its W3C system string.
    pub fn from_system_string(s: &str) -> Self {
        match s {
            "com.widevine.alpha" => KeySystem::Widevine,
            "com.microsoft.playready" => KeySystem::PlayReady,
            "org.w3.clearkey" => KeySystem::ClearKey,
            "com.apple.fps.1_0" => KeySystem::FairPlay,
            "com.adobe.primetime" => KeySystem::PrimeTime,
            other => KeySystem::Other(other.to_string()),
        }
    }
}

/// Protection configuration.
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct ProtectionConfig {
    pub key_system: Option<String>,
    pub server_url: Option<String>,
    pub server_certificate: Option<Vec<u8>>,
    pub clear_keys: Option<HashMap<String, String>>,
    pub robustness: Option<String>,
}

/// Information about a specific key system from the MPD ContentProtection.
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct KeySystemInfo {
    pub scheme_id_uri: String,
    pub key_system: Option<String>,
    pub default_key_id: Option<String>,
    pub init_data: Option<Vec<u8>>,
    pub init_data_type: Option<String>,
    pub content_protection: Option<String>,
}

/// Event triggered when EME needs keys.
#[derive(Clone, Debug)]
pub struct NeedKeyEvent {
    pub init_data: Vec<u8>,
    pub init_data_type: String,
    pub key_system_info: Option<KeySystemInfo>,
}

/// Key session state.
#[derive(Clone, Debug, Default)]
pub struct KeySessionInfo {
    pub session_id: String,
    pub session_type: String,
    pub key_statuses: HashMap<String, String>,
}

/// License request built by the protection controller.
#[derive(Clone, Debug, Default)]
pub struct LicenseRequest {
    pub url: String,
    pub method: String,
    pub headers: HashMap<String, String>,
    pub body: Vec<u8>,
    pub session_id: String,
}

/// License response from the license server.
#[derive(Clone, Debug, Default)]
pub struct LicenseResponse {
    pub data: Vec<u8>,
    pub status: u16,
    pub headers: HashMap<String, String>,
}

/// Trait for EME protection model implementations.
/// Platform-specific implementations (wasm, native) implement this trait.
pub trait ProtectionModel {
    fn request_key_system_access(&self, key_system: &KeySystem, configs: &[ProtectionConfig]) -> Result<(), String>;
    fn create_key_session(&mut self, init_data: &[u8], init_data_type: &str) -> Result<String, String>;
    fn update_key_session(&mut self, session_id: &str, message: &[u8]) -> Result<(), String>;
    fn close_key_session(&mut self, session_id: &str) -> Result<(), String>;
    fn set_server_certificate(&mut self, certificate: &[u8]) -> Result<(), String>;
}

/// Default (stub) protection model returning errors for all operations.
#[derive(Clone, Debug, Default)]
pub struct DefaultProtectionModel;

impl ProtectionModel for DefaultProtectionModel {
    fn request_key_system_access(&self, _key_system: &KeySystem, _configs: &[ProtectionConfig]) -> Result<(), String> {
        Err("EME not available in this environment".into())
    }
    fn create_key_session(&mut self, _init_data: &[u8], _init_data_type: &str) -> Result<String, String> {
        Err("EME not available in this environment".into())
    }
    fn update_key_session(&mut self, _session_id: &str, _message: &[u8]) -> Result<(), String> {
        Err("EME not available in this environment".into())
    }
    fn close_key_session(&mut self, _session_id: &str) -> Result<(), String> {
        Err("EME not available in this environment".into())
    }
    fn set_server_certificate(&mut self, _certificate: &[u8]) -> Result<(), String> {
        Err("EME not available in this environment".into())
    }
}

/// Protection controller managing DRM/EME key sessions and license requests.
///
/// Port of `dash.js/src/streaming/protection/controllers/ProtectionController.js`.
#[derive(Clone, Debug, Default)]
pub struct ProtectionController {
    _initialized: bool,
    protection_data: Option<ProtectionConfig>,
    key_system_infos: Vec<KeySystemInfo>,
    sessions: Vec<KeySessionInfo>,
    server_certificate: Option<Vec<u8>>,
}

impl ProtectionController {
    pub fn new() -> Self { Self::default() }

    pub fn initialize(&mut self) { self._initialized = true; }

    pub fn reset(&mut self) {
        self._initialized = false;
        self.protection_data = None;
        self.key_system_infos.clear();
        self.sessions.clear();
        self.server_certificate = None;
    }

    pub fn is_initialized(&self) -> bool { self._initialized }

    pub fn set_protection_data(&mut self, data: ProtectionConfig) {
        self.protection_data = Some(data);
    }

    pub fn get_protection_data(&self) -> Option<&ProtectionConfig> {
        self.protection_data.as_ref()
    }

    pub fn set_key_system_infos(&mut self, infos: Vec<KeySystemInfo>) {
        self.key_system_infos = infos;
    }

    pub fn get_key_system_infos(&self) -> &[KeySystemInfo] {
        &self.key_system_infos
    }

    pub fn create_key_session(&mut self, init_data: &[u8], init_data_type: &str) -> String {
        let session_id = format!("session-{}", self.sessions.len());
        self.sessions.push(KeySessionInfo {
            session_id: session_id.clone(),
            session_type: init_data_type.to_string(),
            key_statuses: HashMap::new(),
        });
        let _ = init_data; // used by real EME implementation
        session_id
    }

    pub fn close_key_session(&mut self, session_id: &str) -> bool {
        let len_before = self.sessions.len();
        self.sessions.retain(|s| s.session_id != session_id);
        self.sessions.len() < len_before
    }

    pub fn close_all_sessions(&mut self) {
        self.sessions.clear();
    }

    pub fn get_sessions(&self) -> &[KeySessionInfo] {
        &self.sessions
    }

    pub fn set_server_certificate(&mut self, cert: Vec<u8>) {
        self.server_certificate = Some(cert);
    }

    pub fn get_server_certificate(&self) -> Option<&[u8]> {
        self.server_certificate.as_deref()
    }

    /// Build a license request for a given session.
    pub fn build_license_request(&self, challenge: &[u8], session_id: &str) -> Option<LicenseRequest> {
        let server_url = self.protection_data.as_ref()?.server_url.as_ref()?;
        Some(LicenseRequest {
            url: server_url.clone(),
            method: "POST".to_string(),
            headers: HashMap::new(),
            body: challenge.to_vec(),
            session_id: session_id.to_string(),
        })
    }
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
    fn key_system_string_roundtrip() {
        assert_eq!(KeySystem::Widevine.system_string(), "com.widevine.alpha");
        assert_eq!(KeySystem::PlayReady.system_string(), "com.microsoft.playready");
        assert_eq!(KeySystem::ClearKey.system_string(), "org.w3.clearkey");
        assert_eq!(KeySystem::FairPlay.system_string(), "com.apple.fps.1_0");
        assert_eq!(KeySystem::from_system_string("com.widevine.alpha"), KeySystem::Widevine);
        assert_eq!(KeySystem::from_system_string("com.microsoft.playready"), KeySystem::PlayReady);
        assert_eq!(KeySystem::from_system_string("unknown.drm"), KeySystem::Other("unknown.drm".into()));
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
        let mut keys = HashMap::new();
        keys.insert("kid1".into(), "key1".into());
        let cfg = ProtectionConfig { clear_keys: Some(keys), ..Default::default() };
        assert!(cfg.clear_keys.is_some());
        assert_eq!(cfg.clear_keys.unwrap().len(), 1);
    }

    #[test]
    fn set_and_get_protection_data() {
        let mut ctrl = ProtectionController::new();
        assert!(ctrl.get_protection_data().is_none());
        ctrl.set_protection_data(ProtectionConfig {
            server_url: Some("https://license.example.com".into()),
            ..Default::default()
        });
        let data = ctrl.get_protection_data().unwrap();
        assert_eq!(data.server_url.as_deref(), Some("https://license.example.com"));
    }

    #[test]
    fn create_and_close_key_sessions() {
        let mut ctrl = ProtectionController::new();
        let s1 = ctrl.create_key_session(b"init_data_1", "cenc");
        let s2 = ctrl.create_key_session(b"init_data_2", "cenc");
        assert_eq!(ctrl.get_sessions().len(), 2);
        assert!(ctrl.close_key_session(&s1));
        assert_eq!(ctrl.get_sessions().len(), 1);
        assert_eq!(ctrl.get_sessions()[0].session_id, s2);
        ctrl.close_all_sessions();
        assert!(ctrl.get_sessions().is_empty());
    }

    #[test]
    fn close_nonexistent_session() {
        let mut ctrl = ProtectionController::new();
        assert!(!ctrl.close_key_session("nonexistent"));
    }

    #[test]
    fn set_server_certificate() {
        let mut ctrl = ProtectionController::new();
        assert!(ctrl.get_server_certificate().is_none());
        ctrl.set_server_certificate(vec![1, 2, 3]);
        assert_eq!(ctrl.get_server_certificate(), Some(&[1u8, 2, 3][..]));
    }

    #[test]
    fn build_license_request() {
        let mut ctrl = ProtectionController::new();
        // No protection data — should return None
        assert!(ctrl.build_license_request(b"challenge", "s1").is_none());
        ctrl.set_protection_data(ProtectionConfig {
            server_url: Some("https://license.example.com/acquire".into()),
            ..Default::default()
        });
        let req = ctrl.build_license_request(b"challenge", "s1").unwrap();
        assert_eq!(req.url, "https://license.example.com/acquire");
        assert_eq!(req.method, "POST");
        assert_eq!(req.body, b"challenge");
        assert_eq!(req.session_id, "s1");
    }

    #[test]
    fn key_system_info_defaults() {
        let info = KeySystemInfo::default();
        assert!(info.scheme_id_uri.is_empty());
        assert!(info.key_system.is_none());
        assert!(info.init_data.is_none());
    }

    #[test]
    fn set_key_system_infos() {
        let mut ctrl = ProtectionController::new();
        ctrl.set_key_system_infos(vec![
            KeySystemInfo { scheme_id_uri: "urn:uuid:edef8ba9-79d6-4ace-a3c8-27dcd51d21ed".into(), ..Default::default() },
        ]);
        assert_eq!(ctrl.get_key_system_infos().len(), 1);
    }

    #[test]
    fn default_protection_model_returns_errors() {
        let mut model = DefaultProtectionModel;
        assert!(model.request_key_system_access(&KeySystem::Widevine, &[]).is_err());
        assert!(model.create_key_session(b"data", "cenc").is_err());
        assert!(model.update_key_session("s1", b"msg").is_err());
        assert!(model.close_key_session("s1").is_err());
        assert!(model.set_server_certificate(b"cert").is_err());
    }

    #[test]
    fn reset_clears_all_state() {
        let mut ctrl = ProtectionController::new();
        ctrl.initialize();
        ctrl.set_protection_data(ProtectionConfig { server_url: Some("url".into()), ..Default::default() });
        ctrl.create_key_session(b"data", "cenc");
        ctrl.set_server_certificate(vec![1, 2]);
        ctrl.set_key_system_infos(vec![KeySystemInfo::default()]);
        ctrl.reset();
        assert!(!ctrl.is_initialized());
        assert!(ctrl.get_protection_data().is_none());
        assert!(ctrl.get_sessions().is_empty());
        assert!(ctrl.get_server_certificate().is_none());
        assert!(ctrl.get_key_system_infos().is_empty());
    }
}
