//! Media Source Extension (MSE) buffer management
//!
//! Handles SourceBuffer operations, seeking, and buffer lifecycle.

use crate::hls::error::{ErrorCode, HlsError, HlsResult};
use js_sys::{Array, Function, Promise, Uint8Array};
use std::collections::VecDeque;
use wasm_bindgen::prelude::*;
use wasm_bindgen::JsCast;
use wasm_bindgen_futures::JsFuture;
use web_sys::{HtmlVideoElement, MediaSource, SourceBuffer};

/// Segment type being buffered
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SegmentType {
    Audio,
    Video,
    Combined,
}

/// Pending buffer operation
#[allow(dead_code)]
#[derive(Debug)]
enum BufferOperation {
    Append {
        data: Vec<u8>,
        segment_type: SegmentType,
    },
    Remove {
        start: f64,
        end: f64,
    },
}

/// Buffer state information
#[derive(Clone, Debug)]
pub struct BufferState {
    /// Total buffered time ranges
    pub buffered_ranges: Vec<(f64, f64)>,
    /// Buffer ahead of current time
    pub buffer_ahead: f64,
    /// Buffer behind current time  
    pub buffer_behind: f64,
    /// Current playback position
    pub current_time: f64,
    /// Video duration
    pub duration: f64,
    /// Whether buffer is updating
    pub updating: bool,
}

impl BufferState {
    /// Check if a time range is buffered
    pub fn is_buffered(&self, time: f64) -> bool {
        self.buffered_ranges.iter().any(|(start, end)| {
            time >= *start && time < *end
        })
    }
    
    /// Get the buffered range containing a time point
    pub fn buffered_range_at(&self, time: f64) -> Option<(f64, f64)> {
        self.buffered_ranges.iter().find(|(start, end)| {
            time >= *start && time < *end
        }).copied()
    }
}

/// MSE Buffer Controller
#[allow(dead_code)]
pub struct BufferController {
    /// Reference to MediaSource
    media_source: MediaSource,
    /// Reference to video element
    video: HtmlVideoElement,
    /// Main SourceBuffer (video + audio muxed, or video only)
    source_buffer: Option<SourceBuffer>,
    /// Audio SourceBuffer (for demuxed streams)
    audio_buffer: Option<SourceBuffer>,
    /// MIME type for the source buffer
    mime_type: String,
    /// Audio MIME type (if demuxed)
    audio_mime_type: Option<String>,
    /// Pending operations queue
    pending_ops: VecDeque<BufferOperation>,
    /// Whether end of stream has been signaled
    eos_signaled: bool,
    /// Maximum buffer length (seconds)
    max_buffer_length: f64,
    /// Back buffer length before cleanup (seconds)
    back_buffer_length: f64,
    /// Whether we're in seeking mode
    seeking: bool,
    /// Target seek time
    seek_target: Option<f64>,
}

impl BufferController {
    /// Create a new buffer controller
    pub fn new(video: HtmlVideoElement, max_buffer: f64, back_buffer: f64) -> HlsResult<Self> {
        let media_source = MediaSource::new().map_err(|e| {
            HlsError::capability(ErrorCode::MseNotSupported, format!("MediaSource::new failed: {e:?}"))
        })?;
        
        Ok(Self {
            media_source,
            video,
            source_buffer: None,
            audio_buffer: None,
            mime_type: String::new(),
            audio_mime_type: None,
            pending_ops: VecDeque::new(),
            eos_signaled: false,
            max_buffer_length: max_buffer,
            back_buffer_length: back_buffer,
            seeking: false,
            seek_target: None,
        })
    }
    
    /// Get the MediaSource
    pub fn media_source(&self) -> &MediaSource {
        &self.media_source
    }
    
    /// Attach to video element and wait for sourceopen
    pub async fn attach(&mut self) -> HlsResult<String> {
        let obj_url = web_sys::Url::create_object_url_with_source(&self.media_source)
            .map_err(|e| HlsError::media(ErrorCode::MediaSourceError, format!("createObjectURL: {e:?}")))?;
        
        self.video.set_src(&obj_url);
        
        // Wait for sourceopen
        self.wait_for_sourceopen().await?;
        
        Ok(obj_url)
    }
    
