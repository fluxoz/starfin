//! Port of `dash.js/src/dash/DashAdapter.js`.
//!
//! Adapter between the streaming layer and the DASH manifest model.

use crate::dash::vo::adaptation_set::AdaptationSet;
use crate::dash::vo::media_info::{BitrateListEntry, MediaInfo};
use crate::dash::vo::mpd::{Mpd, PresentationType};
use crate::dash::vo::period::Period;
use crate::dash::vo::representation::Representation;
use crate::dash::vo::stream_info::StreamInfo;

/// DashAdapter provides a high-level API for extracting media information
/// from a parsed MPD manifest.
pub struct DashAdapter {
    mpd: Mpd,
}

impl DashAdapter {
    pub fn new(mpd: Mpd) -> Self {
        Self { mpd }
    }

    /// Get a reference to the underlying MPD.
    pub fn get_mpd(&self) -> &Mpd {
        &self.mpd
    }

    /// Update the MPD (e.g. after a manifest refresh).
    pub fn set_mpd(&mut self, mpd: Mpd) {
        self.mpd = mpd;
    }

    /// Whether this is a dynamic (live) manifest.
    pub fn is_dynamic(&self) -> bool {
        self.mpd.type_ == PresentationType::Dynamic
    }

    /// Get stream information for all periods.
    pub fn get_streams_info(&self) -> Vec<StreamInfo> {
        self.mpd
            .periods
            .iter()
            .enumerate()
            .map(|(i, period)| self.build_stream_info(period, i))
            .collect()
    }

    /// Get media info for a specific type (video, audio, text) in a period.
    pub fn get_media_info_for_type(
        &self,
        stream_info: &StreamInfo,
        media_type: &str,
    ) -> Option<MediaInfo> {
        let period_index = stream_info.index? as usize;
        let period = self.mpd.periods.get(period_index)?;

        let matching_sets: Vec<&AdaptationSet> = period
            .adaptation_sets
            .iter()
            .filter(|aset| self.matches_type(aset, media_type))
            .collect();

        // Return the first matching (main/default) adaptation set
        matching_sets.first().map(|aset| {
            self.build_media_info(aset, stream_info, media_type)
        })
    }

    /// Get all media info entries for a specific type in a period.
    pub fn get_all_media_info_for_type(
        &self,
        stream_info: &StreamInfo,
        media_type: &str,
    ) -> Vec<MediaInfo> {
        let period_index = match stream_info.index {
            Some(i) => i as usize,
            None => return Vec::new(),
        };
        let period = match self.mpd.periods.get(period_index) {
            Some(p) => p,
            None => return Vec::new(),
        };

        period
            .adaptation_sets
            .iter()
            .filter(|aset| self.matches_type(aset, media_type))
            .map(|aset| self.build_media_info(aset, stream_info, media_type))
            .collect()
    }

    /// Get the adaptation set that corresponds to a media info.
    pub fn get_adaptation_for_media_info(&self, media_info: &MediaInfo) -> Option<&AdaptationSet> {
        let stream_info = media_info.stream_info.as_ref()?;
        let period_index = stream_info.index? as usize;
        let period = self.mpd.periods.get(period_index)?;
        let as_index = media_info.index? as usize;
        period.adaptation_sets.get(as_index)
    }

    /// Get a representation by quality index within a media info.
    pub fn get_representation_for_quality_index(
        &self,
        quality: u32,
        media_info: &MediaInfo,
    ) -> Option<&Representation> {
        let aset = self.get_adaptation_for_media_info(media_info)?;
        aset.representations.get(quality as usize)
    }

    /// Resolve the list of BaseURLs for a representation (inherited from MPD→Period→AS→Rep).
    pub fn get_urls_for_representation(
        &self,
        period_index: usize,
        as_index: usize,
        rep_index: usize,
    ) -> Vec<String> {
        let mut urls = Vec::new();

        // MPD-level BaseURLs
        for bu in &self.mpd.base_urls {
            urls.push(bu.url.clone());
        }

        if let Some(period) = self.mpd.periods.get(period_index) {
            // Period-level BaseURLs
            if !period.base_urls.is_empty() {
                let mut new_urls = Vec::new();
                for base in &period.base_urls {
                    if urls.is_empty() {
                        new_urls.push(base.url.clone());
                    } else {
                        for parent in &urls {
                            new_urls.push(resolve_url(&base.url, parent));
                        }
                    }
                }
                urls = new_urls;
            }

            if let Some(aset) = period.adaptation_sets.get(as_index) {
                // AdaptationSet-level BaseURLs
                if !aset.base_urls.is_empty() {
                    let mut new_urls = Vec::new();
                    for base in &aset.base_urls {
                        if urls.is_empty() {
                            new_urls.push(base.url.clone());
                        } else {
                            for parent in &urls {
                                new_urls.push(resolve_url(&base.url, parent));
                            }
                        }
                    }
                    urls = new_urls;
                }

                if let Some(rep) = aset.representations.get(rep_index) {
                    // Representation-level BaseURLs
                    if !rep.base_urls.is_empty() {
                        let mut new_urls = Vec::new();
                        for base in &rep.base_urls {
                            if urls.is_empty() {
                                new_urls.push(base.url.clone());
                            } else {
                                for parent in &urls {
                                    new_urls.push(resolve_url(&base.url, parent));
                                }
                            }
                        }
                        urls = new_urls;
                    }
                }
            }
        }

        urls
    }

