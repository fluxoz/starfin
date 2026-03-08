//! Fragment loader with retry mechanism and bandwidth tracking

use crate::hls::config::HlsConfig;
use crate::hls::error::{ErrorCode, HlsError, HlsResult};
use crate::hls::events::FragmentStats;
use crate::hls::playlist::{EncryptionMethod, KeyInfo, MapInfo, Segment};
use gloo_net::http::Request;
use std::collections::HashMap;

/// Fragment load context
#[derive(Clone, Debug)]
pub struct FragmentContext {
    /// Level index
    pub level: usize,
    /// Segment sequence number
    pub sn: u64,
    /// Start time of the segment
    pub start_time: f64,
    /// Duration of the segment
    pub duration: f64,
    /// Byte range if applicable
    pub byte_range: Option<(u64, u64)>,
    /// Whether this is an init segment
    pub is_init: bool,
}

/// Fragment load result
#[derive(Debug)]
pub struct FragmentLoadResult {
    /// Loaded data
    pub data: Vec<u8>,
    /// Load statistics
    pub stats: FragmentStats,
    /// Fragment context
    pub context: FragmentContext,
}

/// Loaded encryption key
#[derive(Clone)]
pub struct LoadedKey {
    pub key_data: Vec<u8>,
    pub iv: Option<Vec<u8>>,
    pub method: EncryptionMethod,
}

/// Fragment loader with retry and caching
pub struct FragmentLoader {
    /// Configuration
    config: HlsConfig,
    /// Cached init segments (level -> data)
    init_cache: HashMap<usize, Vec<u8>>,
    /// Cached encryption keys (uri -> key data)
    key_cache: HashMap<String, LoadedKey>,
    /// Current load operation timestamp (for cancellation)
    current_load_id: u64,
    /// Base URL for resolving relative URLs
    base_url: String,
}

impl FragmentLoader {
    /// Create a new fragment loader
    pub fn new(config: &HlsConfig) -> Self {
        Self {
            config: config.clone(),
            init_cache: HashMap::new(),
            key_cache: HashMap::new(),
            current_load_id: 0,
            base_url: String::new(),
        }
    }
    
    /// Set base URL for relative URL resolution
    pub fn set_base_url(&mut self, url: &str) {
        self.base_url = url.to_string();
    }
    
    /// Load a segment
    pub async fn load_segment(&mut self, segment: &Segment, level: usize) -> HlsResult<FragmentLoadResult> {
        let context = FragmentContext {
            level,
            sn: segment.sequence_number,
            start_time: segment.start_time,
            duration: segment.duration,
            byte_range: segment.byte_range,
            is_init: false,
        };
        
        self.load_with_retry(&segment.uri, segment.byte_range, context).await
    }
    
    /// Load an init segment (with caching)
    pub async fn load_init_segment(&mut self, map: &MapInfo, level: usize) -> HlsResult<FragmentLoadResult> {
        // Check cache first
        if let Some(data) = self.init_cache.get(&level) {
            return Ok(FragmentLoadResult {
                data: data.clone(),
                stats: FragmentStats {
                    load_time: 0.0,
                    loaded_bytes: data.len() as u64,
                    bandwidth: 0.0,
                    retry_count: 0,
                },
                context: FragmentContext {
                    level,
                    sn: 0,
                    start_time: 0.0,
                    duration: 0.0,
                    byte_range: map.byte_range,
                    is_init: true,
                },
            });
        }
        
        let context = FragmentContext {
            level,
            sn: 0,
            start_time: 0.0,
            duration: 0.0,
            byte_range: map.byte_range,
            is_init: true,
        };
        
        let result = self.load_with_retry(&map.uri, map.byte_range, context).await?;
        
        // Cache the init segment
        self.init_cache.insert(level, result.data.clone());
        
        Ok(result)
    }
    
    /// Load an encryption key
    pub async fn load_key(&mut self, key_info: &KeyInfo) -> HlsResult<LoadedKey> {
        let uri = key_info.uri.as_ref().ok_or_else(|| {
            HlsError::network(ErrorCode::KeyLoadError, "Key URI is missing")
        })?;
        
        // Check cache
        if let Some(cached) = self.key_cache.get(uri) {
            return Ok(cached.clone());
        }
        
        // Load the key (ignore retry count for keys)
        let (data, _) = self.fetch_with_retry(uri, None, self.config.frag_load_max_retry).await?;
        
        if data.len() != 16 {
            return Err(HlsError::network(
                ErrorCode::KeyLoadError,
                format!("Invalid key size: expected 16 bytes, got {}", data.len()),
            ));
        }
        
        let loaded = LoadedKey {
            key_data: data,
            iv: key_info.iv.clone(),
            method: key_info.method,
        };
        
        // Cache the key
        self.key_cache.insert(uri.clone(), loaded.clone());
        
        Ok(loaded)
    }
    