    /// Wait for MediaSource to open
    async fn wait_for_sourceopen(&self) -> HlsResult<()> {
        let p = Promise::new(&mut |resolve: Function, _reject: Function| {
            let cb = Closure::once_into_js(move || {
                resolve.call0(&JsValue::NULL).ok();
            });
            self.media_source.set_onsourceopen(Some(cb.unchecked_ref()));
        });
        
        JsFuture::from(p).await.map_err(|e| {
            HlsError::media(ErrorCode::MediaSourceError, format!("sourceopen failed: {e:?}"))
        })?;
        
        Ok(())
    }
    
    /// Add a source buffer with the given MIME type
    pub fn add_source_buffer(&mut self, mime_type: &str) -> HlsResult<()> {
        if !MediaSource::is_type_supported(mime_type) {
            return Err(HlsError::capability(
                ErrorCode::CodecNotSupported,
                format!("MIME type not supported: {}", mime_type),
            ));
        }
        
        let sb = self.media_source.add_source_buffer(mime_type).map_err(|e| {
            HlsError::media(ErrorCode::MediaSourceError, format!("addSourceBuffer: {e:?}"))
        })?;
        
        self.mime_type = mime_type.to_string();
        self.source_buffer = Some(sb);
        
        Ok(())
    }
    
    /// Add an audio source buffer (for demuxed streams)
    pub fn add_audio_buffer(&mut self, mime_type: &str) -> HlsResult<()> {
        if !MediaSource::is_type_supported(mime_type) {
            return Err(HlsError::capability(
                ErrorCode::CodecNotSupported,
                format!("Audio MIME type not supported: {}", mime_type),
            ));
        }
        
        let sb = self.media_source.add_source_buffer(mime_type).map_err(|e| {
            HlsError::media(ErrorCode::MediaSourceError, format!("addSourceBuffer (audio): {e:?}"))
        })?;
        
        self.audio_mime_type = Some(mime_type.to_string());
        self.audio_buffer = Some(sb);
        
        Ok(())
    }
    
    /// Append data to the buffer
    pub async fn append_buffer(&mut self, data: &[u8], segment_type: SegmentType) -> HlsResult<()> {
        // Clone the source buffer reference to avoid borrow issues
        let sb_clone = match segment_type {
            SegmentType::Audio => self.audio_buffer.clone(),
            _ => self.source_buffer.clone(),
        };
        
        let sb = sb_clone.ok_or_else(|| {
            HlsError::media(ErrorCode::BufferAppendError, "No source buffer available")
        })?;
        
        // Wait for any pending updates
        Self::wait_for_update_static(&sb).await?;
        
        // Try to append
        match Self::try_append_static(&sb, data).await {
            Ok(()) => Ok(()),
            Err(e) if is_quota_exceeded(&e) => {
                // Handle quota exceeded by removing old buffer
                log::info!("Buffer quota exceeded, removing old data...");
                let current_time = self.video.current_time();
                let remove_end = (current_time - self.back_buffer_length).max(0.0);
                if remove_end > 0.0 {
                    Self::remove_buffer_range_static(&sb, 0.0, remove_end).await?;
                }
                // Retry append
                Self::try_append_static(&sb, data).await
            }
            Err(e) => Err(e),
        }
    }
    
    /// Wait for buffer to finish updating (static version)
    async fn wait_for_update_static(sb: &SourceBuffer) -> HlsResult<()> {
        while sb.updating() {
            let p = Promise::new(&mut |resolve: Function, _: Function| {
                let cb = Closure::once_into_js(move || {
                    resolve.call0(&JsValue::NULL).ok();
                });
                sb.set_onupdateend(Some(cb.unchecked_ref()));
            });
            
            JsFuture::from(p).await.map_err(|e| {
                HlsError::media(ErrorCode::BufferAppendError, format!("Wait for update failed: {e:?}"))
            })?;
            
            sb.set_onupdateend(None);
        }
        
        Ok(())
    }
    