    /// Get the codec string for an adaptation set.
    pub fn get_codec_for_adaptation(aset: &AdaptationSet) -> Option<String> {
        // First check the adaptation set's codecs
        if aset.codecs.is_some() {
            return aset.codecs.clone();
        }
        // Fall back to the first representation's codecs
        aset.representations.first().and_then(|r| r.codecs.clone())
    }

    /// Get all regular periods from the manifest.
    pub fn get_regular_periods(&self) -> Vec<&Period> {
        self.mpd.periods.iter().collect()
    }

    fn build_stream_info(&self, period: &Period, index: usize) -> StreamInfo {
        StreamInfo {
            id: period.id.clone().or_else(|| Some(format!("period_{index}"))),
            index: Some(index as u32),
            start: period.start,
            duration: period.duration,
            is_last: index == self.mpd.periods.len() - 1,
            is_encrypted: period.is_encrypted,
            manifest_info: None,
        }
    }

    fn build_media_info(
        &self,
        aset: &AdaptationSet,
        stream_info: &StreamInfo,
        media_type: &str,
    ) -> MediaInfo {
        let bitrate_list: Vec<BitrateListEntry> = aset
            .representations
            .iter()
            .map(|rep| BitrateListEntry {
                bandwidth: rep.bandwidth,
                width: rep.width,
                height: rep.height,
                scan_type: rep.scan_type.clone(),
                id: rep.id.clone(),
            })
            .collect();

        let codec = Self::get_codec_for_adaptation(aset);

        MediaInfo {
            id: aset.id.clone(),
            index: Some(aset.index as u32),
            type_: Some(media_type.to_string()),
            stream_info: Some(stream_info.clone()),
            representation_count: aset.representations.len() as u32,
            lang: aset.lang.clone(),
            codec,
            mime_type: aset.mime_type.clone(),
            content_protection: if aset.content_protection.is_empty() {
                None
            } else {
                Some(aset.content_protection.clone())
            },
            bitrate_list: Some(bitrate_list),
            roles: if aset.role.is_empty() {
                None
            } else {
                Some(aset.role.clone())
            },
            accessibility: if aset.accessibility.is_empty() {
                None
            } else {
                Some(aset.accessibility.clone())
            },
            audio_channel_configuration: if aset.audio_channel_configuration.is_empty() {
                None
            } else {
                Some(aset.audio_channel_configuration.clone())
            },
            supplemental_properties: aset.supplemental_property.clone(),
            essential_properties: aset.essential_property.clone(),
            segment_alignment: aset.segment_alignment,
            sub_segment_alignment: aset.subsegment_alignment,
            ..MediaInfo::default()
        }
    }

    fn matches_type(&self, aset: &AdaptationSet, media_type: &str) -> bool {
        // Check contentType attribute
        if let Some(ref ct) = aset.content_type {
            if ct == media_type {
                return true;
            }
        }
        // Check mimeType
        if let Some(ref mt) = aset.mime_type {
            if mt.starts_with(&format!("{media_type}/")) {
                return true;
            }
        }
        // Check representations' mimeType
        aset.representations.iter().any(|rep| {
            rep.mime_type
                .as_ref()
                .map_or(false, |mt| mt.starts_with(&format!("{media_type}/")))
        })
    }
}

