//! Timed Metadata handling
//!
//! Supports ID3 tags, EMSG boxes, and DATERANGE playlist tags.

use crate::hls::events::{DateRange, EmsgEvent, Id3Frame, Id3Sample};
use crate::hls::playlist::DateRangeTag;

/// Metadata types that can be emitted
#[derive(Clone, Debug)]
pub enum MetadataType {
    Id3(Id3Sample),
    Emsg(EmsgEvent),
    DateRange(DateRange),
}

/// Metadata parser for timed metadata in HLS streams
pub struct MetadataParser {
    /// Pending date ranges from playlist
    date_ranges: Vec<DateRangeTag>,
    /// Last PTS for ID3 metadata
    last_id3_pts: Option<f64>,
}

impl MetadataParser {
    pub fn new() -> Self {
        Self {
            date_ranges: Vec::new(),
            last_id3_pts: None,
        }
    }
    
    /// Parse ID3 tags from MPEG-2 TS PES packet
    pub fn parse_id3(&mut self, data: &[u8], pts: f64, dts: f64) -> Option<Id3Sample> {
        // Check for ID3 header (ID3)
        if data.len() < 10 {
            return None;
        }
        
        if &data[0..3] != b"ID3" {
            return None;
        }
        
        // Parse ID3v2 header
        let _version_major = data[3];
        let _version_minor = data[4];
        let _flags = data[5];
        
        // Size is stored as syncsafe integer (7 bits per byte)
        let size = ((data[6] as usize & 0x7F) << 21)
            | ((data[7] as usize & 0x7F) << 14)
            | ((data[8] as usize & 0x7F) << 7)
            | (data[9] as usize & 0x7F);
        
        if data.len() < 10 + size {
            return None;
        }
        
        // Parse frames
        let frames = self.parse_id3_frames(&data[10..10 + size]);
        
        self.last_id3_pts = Some(pts);
        
        Some(Id3Sample {
            pts,
            dts,
            data: data[..10 + size].to_vec(),
            frames,
        })
    }
    
    /// Parse ID3 frames from data
    fn parse_id3_frames(&self, data: &[u8]) -> Vec<Id3Frame> {
        let mut frames = Vec::new();
        let mut offset = 0;
        
        while offset + 10 <= data.len() {
            let frame_id = String::from_utf8_lossy(&data[offset..offset + 4]).to_string();
            
            // Check for padding (null frame ID)
            if frame_id.chars().all(|c| c == '\0') {
                break;
            }
            
            let frame_size = ((data[offset + 4] as usize) << 24)
                | ((data[offset + 5] as usize) << 16)
                | ((data[offset + 6] as usize) << 8)
                | (data[offset + 7] as usize);
            
            // Skip flags (2 bytes)
            let frame_data_start = offset + 10;
            let frame_data_end = frame_data_start + frame_size;
            
            if frame_data_end > data.len() {
                break;
            }
            
            frames.push(Id3Frame {
                id: frame_id,
                data: data[frame_data_start..frame_data_end].to_vec(),
            });
            
            offset = frame_data_end;
        }
        
        frames
    }
    
    /// Parse EMSG box from CMAF/fMP4 segment
    pub fn parse_emsg(&self, data: &[u8]) -> Option<EmsgEvent> {
        // Find emsg box
        let mut offset = 0;
        
        while offset + 8 <= data.len() {
            let box_size = u32::from_be_bytes([
                data[offset],
                data[offset + 1],
                data[offset + 2],
                data[offset + 3],
            ]) as usize;
            
            let box_type = &data[offset + 4..offset + 8];
            
            if box_type == b"emsg" {
                return self.parse_emsg_box(&data[offset + 8..offset + box_size.min(data.len())]);
            }
            
            if box_size == 0 {
                break;
            }
            
            offset += box_size;
        }
        
        None
    }
    