    /// Try to append data to buffer (static version)
    async fn try_append_static(sb: &SourceBuffer, data: &[u8]) -> HlsResult<()> {
        let arr = Uint8Array::from(data);
        
        // Set up event handlers
        let updateend_p = Promise::new(&mut |resolve: Function, _: Function| {
            let cb = Closure::once_into_js(move || {
                resolve.call0(&JsValue::NULL).ok();
            });
            sb.set_onupdateend(Some(cb.unchecked_ref()));
        });
        
        let error_p = Promise::new(&mut |_: Function, reject: Function| {
            let cb = Closure::once_into_js(move || {
                reject.call1(&JsValue::NULL, &JsValue::from_str("SourceBuffer error")).ok();
            });
            sb.set_onerror(Some(cb.unchecked_ref()));
        });
        
        let race = Promise::race(&Array::of2(updateend_p.as_ref(), error_p.as_ref()));
        
        // Append the data
        if let Err(e) = sb.append_buffer_with_array_buffer_view(arr.unchecked_ref()) {
            sb.set_onupdateend(None);
            sb.set_onerror(None);
            return Err(HlsError::media(
                ErrorCode::BufferAppendError,
                format!("appendBuffer failed: {e:?}"),
            ));
        }
        
        // Wait for completion
        let result = JsFuture::from(race).await;
        sb.set_onupdateend(None);
        sb.set_onerror(None);
        
        result.map_err(|e| {
            HlsError::media(ErrorCode::BufferAppendError, format!("Buffer decode error: {e:?}"))
        })?;
        
        Ok(())
    }
    
    /// Remove a range from the buffer (static version)
    async fn remove_buffer_range_static(sb: &SourceBuffer, start: f64, end: f64) -> HlsResult<()> {
        Self::wait_for_update_static(sb).await?;
        
        if let Err(e) = sb.remove(start, end) {
            // Log but don't fail - browser may reject removal for valid reasons
            log::debug!("Buffer removal skipped: {:?}", e);
            return Ok(());
        }
        
        Self::wait_for_update_static(sb).await
    }
    
    /// Create event promises for buffer operations
    #[allow(dead_code)]
    fn create_event_promises(&self, sb: &SourceBuffer) -> (Promise, Promise) {
        let updateend_p = Promise::new(&mut |resolve: Function, _: Function| {
            let cb = Closure::once_into_js(move || {
                resolve.call0(&JsValue::NULL).ok();
            });
            sb.set_onupdateend(Some(cb.unchecked_ref()));
        });
        
        let error_p = Promise::new(&mut |_: Function, reject: Function| {
            let cb = Closure::once_into_js(move || {
                reject.call1(&JsValue::NULL, &JsValue::from_str("SourceBuffer error")).ok();
            });
            sb.set_onerror(Some(cb.unchecked_ref()));
        });
        
        (updateend_p, error_p)
    }
    
    /// Clear event handlers from buffer
    #[allow(dead_code)]
    fn clear_event_handlers(&self, sb: &SourceBuffer) {
        sb.set_onupdateend(None);
        sb.set_onerror(None);
    }
    
    /// Wait for buffer to finish updating
    #[allow(dead_code)]
    async fn wait_for_update(&self, sb: &SourceBuffer) -> HlsResult<()> {
        while sb.updating() {
            let p = Promise::new(&mut |resolve: Function, _: Function| {
                let cb = Closure::once_into_js(move || {
                    resolve.call0(&JsValue::NULL).ok();
                });
                sb.set_onupdateend(Some(cb.unchecked_ref()));
            });
            
            JsFuture::from(p).await.map_err(|e| {
                HlsError::media(ErrorCode::BufferAppendError, format!("Wait for update failed: {e:?}"))
            })?;
            
            sb.set_onupdateend(None);
        }
        
        Ok(())
    }
    
    /// Remove old buffer data behind current position
    #[allow(dead_code)]
    async fn remove_old_buffer(&mut self, sb: &SourceBuffer) -> HlsResult<()> {
        let current_time = self.video.current_time();
        let remove_end = (current_time - self.back_buffer_length).max(0.0);
        
        if remove_end <= 0.0 {
            return Ok(());
        }
        
        self.remove_buffer_range(sb, 0.0, remove_end).await
    }
    
    /// Remove a range from the buffer
    #[allow(dead_code)]
    async fn remove_buffer_range(&self, sb: &SourceBuffer, start: f64, end: f64) -> HlsResult<()> {
        self.wait_for_update(sb).await?;
        
        if let Err(e) = sb.remove(start, end) {
            // Log but don't fail - browser may reject removal for valid reasons
            log::debug!("Buffer removal skipped: {:?}", e);
            return Ok(());
        }
        
        self.wait_for_update(sb).await
    }
    
