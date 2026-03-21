//! Port of `dash.js/src/dash/models/DashManifestModel.js`.
//!
//! Model for accessing parsed MPD manifest data.

use crate::dash::vo::adaptation_set::AdaptationSet;
use crate::dash::vo::base_url::BaseUrl;
use crate::dash::vo::content_protection::ContentProtection;
use crate::dash::vo::descriptor_type::DescriptorType;
use crate::dash::vo::mpd::{Mpd, PresentationType};
use crate::dash::vo::period::Period;
use crate::dash::vo::representation::Representation;
use crate::dash::vo::utc_timing::UtcTiming;

/// Model that provides structured access to parsed MPD manifest data.
pub struct DashManifestModel {
    mpd: Option<Mpd>,
}

impl Default for DashManifestModel {
    fn default() -> Self {
        Self::new()
    }
}

impl DashManifestModel {
    pub fn new() -> Self {
        Self { mpd: None }
    }

    /// Set the MPD manifest.
    pub fn set_mpd(&mut self, mpd: Mpd) {
        self.mpd = Some(mpd);
    }

    /// Get a reference to the MPD manifest.
    pub fn get_mpd(&self) -> Option<&Mpd> {
        self.mpd.as_ref()
    }

    /// Check if the manifest is dynamic (live).
    pub fn is_dynamic(&self) -> bool {
        self.mpd
            .as_ref()
            .map_or(false, |m| m.type_ == PresentationType::Dynamic)
    }

    /// Get the media presentation duration in seconds.
    pub fn get_duration(&self) -> Option<f64> {
        self.mpd.as_ref().and_then(|m| m.media_presentation_duration)
    }

    /// Get all periods.
    pub fn get_periods(&self) -> &[Period] {
        self.mpd.as_ref().map_or(&[], |m| &m.periods)
    }

    /// Get a period by index.
    pub fn get_period(&self, index: usize) -> Option<&Period> {
        self.mpd.as_ref().and_then(|m| m.periods.get(index))
    }

    /// Get adaptation sets for a given period index.
    pub fn get_adaptation_sets_for_period(&self, period_index: usize) -> &[AdaptationSet] {
        self.get_period(period_index)
            .map_or(&[], |p| &p.adaptation_sets)
    }

    /// Get a specific adaptation set within a period.
    pub fn get_adaptation_set(
        &self,
        period_index: usize,
        as_index: usize,
    ) -> Option<&AdaptationSet> {
        self.get_period(period_index)
            .and_then(|p| p.adaptation_sets.get(as_index))
    }

    /// Get representations for a given adaptation set.
    pub fn get_representations_for(
        &self,
        period_index: usize,
        as_index: usize,
    ) -> &[Representation] {
        self.get_adaptation_set(period_index, as_index)
            .map_or(&[], |a| &a.representations)
    }

    /// Get a representation by its index within an adaptation set.
    pub fn get_representation_for(
        &self,
        period_index: usize,
        as_index: usize,
        rep_index: usize,
    ) -> Option<&Representation> {
        self.get_adaptation_set(period_index, as_index)
            .and_then(|a| a.representations.get(rep_index))
    }

    /// Get adaptation sets matching a specific content type for a period.
    pub fn get_adaptation_sets_for_type(
        &self,
        period_index: usize,
        content_type: &str,
    ) -> Vec<&AdaptationSet> {
        self.get_adaptation_sets_for_period(period_index)
            .iter()
            .filter(|aset| {
                aset.content_type.as_deref() == Some(content_type)
                    || aset
                        .mime_type
                        .as_ref()
                        .map_or(false, |m| m.starts_with(&format!("{content_type}/")))
            })
            .collect()
    }

    /// Get all BaseURLs at the MPD level.
    pub fn get_base_urls(&self) -> &[BaseUrl] {
        self.mpd.as_ref().map_or(&[], |m| &m.base_urls)
    }

    /// Get UTCTiming elements.
    pub fn get_utc_timing_sources(&self) -> &[UtcTiming] {
        self.mpd.as_ref().map_or(&[], |m| &m.utc_timing)
    }

    /// Get content protection descriptors for an adaptation set.
    pub fn get_content_protection_for(
        &self,
        period_index: usize,
        as_index: usize,
    ) -> &[ContentProtection] {
        self.get_adaptation_set(period_index, as_index)
            .map_or(&[], |a| &a.content_protection)
    }