    /// Parse contents of an EMSG box
    fn parse_emsg_box(&self, data: &[u8]) -> Option<EmsgEvent> {
        if data.is_empty() {
            return None;
        }
        
        let version = data[0];
        
        if version == 0 {
            self.parse_emsg_v0(data)
        } else if version == 1 {
            self.parse_emsg_v1(data)
        } else {
            None
        }
    }
    
    /// Parse EMSG version 0
    fn parse_emsg_v0(&self, data: &[u8]) -> Option<EmsgEvent> {
        let mut offset = 4; // Skip version and flags
        
        // Read null-terminated strings
        let scheme_id_uri = read_null_terminated_string(data, &mut offset)?;
        let value = read_null_terminated_string(data, &mut offset)?;
        
        if offset + 16 > data.len() {
            return None;
        }
        
        let timescale = u32::from_be_bytes([
            data[offset], data[offset + 1], data[offset + 2], data[offset + 3],
        ]);
        offset += 4;
        
        let presentation_time_delta = u32::from_be_bytes([
            data[offset], data[offset + 1], data[offset + 2], data[offset + 3],
        ]);
        offset += 4;
        
        let event_duration = u32::from_be_bytes([
            data[offset], data[offset + 1], data[offset + 2], data[offset + 3],
        ]);
        offset += 4;
        
        let id = u32::from_be_bytes([
            data[offset], data[offset + 1], data[offset + 2], data[offset + 3],
        ]);
        offset += 4;
        
        let message_data = data[offset..].to_vec();
        
        Some(EmsgEvent {
            scheme_id_uri,
            value,
            timescale,
            presentation_time_delta,
            event_duration,
            id,
            message_data,
        })
    }
    
    /// Parse EMSG version 1
    fn parse_emsg_v1(&self, data: &[u8]) -> Option<EmsgEvent> {
        if data.len() < 24 {
            return None;
        }
        
        let mut offset = 4; // Skip version and flags
        
        let timescale = u32::from_be_bytes([
            data[offset], data[offset + 1], data[offset + 2], data[offset + 3],
        ]);
        offset += 4;
        
        let _presentation_time = u64::from_be_bytes([
            data[offset], data[offset + 1], data[offset + 2], data[offset + 3],
            data[offset + 4], data[offset + 5], data[offset + 6], data[offset + 7],
        ]);
        offset += 8;
        
        let event_duration = u32::from_be_bytes([
            data[offset], data[offset + 1], data[offset + 2], data[offset + 3],
        ]);
        offset += 4;
        
        let id = u32::from_be_bytes([
            data[offset], data[offset + 1], data[offset + 2], data[offset + 3],
        ]);
        offset += 4;
        
        let scheme_id_uri = read_null_terminated_string(data, &mut offset)?;
        let value = read_null_terminated_string(data, &mut offset)?;
        
        let message_data = data[offset..].to_vec();
        
        Some(EmsgEvent {
            scheme_id_uri,
            value,
            timescale,
            presentation_time_delta: 0, // v1 uses absolute time
            event_duration,
            id,
            message_data,
        })
    }
    
    /// Update date ranges from playlist
    pub fn set_date_ranges(&mut self, ranges: Vec<DateRangeTag>) {
        self.date_ranges = ranges;
    }
    
    /// Get date ranges that are active at a given time
    pub fn active_date_ranges(&self, _time: f64) -> Vec<DateRange> {
        // Convert playlist date ranges to events
        // This would need playlist start date context in a real implementation
        self.date_ranges.iter().map(|dr| DateRange {
            id: dr.id.clone(),
            class: dr.class.clone(),
            start_date: dr.start_date.clone(),
            end_date: dr.end_date.clone(),
            duration: dr.duration,
            planned_duration: dr.planned_duration,
            scte35_cmd: dr.scte35_cmd.as_ref().map(|s| parse_hex(s)),
            scte35_out: dr.scte35_out.as_ref().map(|s| parse_hex(s)),
            scte35_in: dr.scte35_in.as_ref().map(|s| parse_hex(s)),
            end_on_next: dr.end_on_next,
            client_attributes: dr.client_attributes.iter()
                .map(|(k, v)| (k.clone(), v.clone()))
                .collect(),
        }).collect()
    }
    