    /// Get current buffer state
    pub fn get_buffer_state(&self) -> BufferState {
        let current_time = self.video.current_time();
        let duration = self.video.duration();
        
        let buffered_ranges = self.get_buffered_ranges();
        
        let buffer_ahead = buffered_ranges.iter()
            .find(|(start, end)| current_time >= *start && current_time < *end)
            .map(|(_, end)| end - current_time)
            .unwrap_or(0.0);
            
        let buffer_behind = buffered_ranges.iter()
            .find(|(start, end)| current_time >= *start && current_time < *end)
            .map(|(start, _)| current_time - start)
            .unwrap_or(0.0);
        
        let updating = self.source_buffer.as_ref().map(|sb| sb.updating()).unwrap_or(false)
            || self.audio_buffer.as_ref().map(|sb| sb.updating()).unwrap_or(false);
        
        BufferState {
            buffered_ranges,
            buffer_ahead,
            buffer_behind,
            current_time,
            duration: if duration.is_finite() { duration } else { 0.0 },
            updating,
        }
    }
    
    /// Get buffered time ranges
    fn get_buffered_ranges(&self) -> Vec<(f64, f64)> {
        let mut ranges = Vec::new();
        let buffered = self.video.buffered();
        
        for i in 0..buffered.length() {
            if let (Ok(start), Ok(end)) = (buffered.start(i), buffered.end(i)) {
                ranges.push((start, end));
            }
        }
        
        ranges
    }
    
    /// Check if a segment is buffered
    pub fn is_segment_buffered(&self, start_time: f64, duration: f64) -> bool {
        let segment_mid = start_time + duration / 2.0;
        let buffered = self.video.buffered();
        
        for i in 0..buffered.length() {
            if let (Ok(start), Ok(end)) = (buffered.start(i), buffered.end(i)) {
                if segment_mid >= start && segment_mid < end {
                    return true;
                }
            }
        }
        
        false
    }
    
    /// Prepare for seeking - flush buffer if needed
    pub async fn prepare_seek(&mut self, target: f64) -> HlsResult<()> {
        self.seeking = true;
        self.seek_target = Some(target);
        
        // Check if target is already buffered
        let state = self.get_buffer_state();
        if state.is_buffered(target) {
            // Already buffered, just seek
            return Ok(());
        }
        
        // For instant quality switch, flush the buffer
        // For smooth/conservative, keep existing buffer
        
        Ok(())
    }
    
    /// Complete seek operation
    pub fn complete_seek(&mut self) {
        self.seeking = false;
        self.seek_target = None;
    }
    
    /// Signal end of stream
    pub fn end_of_stream(&mut self) -> HlsResult<()> {
        if self.eos_signaled {
            return Ok(());
        }
        
        // Wait for buffers to finish
        if let Some(ref sb) = self.source_buffer {
            if sb.updating() {
                return Ok(()); // Will be called again
            }
        }
        if let Some(ref sb) = self.audio_buffer {
            if sb.updating() {
                return Ok(());
            }
        }
        
        self.media_source.end_of_stream().map_err(|e| {
            HlsError::media(ErrorCode::MediaSourceError, format!("endOfStream failed: {e:?}"))
        })?;
        
        self.eos_signaled = true;
        Ok(())
    }
    
    /// Flush all buffers (for level switch or recovery)
    pub async fn flush_buffers(&mut self) -> HlsResult<()> {
        if let Some(sb) = self.source_buffer.clone() {
            Self::wait_for_update_static(&sb).await?;
            if let Ok(buffered) = sb.buffered() {
                if buffered.length() > 0 {
                    if let (Ok(start), Ok(end)) = (buffered.start(0), buffered.end(buffered.length() - 1)) {
                        Self::remove_buffer_range_static(&sb, start, end).await?;
                    }
                }
            }
        }
        
        if let Some(sb) = self.audio_buffer.clone() {
            Self::wait_for_update_static(&sb).await?;
            if let Ok(buffered) = sb.buffered() {
                if buffered.length() > 0 {
                    if let (Ok(start), Ok(end)) = (buffered.start(0), buffered.end(buffered.length() - 1)) {
                        Self::remove_buffer_range_static(&sb, start, end).await?;
                    }
                }
            }
        }
        
        self.eos_signaled = false;
        Ok(())
    }
    
    /// Detach from video element and cleanup
    pub fn detach(&mut self, object_url: &str) {
        if let Some(ref sb) = self.source_buffer {
            self.clear_event_handlers(sb);
        }
        if let Some(ref sb) = self.audio_buffer {
            self.clear_event_handlers(sb);
        }
        
        // Revoke object URL
        web_sys::Url::revoke_object_url(object_url).ok();
        
        // Clear video source
        self.video.set_src("");
    }
}

/// Check if error is a quota exceeded error
fn is_quota_exceeded(error: &HlsError) -> bool {
    error.message.contains("QuotaExceeded") || error.message.contains("quota")
}