    /// Get role descriptors for an adaptation set.
    pub fn get_roles_for(&self, period_index: usize, as_index: usize) -> &[DescriptorType] {
        self.get_adaptation_set(period_index, as_index)
            .map_or(&[], |a| &a.role)
    }

    /// Get accessibility descriptors for an adaptation set.
    pub fn get_accessibility_for(
        &self,
        period_index: usize,
        as_index: usize,
    ) -> &[DescriptorType] {
        self.get_adaptation_set(period_index, as_index)
            .map_or(&[], |a| &a.accessibility)
    }

    /// Get codec string for an adaptation set (from the set itself or first representation).
    pub fn get_codecs_for(&self, period_index: usize, as_index: usize) -> Option<String> {
        let aset = self.get_adaptation_set(period_index, as_index)?;
        aset.codecs
            .clone()
            .or_else(|| aset.representations.first().and_then(|r| r.codecs.clone()))
    }

    /// Get language for an adaptation set.
    pub fn get_lang_for(&self, period_index: usize, as_index: usize) -> Option<&str> {
        self.get_adaptation_set(period_index, as_index)
            .and_then(|a| a.lang.as_deref())
    }

    /// Get the minimum buffer time from the MPD.
    pub fn get_min_buffer_time(&self) -> Option<f64> {
        self.mpd.as_ref().and_then(|m| m.min_buffer_time)
    }

    /// Get the minimum update period from the MPD.
    pub fn get_minimum_update_period(&self) -> Option<f64> {
        self.mpd.as_ref().and_then(|m| m.minimum_update_period)
    }

    /// Get the time shift buffer depth from the MPD.
    pub fn get_time_shift_buffer_depth(&self) -> Option<f64> {
        self.mpd.as_ref().and_then(|m| m.time_shift_buffer_depth)
    }

    /// Get the suggested presentation delay from the MPD.
    pub fn get_suggested_presentation_delay(&self) -> Option<f64> {
        self.mpd.as_ref().and_then(|m| m.suggested_presentation_delay)
    }

    /// Get the availability start time as a string.
    pub fn get_availability_start_time(&self) -> Option<&str> {
        self.mpd
            .as_ref()
            .and_then(|m| m.availability_start_time.as_deref())
    }

    /// Get number of periods.
    pub fn get_number_of_periods(&self) -> usize {
        self.mpd.as_ref().map_or(0, |m| m.periods.len())
    }