    /// Parse metadata from segment data
    pub fn parse_segment_metadata(&mut self, data: &[u8], pts: f64, dts: f64) -> Vec<MetadataType> {
        let mut metadata = Vec::new();
        
        // Try to parse ID3
        if let Some(id3) = self.parse_id3(data, pts, dts) {
            metadata.push(MetadataType::Id3(id3));
        }
        
        // Try to parse EMSG
        if let Some(emsg) = self.parse_emsg(data) {
            metadata.push(MetadataType::Emsg(emsg));
        }
        
        metadata
    }
    
    /// Reset parser state
    pub fn reset(&mut self) {
        self.date_ranges.clear();
        self.last_id3_pts = None;
    }
}

impl Default for MetadataParser {
    fn default() -> Self {
        Self::new()
    }
}

/// Read a null-terminated string from data
fn read_null_terminated_string(data: &[u8], offset: &mut usize) -> Option<String> {
    let start = *offset;
    let end = data[start..].iter().position(|&b| b == 0)?;
    let s = String::from_utf8(data[start..start + end].to_vec()).ok()?;
    *offset = start + end + 1;
    Some(s)
}

/// Parse hex string to bytes
fn parse_hex(s: &str) -> Vec<u8> {
    let s = s.trim_start_matches("0x").trim_start_matches("0X");
    (0..s.len())
        .step_by(2)
        .filter_map(|i| u8::from_str_radix(&s[i..i + 2], 16).ok())
        .collect()
}

/// SAMPLE-AES decryptor for MPEG-2 TS segments
#[allow(dead_code)]
pub struct SampleAesDecryptor {
    /// AES key (16 bytes)
    key: Vec<u8>,
    /// Initialization vector (16 bytes)
    iv: Vec<u8>,
}

impl SampleAesDecryptor {
    pub fn new(key: Vec<u8>, iv: Vec<u8>) -> Self {
        Self { key, iv }
    }
    
    /// Decrypt SAMPLE-AES encrypted MPEG-2 TS segment
    /// 
    /// SAMPLE-AES encrypts only the NAL units, not the TS container.
    /// The encryption is applied to video NAL units (H.264) and audio frames (AAC).
    pub async fn decrypt_ts(&self, data: &[u8]) -> Result<Vec<u8>, String> {
        // Note: Full SAMPLE-AES implementation would need to:
        // 1. Parse MPEG-2 TS packets
        // 2. Find PES packets containing video/audio
        // 3. Extract NAL units from video PES
        // 4. Decrypt only certain bytes within NAL units (not headers)
        // 5. Decrypt AAC frames (first 16 bytes of each frame)
        
        // This is a simplified placeholder - a full implementation would use
        // Web Crypto API for AES-128-CBC decryption of the appropriate sections
        
        log::warn!("SAMPLE-AES decryption is not yet fully implemented");
        Ok(data.to_vec())
    }
    
    /// Decrypt a single encrypted block using AES-128-CBC
    /// Note: This requires the SubtleCrypto feature in web-sys
    pub async fn decrypt_block(&self, encrypted: &[u8]) -> Result<Vec<u8>, String> {
        // Web Crypto API requires dynamic JavaScript calls since the Rust bindings
        // don't cover all SubtleCrypto methods. For a full implementation, 
        // we would use js_sys::Reflect to call the methods dynamically.
        
        // For now, return a placeholder that indicates decryption is needed
        log::warn!("AES decryption block called - full implementation requires SubtleCrypto bindings");
        
        // In a production implementation, you would:
        // 1. Call subtle.importKey() to import the AES key
        // 2. Call subtle.decrypt() with the algorithm and IV
        // 3. Return the decrypted data
        
        // Return input unchanged as placeholder
        Ok(encrypted.to_vec())
    }
}