/// Resolve a relative URL against a base URL.
fn resolve_url(url: &str, base: &str) -> String {
    // If url is absolute, return as-is
    if url.starts_with("http://") || url.starts_with("https://") || url.starts_with("//") {
        return url.to_string();
    }

    // Simple concatenation for relative URLs
    if base.ends_with('/') {
        format!("{base}{url}")
    } else {
        format!("{base}/{url}")
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::dash::parser;

    fn parse_test_mpd() -> Mpd {
        let xml = r#"<?xml version="1.0" encoding="UTF-8"?>
        <MPD type="static" mediaPresentationDuration="PT30S" minBufferTime="PT2S">
          <BaseURL>https://cdn.example.com/</BaseURL>
          <Period id="1" duration="PT30S">
            <AdaptationSet mimeType="video/mp4" contentType="video" codecs="avc1.42c01e">
              <Representation id="low" bandwidth="500000" width="640" height="360">
                <SegmentTemplate media="video-low-$Number$.m4s" initialization="video-low-init.mp4"
                                 timescale="1000" duration="2000" startNumber="1"/>
              </Representation>
              <Representation id="high" bandwidth="2000000" width="1920" height="1080" codecs="avc1.640028">
                <SegmentTemplate media="video-high-$Number$.m4s" initialization="video-high-init.mp4"
                                 timescale="1000" duration="2000" startNumber="1"/>
              </Representation>
            </AdaptationSet>
            <AdaptationSet mimeType="audio/mp4" contentType="audio" lang="en" codecs="mp4a.40.2">
              <Role schemeIdUri="urn:mpeg:dash:role:2011" value="main"/>
              <Representation id="audio" bandwidth="128000">
                <SegmentTemplate media="audio-$Number$.m4s" initialization="audio-init.mp4"
                                 timescale="44100" duration="88200" startNumber="1"/>
              </Representation>
            </AdaptationSet>
          </Period>
        </MPD>"#;
        parser::parse(xml).unwrap()
    }

    #[test]
    fn test_is_dynamic() {
        let mpd = parse_test_mpd();
        let adapter = DashAdapter::new(mpd);
        assert!(!adapter.is_dynamic());
    }

    #[test]
    fn test_get_streams_info() {
        let mpd = parse_test_mpd();
        let adapter = DashAdapter::new(mpd);
        let streams = adapter.get_streams_info();
        assert_eq!(streams.len(), 1);
        assert_eq!(streams[0].id.as_deref(), Some("1"));
        assert!(streams[0].is_last);
    }

    #[test]
    fn test_get_media_info_for_video() {
        let mpd = parse_test_mpd();
        let adapter = DashAdapter::new(mpd);
        let streams = adapter.get_streams_info();
        let mi = adapter.get_media_info_for_type(&streams[0], "video").unwrap();
        assert_eq!(mi.type_.as_deref(), Some("video"));
        assert_eq!(mi.representation_count, 2);
        assert_eq!(mi.mime_type.as_deref(), Some("video/mp4"));

        let bl = mi.bitrate_list.unwrap();
        assert_eq!(bl.len(), 2);
        assert_eq!(bl[0].bandwidth, Some(500000));
        assert_eq!(bl[1].bandwidth, Some(2000000));
    }

    #[test]
    fn test_get_media_info_for_audio() {
        let mpd = parse_test_mpd();
        let adapter = DashAdapter::new(mpd);
        let streams = adapter.get_streams_info();
        let mi = adapter.get_media_info_for_type(&streams[0], "audio").unwrap();
        assert_eq!(mi.type_.as_deref(), Some("audio"));
        assert_eq!(mi.lang.as_deref(), Some("en"));
        assert_eq!(mi.representation_count, 1);
    }

    #[test]
    fn test_get_all_media_info() {
        let mpd = parse_test_mpd();
        let adapter = DashAdapter::new(mpd);
        let streams = adapter.get_streams_info();
        let all_video = adapter.get_all_media_info_for_type(&streams[0], "video");
        assert_eq!(all_video.len(), 1);
        let all_audio = adapter.get_all_media_info_for_type(&streams[0], "audio");
        assert_eq!(all_audio.len(), 1);
    }

    #[test]
    fn test_get_representation_for_quality() {
        let mpd = parse_test_mpd();
        let adapter = DashAdapter::new(mpd);
        let streams = adapter.get_streams_info();
        let mi = adapter.get_media_info_for_type(&streams[0], "video").unwrap();

        let rep0 = adapter.get_representation_for_quality_index(0, &mi).unwrap();
        assert_eq!(rep0.bandwidth, Some(500000));

        let rep1 = adapter.get_representation_for_quality_index(1, &mi).unwrap();
        assert_eq!(rep1.bandwidth, Some(2000000));
    }

    #[test]
    fn test_get_urls_for_representation() {
        let mpd = parse_test_mpd();
        let adapter = DashAdapter::new(mpd);
        let urls = adapter.get_urls_for_representation(0, 0, 0);
        assert_eq!(urls, vec!["https://cdn.example.com/"]);
    }

    #[test]
    fn test_get_codec_for_adaptation() {
        let mpd = parse_test_mpd();
        let aset = &mpd.periods[0].adaptation_sets[0];
        let codec = DashAdapter::get_codec_for_adaptation(aset);
        assert_eq!(codec.as_deref(), Some("avc1.42c01e"));
    }

    #[test]
    fn test_no_matching_type() {
        let mpd = parse_test_mpd();
        let adapter = DashAdapter::new(mpd);
        let streams = adapter.get_streams_info();
        let mi = adapter.get_media_info_for_type(&streams[0], "text");
        assert!(mi.is_none());
    }

    #[test]
    fn test_resolve_url() {
        assert_eq!(
            resolve_url("video/seg-1.m4s", "https://cdn.example.com/"),
            "https://cdn.example.com/video/seg-1.m4s"
        );
        assert_eq!(
            resolve_url("https://other.cdn/seg.m4s", "https://cdn.example.com/"),
            "https://other.cdn/seg.m4s"
        );
    }
}