    /// Load with retry mechanism
    async fn load_with_retry(
        &mut self,
        url: &str,
        byte_range: Option<(u64, u64)>,
        context: FragmentContext,
    ) -> HlsResult<FragmentLoadResult> {
        let start_time = js_sys::Date::now();
        let _load_id = {
            self.current_load_id += 1;
            self.current_load_id
        };
        
        let max_retries = if context.is_init {
            self.config.level_load_max_retry
        } else {
            self.config.frag_load_max_retry
        };
        
        let (data, retry_count) = self.fetch_with_retry(url, byte_range, max_retries).await?;
        
        let end_time = js_sys::Date::now();
        let load_time = end_time - start_time;
        let loaded_bytes = data.len() as u64;
        let bandwidth = if load_time > 0.0 {
            (loaded_bytes as f64 * 8.0 * 1000.0) / load_time
        } else {
            0.0
        };
        
        Ok(FragmentLoadResult {
            data,
            stats: FragmentStats {
                load_time,
                loaded_bytes,
                bandwidth,
                retry_count,
            },
            context,
        })
    }
    
    /// Fetch data with retry
    /// Returns (data, retry_count)
    async fn fetch_with_retry(
        &self,
        url: &str,
        byte_range: Option<(u64, u64)>,
        max_retries: u32,
    ) -> HlsResult<(Vec<u8>, u32)> {
        let mut last_error = None;
        let mut delay = self.config.retry_delay;
        let mut retry_count = 0;
        
        for attempt in 0..=max_retries {
            match self.fetch_bytes(url, byte_range).await {
                Ok(data) => return Ok((data, retry_count)),
                Err(e) => {
                    last_error = Some(e);
                    if attempt < max_retries {
                        retry_count += 1;
                        // Wait before retry with exponential backoff
                        gloo_timers::future::TimeoutFuture::new(delay).await;
                        delay = ((delay as f64 * self.config.retry_backoff_factor) as u32)
                            .min(self.config.max_retry_delay);
                    }
                }
            }
        }
        
        Err(last_error.unwrap_or_else(|| {
            HlsError::network(ErrorCode::FragLoadError, "Failed to load fragment")
        }))
    }
    
    /// Fetch bytes from URL
    async fn fetch_bytes(&self, url: &str, byte_range: Option<(u64, u64)>) -> HlsResult<Vec<u8>> {
        let mut request = Request::get(url);
        
        // Add byte range header if specified
        if let Some((offset, length)) = byte_range {
            let range_header = format!("bytes={}-{}", offset, offset + length - 1);
            request = request.header("Range", &range_header);
        }
        
        let resp = request.send().await.map_err(|e| {
            HlsError::network(ErrorCode::FragLoadError, format!("Fetch error: {e:?}"))
        })?;
        
        if !resp.ok() && resp.status() != 206 {
            return Err(HlsError::network(
                ErrorCode::FragLoadError,
                format!("HTTP {} for {}", resp.status(), url),
            ));
        }
        
        resp.binary().await.map_err(|e| {
            HlsError::network(ErrorCode::FragLoadError, format!("Binary read error: {e:?}"))
        })
    }
    
    /// Clear init segment cache for a level
    pub fn clear_init_cache(&mut self, level: Option<usize>) {
        if let Some(lvl) = level {
            self.init_cache.remove(&lvl);
        } else {
            self.init_cache.clear();
        }
    }
    
    /// Clear key cache
    pub fn clear_key_cache(&mut self) {
        self.key_cache.clear();
    }
    
    /// Cancel current load operation (for seeking)
    pub fn cancel_load(&mut self) {
        self.current_load_id += 1;
    }
    
    /// Check if a load is still valid (not cancelled)
    pub fn is_load_valid(&self, load_id: u64) -> bool {
        load_id == self.current_load_id
    }
}

/// Manifest/playlist loader
pub struct ManifestLoader {
    config: HlsConfig,
}

impl ManifestLoader {
    pub fn new(config: &HlsConfig) -> Self {
        Self {
            config: config.clone(),
        }
    }
    
    /// Load a manifest/playlist
    pub async fn load(&self, url: &str) -> HlsResult<String> {
        self.load_with_retry(url).await
    }
    
    /// Load with retry mechanism
    async fn load_with_retry(&self, url: &str) -> HlsResult<String> {
        let mut last_error = None;
        let mut delay = self.config.retry_delay;
        
        for attempt in 0..=self.config.manifest_load_max_retry {
            match self.fetch_text(url).await {
                Ok(text) => return Ok(text),
                Err(e) => {
                    last_error = Some(e);
                    if attempt < self.config.manifest_load_max_retry {
                        gloo_timers::future::TimeoutFuture::new(delay).await;
                        delay = ((delay as f64 * self.config.retry_backoff_factor) as u32)
                            .min(self.config.max_retry_delay);
                    }
                }
            }
        }
        
        Err(last_error.unwrap_or_else(|| {
            HlsError::network(ErrorCode::ManifestLoadError, "Failed to load manifest")
        }))
    }
    
    /// Fetch text from URL
    async fn fetch_text(&self, url: &str) -> HlsResult<String> {
        let resp = Request::get(url).send().await.map_err(|e| {
            HlsError::network(ErrorCode::ManifestLoadError, format!("Fetch error: {e:?}"))
        })?;
        
        if !resp.ok() {
            return Err(HlsError::network(
                ErrorCode::ManifestLoadError,
                format!("HTTP {} for {}", resp.status(), url),
            ));
        }
        
        resp.text().await.map_err(|e| {
            HlsError::network(ErrorCode::ManifestLoadError, format!("Text read error: {e:?}"))
        })
    }
}