    /// Clear the stored manifest.
    pub fn reset(&mut self) {
        self.mpd = None;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::dash::parser;

    fn parse_test_mpd() -> Mpd {
        let xml = r#"<?xml version="1.0"?>
        <MPD type="static" mediaPresentationDuration="PT60S" minBufferTime="PT2S">
          <BaseURL>https://cdn.example.com/</BaseURL>
          <Period id="p1" start="PT0S" duration="PT30S">
            <AdaptationSet mimeType="video/mp4" contentType="video" codecs="avc1.42c01e" lang="en">
              <Role schemeIdUri="urn:mpeg:dash:role:2011" value="main"/>
              <ContentProtection schemeIdUri="urn:mpeg:dash:mp4protection:2011" value="cenc"/>
              <Representation id="low" bandwidth="500000" width="640" height="360">
                <SegmentTemplate media="video-$Number$.m4s" initialization="video-init.mp4"
                                 timescale="1000" duration="2000" startNumber="1"/>
              </Representation>
              <Representation id="high" bandwidth="2000000" width="1920" height="1080">
                <SegmentTemplate media="video-$Number$.m4s" initialization="video-init.mp4"
                                 timescale="1000" duration="2000" startNumber="1"/>
              </Representation>
            </AdaptationSet>
            <AdaptationSet mimeType="audio/mp4" contentType="audio" lang="en">
              <Representation id="audio" bandwidth="128000" codecs="mp4a.40.2">
                <SegmentTemplate media="audio-$Number$.m4s" initialization="audio-init.mp4"
                                 timescale="44100" duration="88200" startNumber="1"/>
              </Representation>
            </AdaptationSet>
          </Period>
          <Period id="p2" start="PT30S" duration="PT30S">
            <AdaptationSet mimeType="video/mp4" contentType="video">
              <Representation id="1" bandwidth="1000000" codecs="avc1.42c01e">
                <SegmentTemplate media="p2-$Number$.m4s" initialization="p2-init.mp4"
                                 timescale="1000" duration="2000" startNumber="1"/>
              </Representation>
            </AdaptationSet>
          </Period>
        </MPD>"#;
        parser::parse(xml).unwrap()
    }

    #[test]
    fn test_basic_model() {
        let mpd = parse_test_mpd();
        let mut model = DashManifestModel::new();
        model.set_mpd(mpd);

        assert!(!model.is_dynamic());
        assert!((model.get_duration().unwrap() - 60.0).abs() < 0.01);
        assert!((model.get_min_buffer_time().unwrap() - 2.0).abs() < 0.01);
    }

    #[test]
    fn test_periods() {
        let mpd = parse_test_mpd();
        let mut model = DashManifestModel::new();
        model.set_mpd(mpd);

        assert_eq!(model.get_number_of_periods(), 2);
        assert_eq!(model.get_period(0).unwrap().id.as_deref(), Some("p1"));
        assert_eq!(model.get_period(1).unwrap().id.as_deref(), Some("p2"));
    }

    #[test]
    fn test_adaptation_sets() {
        let mpd = parse_test_mpd();
        let mut model = DashManifestModel::new();
        model.set_mpd(mpd);

        let as_list = model.get_adaptation_sets_for_period(0);
        assert_eq!(as_list.len(), 2);

        let video_sets = model.get_adaptation_sets_for_type(0, "video");
        assert_eq!(video_sets.len(), 1);

        let audio_sets = model.get_adaptation_sets_for_type(0, "audio");
        assert_eq!(audio_sets.len(), 1);
    }

    #[test]
    fn test_representations() {
        let mpd = parse_test_mpd();
        let mut model = DashManifestModel::new();
        model.set_mpd(mpd);

        let reps = model.get_representations_for(0, 0);
        assert_eq!(reps.len(), 2);
        assert_eq!(reps[0].id.as_deref(), Some("low"));
        assert_eq!(reps[1].id.as_deref(), Some("high"));

        let rep = model.get_representation_for(0, 0, 1).unwrap();
        assert_eq!(rep.bandwidth, Some(2000000));
    }

    #[test]
    fn test_codecs_and_lang() {
        let mpd = parse_test_mpd();
        let mut model = DashManifestModel::new();
        model.set_mpd(mpd);

        assert_eq!(
            model.get_codecs_for(0, 0).as_deref(),
            Some("avc1.42c01e")
        );
        assert_eq!(model.get_lang_for(0, 0), Some("en"));
    }

    #[test]
    fn test_content_protection() {
        let mpd = parse_test_mpd();
        let mut model = DashManifestModel::new();
        model.set_mpd(mpd);

        let cp = model.get_content_protection_for(0, 0);
        assert_eq!(cp.len(), 1);
        assert_eq!(
            cp[0].scheme_id_uri.as_deref(),
            Some("urn:mpeg:dash:mp4protection:2011")
        );
    }

    #[test]
    fn test_roles() {
        let mpd = parse_test_mpd();
        let mut model = DashManifestModel::new();
        model.set_mpd(mpd);

        let roles = model.get_roles_for(0, 0);
        assert_eq!(roles.len(), 1);
        assert_eq!(roles[0].value.as_deref(), Some("main"));
    }

    #[test]
    fn test_base_urls() {
        let mpd = parse_test_mpd();
        let mut model = DashManifestModel::new();
        model.set_mpd(mpd);

        let base_urls = model.get_base_urls();
        assert_eq!(base_urls.len(), 1);
        assert_eq!(base_urls[0].url, "https://cdn.example.com/");
    }

    #[test]
    fn test_reset() {
        let mpd = parse_test_mpd();
        let mut model = DashManifestModel::new();
        model.set_mpd(mpd);
        model.reset();
        assert!(model.get_mpd().is_none());
        assert_eq!(model.get_number_of_periods(), 0);
    }

    #[test]
    fn test_empty_model() {
        let model = DashManifestModel::new();
        assert!(!model.is_dynamic());
        assert!(model.get_duration().is_none());
        assert!(model.get_period(0).is_none());
        assert_eq!(model.get_number_of_periods(), 0);
    }
}
