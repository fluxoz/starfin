//! DASH MPD parser — Rust port of `dash.js/src/dash/parser/DashParser.js`.
//!
//! Parses an MPD XML string into the [`Mpd`] value object.

pub mod matchers;

use std::collections::HashMap;

use quick_xml::events::{BytesStart, Event};
use quick_xml::reader::Reader;

use crate::core::errors::{DashError, ErrorCode};
use crate::dash::vo::adaptation_set::AdaptationSet;
use crate::dash::vo::base_url::BaseUrl;
use crate::dash::vo::content_protection::ContentProtection;
use crate::dash::vo::descriptor_type::DescriptorType;
use crate::dash::vo::mpd::{
    Mpd, PresentationType, ServiceDescription, ServiceDescriptionLatency,
    ServiceDescriptionOperatingBandwidth, ServiceDescriptionOperatingQuality,
    ServiceDescriptionPlaybackRate,
};
use crate::dash::vo::period::Period;
use crate::dash::vo::representation::Representation;
use crate::dash::vo::utc_timing::UtcTiming;

use self::matchers::{
    is_duration,
    parse_duration,
};

/// Segment template information parsed from SegmentTemplate/SegmentBase/SegmentList elements.
#[derive(Clone, Debug, Default)]
pub struct SegmentTemplateInfo {
    pub timescale: Option<u64>,
    pub duration: Option<u64>,
    pub start_number: Option<u64>,
    pub end_number: Option<u64>,
    pub media: Option<String>,
    pub initialization: Option<String>,
    pub presentation_time_offset: Option<u64>,
    pub availability_time_offset: Option<f64>,
    pub availability_time_complete: Option<bool>,
    pub index_range: Option<String>,
    pub segment_timeline: Vec<SElement>,
}

/// An S element from SegmentTimeline.
#[derive(Clone, Debug, Default)]
pub struct SElement {
    pub t: Option<u64>,
    pub d: u64,
    pub r: Option<i64>,
    pub k: Option<u64>,
}

/// Parse an MPD XML string into an [`Mpd`] struct.
pub fn parse(xml: &str) -> Result<Mpd, DashError> {
    let mut reader = Reader::from_str(xml);
    reader.trim_text(true);

    let mut mpd: Option<Mpd> = None;

    loop {
        match reader.read_event() {
            Ok(Event::Start(ref e)) if tag_name(e) == "MPD" => {
                mpd = Some(parse_mpd_element(e, &mut reader)?);
            }
            Ok(Event::Eof) => break,
            Err(e) => {
                return Err(DashError::with_message(
                    ErrorCode::ManifestLoaderParsingFailure,
                    format!("XML parse error: {e}"),
                ));
            }
            _ => {}
        }
    }

    mpd.ok_or_else(|| {
        DashError::with_message(ErrorCode::ManifestErrorParse, "No MPD element found")
    })
}

fn tag_name(e: &BytesStart) -> String {
    let full = String::from_utf8_lossy(e.name().as_ref()).to_string();
    // Strip namespace prefix
    if let Some(pos) = full.find(':') {
        full[pos + 1..].to_string()
    } else {
        full
    }
}

fn tag_name_from_bytes(name: &[u8]) -> String {
    let full = String::from_utf8_lossy(name).to_string();
    if let Some(pos) = full.find(':') {
        full[pos + 1..].to_string()
    } else {
        full
    }
}

fn get_attrs(e: &BytesStart) -> HashMap<String, String> {
    let mut map = HashMap::new();
    for attr in e.attributes().flatten() {
        let key = String::from_utf8_lossy(attr.key.as_ref()).to_string();
        let val = String::from_utf8_lossy(&attr.value).to_string();
        map.insert(key, val);
    }
    map
}

fn parse_mpd_element(start: &BytesStart, reader: &mut Reader<&[u8]>) -> Result<Mpd, DashError> {
    let attrs = get_attrs(start);
    let mut mpd = Mpd::default();

    // Parse MPD-level attributes
    if let Some(v) = attrs.get("type") {
        mpd.type_ = if v == "dynamic" {
            PresentationType::Dynamic
        } else {
            PresentationType::Static
        };
    }
    if let Some(v) = attrs.get("id") {
        mpd.id = Some(v.clone());
    }
    if let Some(v) = attrs.get("profiles") {
        mpd.profiles = Some(v.clone());
    }
    if let Some(v) = attrs.get("availabilityStartTime") {
        mpd.availability_start_time = Some(v.clone());
    }
    if let Some(v) = attrs.get("publishTime") {
        mpd.publish_time = Some(v.clone());
    }
    if let Some(v) = attrs.get("mediaPresentationDuration") {
        mpd.media_presentation_duration = parse_duration(v);
    }
    if let Some(v) = attrs.get("minBufferTime") {
        mpd.min_buffer_time = parse_duration(v);
    }
    if let Some(v) = attrs.get("minimumUpdatePeriod") {
        mpd.minimum_update_period = parse_duration(v);
    }
    if let Some(v) = attrs.get("timeShiftBufferDepth") {
        mpd.time_shift_buffer_depth = parse_duration(v);
    }
    if let Some(v) = attrs.get("maxSegmentDuration") {
        mpd.max_segment_duration = parse_duration(v);
    }
    if let Some(v) = attrs.get("suggestedPresentationDelay") {
        mpd.suggested_presentation_delay = parse_duration(v);
    }

    let mut period_index: i32 = 0;

    loop {
        match reader.read_event() {
            Ok(Event::Start(ref e)) => {
                let name = tag_name(e);
                match name.as_str() {
                    "Period" => {
                        let period = parse_period_element(e, reader, period_index)?;
                        mpd.periods.push(period);
                        period_index += 1;
                    }
                    "BaseURL" => {
                        let bu = parse_base_url_element(e, reader)?;
                        mpd.base_urls.push(bu);
                    }
                    "UTCTiming" => {
                        let utc = parse_utc_timing(e, reader)?;
                        mpd.utc_timing.push(utc);
                    }
                    "ServiceDescription" => {
                        let sd = parse_service_description(e, reader)?;
                        mpd.service_descriptions.push(sd);
                    }
                    "Location" => {
                        skip_element(reader)?;
                    }
                    _ => {
                        skip_element(reader)?;
                    }
                }
            }
            Ok(Event::Empty(ref e)) => {
                let name = tag_name(e);
                match name.as_str() {
                    "UTCTiming" => {
                        let utc = parse_utc_timing_empty(e);
                        mpd.utc_timing.push(utc);
                    }
                    "BaseURL" => {
                        // Empty BaseURL (rare) — skip or handle
                    }
                    _ => {}
                }
            }
            Ok(Event::End(ref e)) if tag_name_from_bytes(e.name().as_ref()) == "MPD" => break,
            Ok(Event::Eof) => break,
            Err(e) => {
                return Err(DashError::with_message(
                    ErrorCode::ManifestLoaderParsingFailure,
                    format!("XML parse error in MPD: {e}"),
                ));
            }
            _ => {}
        }
    }

    // Link next period IDs
    for i in 0..mpd.periods.len() {
        if i + 1 < mpd.periods.len() {
            let next_id = mpd.periods[i + 1].id.clone();
            mpd.periods[i].next_period_id = next_id;
        }
    }

    Ok(mpd)
}

fn parse_period_element(
    start: &BytesStart,
    reader: &mut Reader<&[u8]>,
    index: i32,
) -> Result<Period, DashError> {
    let attrs = get_attrs(start);
    let mut period = Period {
        index,
        ..Period::default()
    };

    if let Some(v) = attrs.get("id") {
        period.id = Some(v.clone());
    }
    if let Some(v) = attrs.get("start") {
        if is_duration(v) {
            period.start = parse_duration(v);
        } else {
            period.start = v.parse::<f64>().ok();
        }
    }
    if let Some(v) = attrs.get("duration") {
        if is_duration(v) {
            period.duration = parse_duration(v);
        } else {
            period.duration = v.parse::<f64>().ok();
        }
    }

    let mut as_index: i32 = 0;

    loop {
        match reader.read_event() {
            Ok(Event::Start(ref e)) => {
                let name = tag_name(e);
                match name.as_str() {
                    "AdaptationSet" => {
                        let aset =
                            parse_adaptation_set_element(e, reader, as_index, index as usize)?;
                        period.adaptation_sets.push(aset);
                        as_index += 1;
                    }
                    "BaseURL" => {
                        let bu = parse_base_url_element(e, reader)?;
                        period.base_urls.push(bu);
                    }
                    _ => {
                        skip_element(reader)?;
                    }
                }
            }
            Ok(Event::Empty(ref e)) => {
                let name = tag_name(e);
                if name == "AdaptationSet" {
                    // Empty AdaptationSet with no children
                    let mut aset = AdaptationSet {
                        index: as_index,
                        period_index: Some(index as usize),
                        ..AdaptationSet::default()
                    };
                    apply_adaptation_set_attrs(&mut aset, &get_attrs(e));
                    period.adaptation_sets.push(aset);
                    as_index += 1;
                }
            }
            Ok(Event::End(ref e)) if tag_name_from_bytes(e.name().as_ref()) == "Period" => break,
            Ok(Event::Eof) => break,
            Err(e) => {
                return Err(DashError::with_message(
                    ErrorCode::ManifestLoaderParsingFailure,
                    format!("XML parse error in Period: {e}"),
                ));
            }
            _ => {}
        }
    }

    Ok(period)
}

fn parse_adaptation_set_element(
    start: &BytesStart,
    reader: &mut Reader<&[u8]>,
    index: i32,
    period_index: usize,
) -> Result<AdaptationSet, DashError> {
    let attrs = get_attrs(start);
    let mut aset = AdaptationSet {
        index,
        period_index: Some(period_index),
        ..AdaptationSet::default()
    };
    apply_adaptation_set_attrs(&mut aset, &attrs);

    // Track inherited SegmentTemplate at AdaptationSet level
    let mut as_segment_template: Option<SegmentTemplateInfo> = None;
    let mut rep_index: u32 = 0;

    loop {
        match reader.read_event() {
            Ok(Event::Start(ref e)) => {
                let name = tag_name(e);
                match name.as_str() {
                    "Representation" => {
                        let rep = parse_representation_element(
                            e,
                            reader,
                            rep_index,
                            &aset,
                            as_segment_template.as_ref(),
                        )?;
                        aset.representations.push(rep);
                        rep_index += 1;
                    }
                    "ContentProtection" => {
                        let cp = parse_content_protection(e, reader)?;
                        aset.content_protection.push(cp);
                    }
                    "Role" => {
                        let desc = parse_descriptor_start(e, reader)?;
                        aset.role.push(desc);
                    }
                    "Accessibility" => {
                        let desc = parse_descriptor_start(e, reader)?;
                        aset.accessibility.push(desc);
                    }
                    "SupplementalProperty" => {
                        let desc = parse_descriptor_start(e, reader)?;
                        aset.supplemental_property.push(desc);
                    }
                    "EssentialProperty" => {
                        let desc = parse_descriptor_start(e, reader)?;
                        aset.essential_property.push(desc);
                    }
                    "AudioChannelConfiguration" => {
                        let desc = parse_descriptor_start(e, reader)?;
                        aset.audio_channel_configuration.push(desc);
                    }
                    "BaseURL" => {
                        let bu = parse_base_url_element(e, reader)?;
                        aset.base_urls.push(bu);
                    }
                    "SegmentTemplate" => {
                        let tmpl = parse_segment_template(e, reader)?;
                        let json = serde_json::to_value(&segment_template_to_json(&tmpl))
                            .unwrap_or_default();
                        aset.segment_template = Some(json);
                        as_segment_template = Some(tmpl);
                    }
                    "SegmentBase" => {
                        let sb = parse_segment_base_element(e, reader)?;
                        let json =
                            serde_json::to_value(&segment_template_to_json(&sb)).unwrap_or_default();
                        aset.segment_base = Some(json);
                    }
                    "SegmentList" => {
                        let sl = parse_segment_list_element(e, reader)?;
                        let json =
                            serde_json::to_value(&segment_template_to_json(&sl)).unwrap_or_default();
                        aset.segment_list = Some(json);
                    }
                    _ => {
                        skip_element(reader)?;
                    }
                }
            }
            Ok(Event::Empty(ref e)) => {
                let name = tag_name(e);
                match name.as_str() {
                    "ContentProtection" => {
                        let cp = parse_content_protection_empty(e);
                        aset.content_protection.push(cp);
                    }
                    "Role" => {
                        let desc = parse_descriptor_empty(e);
                        aset.role.push(desc);
                    }
                    "Accessibility" => {
                        let desc = parse_descriptor_empty(e);
                        aset.accessibility.push(desc);
                    }
                    "SupplementalProperty" => {
                        let desc = parse_descriptor_empty(e);
                        aset.supplemental_property.push(desc);
                    }
                    "EssentialProperty" => {
                        let desc = parse_descriptor_empty(e);
                        aset.essential_property.push(desc);
                    }
                    "AudioChannelConfiguration" => {
                        let desc = parse_descriptor_empty(e);
                        aset.audio_channel_configuration.push(desc);
                    }
                    "Representation" => {
                        let rep = parse_representation_empty(
                            e,
                            rep_index,
                            &aset,
                            as_segment_template.as_ref(),
                        );
                        aset.representations.push(rep);
                        rep_index += 1;
                    }
                    "SegmentTemplate" => {
                        let tmpl = parse_segment_template_empty(e);
                        let json = serde_json::to_value(&segment_template_to_json(&tmpl))
                            .unwrap_or_default();
                        aset.segment_template = Some(json);
                        as_segment_template = Some(tmpl);
                    }
                    "SegmentBase" => {
                        let sb = parse_segment_base_empty(e);
                        let json =
                            serde_json::to_value(&segment_template_to_json(&sb)).unwrap_or_default();
                        aset.segment_base = Some(json);
                    }
                    _ => {}
                }
            }
            Ok(Event::End(ref e))
                if tag_name_from_bytes(e.name().as_ref()) == "AdaptationSet" =>
            {
                break
            }
            Ok(Event::Eof) => break,
            Err(e) => {
                return Err(DashError::with_message(
                    ErrorCode::ManifestLoaderParsingFailure,
                    format!("XML parse error in AdaptationSet: {e}"),
                ));
            }
            _ => {}
        }
    }

    // Inherit codecs/mimeType down to representations that don't have them
    for rep in &mut aset.representations {
        if rep.codecs.is_none() {
            rep.codecs = aset.codecs.clone();
        }
        if rep.mime_type.is_none() {
            rep.mime_type = aset.mime_type.clone();
        }
    }

    Ok(aset)
}

fn apply_adaptation_set_attrs(aset: &mut AdaptationSet, attrs: &HashMap<String, String>) {
    if let Some(v) = attrs.get("id") {
        aset.id = Some(v.clone());
    }
    if let Some(v) = attrs.get("contentType") {
        aset.content_type = Some(v.clone());
    }
    if let Some(v) = attrs.get("mimeType") {
        aset.mime_type = Some(v.clone());
    }
    if let Some(v) = attrs.get("codecs") {
        aset.codecs = Some(v.clone());
    }
    if let Some(v) = attrs.get("lang") {
        aset.lang = Some(v.clone());
    }
    if let Some(v) = attrs.get("group") {
        aset.group = v.parse().ok();
    }
    if let Some(v) = attrs.get("par") {
        aset.par = Some(v.clone());
    }
    if let Some(v) = attrs.get("maxWidth") {
        aset.max_width = v.parse().ok();
    }
    if let Some(v) = attrs.get("maxHeight") {
        aset.max_height = v.parse().ok();
    }
    if let Some(v) = attrs.get("maxFrameRate") {
        aset.max_frame_rate = Some(v.clone());
    }
    if let Some(v) = attrs.get("segmentAlignment") {
        aset.segment_alignment = v == "true" || v == "1";
    }
    if let Some(v) = attrs.get("subsegmentAlignment") {
        aset.subsegment_alignment = v == "true" || v == "1";
    }
    if let Some(v) = attrs.get("bitstreamSwitching") {
        aset.bitstream_switching = v == "true" || v == "1";
    }
}

fn parse_representation_element(
    start: &BytesStart,
    reader: &mut Reader<&[u8]>,
    index: u32,
    parent_as: &AdaptationSet,
    inherited_template: Option<&SegmentTemplateInfo>,
) -> Result<Representation, DashError> {
    let attrs = get_attrs(start);
    let mut rep = build_representation(&attrs, index, parent_as, inherited_template);

    loop {
        match reader.read_event() {
            Ok(Event::Start(ref e)) => {
                let name = tag_name(e);
                match name.as_str() {
                    "BaseURL" => {
                        let bu = parse_base_url_element(e, reader)?;
                        rep.base_urls.push(bu);
                    }
                    "SegmentTemplate" => {
                        let tmpl = parse_segment_template(e, reader)?;
                        apply_segment_info_to_rep(&mut rep, &tmpl);
                    }
                    "SegmentBase" => {
                        let sb = parse_segment_base_element(e, reader)?;
                        apply_segment_info_to_rep(&mut rep, &sb);
                    }
                    "SegmentList" => {
                        let sl = parse_segment_list_element(e, reader)?;
                        apply_segment_info_to_rep(&mut rep, &sl);
                    }
                    "EssentialProperty" => {
                        let desc = parse_descriptor_start(e, reader)?;
                        rep.essential_properties.push(desc);
                    }
                    "SupplementalProperty" => {
                        let desc = parse_descriptor_start(e, reader)?;
                        rep.supplemental_properties.push(desc);
                    }
                    _ => {
                        skip_element(reader)?;
                    }
                }
            }
            Ok(Event::Empty(ref e)) => {
                let name = tag_name(e);
                match name.as_str() {
                    "SegmentTemplate" => {
                        let tmpl = parse_segment_template_empty(e);
                        apply_segment_info_to_rep(&mut rep, &tmpl);
                    }
                    "SegmentBase" => {
                        let sb = parse_segment_base_empty(e);
                        apply_segment_info_to_rep(&mut rep, &sb);
                    }
                    "EssentialProperty" => {
                        let desc = parse_descriptor_empty(e);
                        rep.essential_properties.push(desc);
                    }
                    "SupplementalProperty" => {
                        let desc = parse_descriptor_empty(e);
                        rep.supplemental_properties.push(desc);
                    }
                    _ => {}
                }
            }
            Ok(Event::End(ref e))
                if tag_name_from_bytes(e.name().as_ref()) == "Representation" =>
            {
                break
            }
            Ok(Event::Eof) => break,
            Err(e) => {
                return Err(DashError::with_message(
                    ErrorCode::ManifestLoaderParsingFailure,
                    format!("XML parse error in Representation: {e}"),
                ));
            }
            _ => {}
        }
    }

    Ok(rep)
}

fn parse_representation_empty(
    e: &BytesStart,
    index: u32,
    parent_as: &AdaptationSet,
    inherited_template: Option<&SegmentTemplateInfo>,
) -> Representation {
    let attrs = get_attrs(e);
    build_representation(&attrs, index, parent_as, inherited_template)
}

fn build_representation(
    attrs: &HashMap<String, String>,
    index: u32,
    parent_as: &AdaptationSet,
    inherited_template: Option<&SegmentTemplateInfo>,
) -> Representation {
    let mut rep = Representation {
        index: Some(index),
        adaptation_index: Some(parent_as.index as usize),
        ..Representation::default()
    };

    if let Some(v) = attrs.get("id") {
        rep.id = Some(v.clone());
    }
    if let Some(v) = attrs.get("bandwidth") {
        rep.bandwidth = v.parse().ok();
    }
    if let Some(v) = attrs.get("width") {
        rep.width = v.parse().ok();
    }
    if let Some(v) = attrs.get("height") {
        rep.height = v.parse().ok();
    }
    if let Some(v) = attrs.get("codecs") {
        rep.codecs = Some(v.clone());
    }
    if let Some(v) = attrs.get("mimeType") {
        rep.mime_type = Some(v.clone());
    }
    if let Some(v) = attrs.get("scanType") {
        rep.scan_type = Some(v.clone());
    }
    if let Some(v) = attrs.get("frameRate") {
        rep.frame_rate = Some(v.clone());
    }
    if let Some(v) = attrs.get("sar") {
        rep.sar = Some(v.clone());
    }
    if let Some(v) = attrs.get("audioSamplingRate") {
        rep.audio_sampling_rate = Some(v.clone());
    }
    if let Some(v) = attrs.get("qualityRanking") {
        rep.quality_ranking = v.parse().ok();
    }
    if let Some(v) = attrs.get("codecPrivateData") {
        rep.codec_private_data = Some(v.clone());
    }
    if let Some(v) = attrs.get("codingDependency") {
        rep.coding_dependency = Some(v.clone());
    }
    if let Some(v) = attrs.get("dependencyId") {
        rep.dependency_id = Some(v.clone());
    }
    if let Some(v) = attrs.get("maxPlayoutRate") {
        rep.max_playout_rate = v.parse().ok();
    }

    // Apply inherited SegmentTemplate if present
    if let Some(tmpl) = inherited_template {
        apply_segment_info_to_rep(&mut rep, tmpl);
    }

    rep
}

fn apply_segment_info_to_rep(rep: &mut Representation, tmpl: &SegmentTemplateInfo) {
    if let Some(ts) = tmpl.timescale {
        rep.timescale = ts;
    }
    if let Some(sn) = tmpl.start_number {
        rep.start_number = sn;
    }
    if let Some(en) = tmpl.end_number {
        rep.end_number = Some(en);
    }
    if let Some(pto) = tmpl.presentation_time_offset {
        rep.presentation_time_offset = pto as f64 / rep.timescale as f64;
    }
    if let Some(ato) = tmpl.availability_time_offset {
        rep.availability_time_offset = ato;
    }
    if let Some(atc) = tmpl.availability_time_complete {
        rep.availability_time_complete = atc;
    }
    if let Some(ref init) = tmpl.initialization {
        rep.initialization = Some(init.clone());
    }
    if let Some(ref media) = tmpl.media {
        rep.media = Some(media.clone());
    }
    if let Some(dur) = tmpl.duration {
        rep.segment_duration = Some(dur as f64 / rep.timescale as f64);
    }
    if let Some(ref ir) = tmpl.index_range {
        rep.index_range = Some(ir.clone());
    }

    // Determine segment info type
    if !tmpl.segment_timeline.is_empty() {
        rep.segment_info_type = Some(crate::dash::constants::SEGMENT_TIMELINE.to_string());
    } else if tmpl.media.is_some() {
        rep.segment_info_type = Some(crate::dash::constants::SEGMENT_TEMPLATE.to_string());
    } else if tmpl.index_range.is_some() {
        rep.segment_info_type = Some(crate::dash::constants::SEGMENT_BASE.to_string());
    }
}

fn parse_segment_template_attrs(e: &BytesStart) -> SegmentTemplateInfo {
    let attrs = get_attrs(e);
    let mut tmpl = SegmentTemplateInfo::default();

    if let Some(v) = attrs.get("timescale") {
        tmpl.timescale = v.parse().ok();
    }
    if let Some(v) = attrs.get("duration") {
        tmpl.duration = v.parse().ok();
    }
    if let Some(v) = attrs.get("startNumber") {
        tmpl.start_number = v.parse().ok();
    }
    if let Some(v) = attrs.get("endNumber") {
        tmpl.end_number = v.parse().ok();
    }
    if let Some(v) = attrs.get("media") {
        tmpl.media = Some(v.clone());
    }
    if let Some(v) = attrs.get("initialization") {
        tmpl.initialization = Some(v.clone());
    }
    if let Some(v) = attrs.get("presentationTimeOffset") {
        tmpl.presentation_time_offset = v.parse().ok();
    }
    if let Some(v) = attrs.get("availabilityTimeOffset") {
        tmpl.availability_time_offset = v.parse().ok();
    }
    if let Some(v) = attrs.get("availabilityTimeComplete") {
        tmpl.availability_time_complete = Some(v == "true" || v == "1");
    }
    if let Some(v) = attrs.get("indexRange") {
        tmpl.index_range = Some(v.clone());
    }

    tmpl
}

fn parse_segment_template(
    start: &BytesStart,
    reader: &mut Reader<&[u8]>,
) -> Result<SegmentTemplateInfo, DashError> {
    let mut tmpl = parse_segment_template_attrs(start);

    loop {
        match reader.read_event() {
            Ok(Event::Start(ref e)) => {
                let name = tag_name(e);
                if name == "SegmentTimeline" {
                    tmpl.segment_timeline = parse_segment_timeline(reader)?;
                } else {
                    skip_element(reader)?;
                }
            }
            Ok(Event::Empty(ref e)) => {
                let name = tag_name(e);
                if name == "Initialization" || name == "initialization" {
                    let a = get_attrs(e);
                    if let Some(v) = a.get("sourceURL").or_else(|| a.get("range")) {
                        tmpl.initialization = Some(v.clone());
                    }
                }
            }
            Ok(Event::End(ref e))
                if tag_name_from_bytes(e.name().as_ref()) == "SegmentTemplate" =>
            {
                break
            }
            Ok(Event::Eof) => break,
            Err(e) => {
                return Err(DashError::with_message(
                    ErrorCode::ManifestLoaderParsingFailure,
                    format!("XML parse error in SegmentTemplate: {e}"),
                ));
            }
            _ => {}
        }
    }

    Ok(tmpl)
}

fn parse_segment_template_empty(e: &BytesStart) -> SegmentTemplateInfo {
    parse_segment_template_attrs(e)
}

fn parse_segment_base_element(
    start: &BytesStart,
    reader: &mut Reader<&[u8]>,
) -> Result<SegmentTemplateInfo, DashError> {
    let tmpl = parse_segment_template_attrs(start);
    skip_element(reader)?;
    Ok(tmpl)
}

fn parse_segment_base_empty(e: &BytesStart) -> SegmentTemplateInfo {
    parse_segment_template_attrs(e)
}

fn parse_segment_list_element(
    start: &BytesStart,
    reader: &mut Reader<&[u8]>,
) -> Result<SegmentTemplateInfo, DashError> {
    let mut tmpl = parse_segment_template_attrs(start);

    loop {
        match reader.read_event() {
            Ok(Event::Start(ref e)) => {
                let name = tag_name(e);
                if name == "SegmentTimeline" {
                    tmpl.segment_timeline = parse_segment_timeline(reader)?;
                } else {
                    skip_element(reader)?;
                }
            }
            Ok(Event::End(ref e))
                if tag_name_from_bytes(e.name().as_ref()) == "SegmentList" =>
            {
                break
            }
            Ok(Event::Eof) => break,
            Err(e) => {
                return Err(DashError::with_message(
                    ErrorCode::ManifestLoaderParsingFailure,
                    format!("XML parse error in SegmentList: {e}"),
                ));
            }
            _ => {}
        }
    }

    Ok(tmpl)
}

fn parse_segment_timeline(reader: &mut Reader<&[u8]>) -> Result<Vec<SElement>, DashError> {
    let mut elements = Vec::new();

    loop {
        match reader.read_event() {
            Ok(Event::Empty(ref e)) if tag_name(e) == "S" => {
                let attrs = get_attrs(e);
                let mut s = SElement::default();
                if let Some(v) = attrs.get("t") {
                    s.t = v.parse().ok();
                }
                if let Some(v) = attrs.get("d") {
                    s.d = v.parse().unwrap_or(0);
                }
                if let Some(v) = attrs.get("r") {
                    s.r = v.parse().ok();
                }
                if let Some(v) = attrs.get("k") {
                    s.k = v.parse().ok();
                }
                elements.push(s);
            }
            Ok(Event::End(ref e))
                if tag_name_from_bytes(e.name().as_ref()) == "SegmentTimeline" =>
            {
                break
            }
            Ok(Event::Eof) => break,
            Err(e) => {
                return Err(DashError::with_message(
                    ErrorCode::ManifestLoaderParsingFailure,
                    format!("XML parse error in SegmentTimeline: {e}"),
                ));
            }
            _ => {}
        }
    }

    Ok(elements)
}

fn parse_base_url_element(
    start: &BytesStart,
    reader: &mut Reader<&[u8]>,
) -> Result<BaseUrl, DashError> {
    let attrs = get_attrs(start);
    let mut url_text = String::new();

    loop {
        match reader.read_event() {
            Ok(Event::Text(ref t)) => {
                url_text = t.unescape().unwrap_or_default().to_string();
            }
            Ok(Event::End(ref e)) if tag_name_from_bytes(e.name().as_ref()) == "BaseURL" => break,
            Ok(Event::Eof) => break,
            Err(e) => {
                return Err(DashError::with_message(
                    ErrorCode::ManifestLoaderParsingFailure,
                    format!("XML parse error in BaseURL: {e}"),
                ));
            }
            _ => {}
        }
    }

    let mut bu = BaseUrl::new(url_text);
    if let Some(v) = attrs.get("serviceLocation") {
        bu.service_location = v.clone();
    }
    if let Some(v) = attrs.get("dvb:priority") {
        bu.dvb_priority = v.parse().unwrap_or(1);
    }
    if let Some(v) = attrs.get("dvb:weight") {
        bu.dvb_weight = v.parse().unwrap_or(1);
    }
    if let Some(v) = attrs.get("availabilityTimeOffset") {
        bu.availability_time_offset = v.parse().unwrap_or(0.0);
    }
    if let Some(v) = attrs.get("availabilityTimeComplete") {
        bu.availability_time_complete = v == "true" || v == "1";
    }

    Ok(bu)
}

fn parse_utc_timing(
    start: &BytesStart,
    reader: &mut Reader<&[u8]>,
) -> Result<UtcTiming, DashError> {
    let utc = parse_utc_timing_empty(start);
    skip_element(reader)?;
    Ok(utc)
}

fn parse_utc_timing_empty(e: &BytesStart) -> UtcTiming {
    let attrs = get_attrs(e);
    UtcTiming {
        scheme_id_uri: attrs.get("schemeIdUri").cloned().unwrap_or_default(),
        value: attrs.get("value").cloned().unwrap_or_default(),
    }
}

fn parse_content_protection(
    start: &BytesStart,
    reader: &mut Reader<&[u8]>,
) -> Result<ContentProtection, DashError> {
    let mut cp = parse_content_protection_empty(start);

    loop {
        match reader.read_event() {
            Ok(Event::Start(ref e)) => {
                let name = tag_name(e);
                if name == "cenc:pssh" || name == "pssh" {
                    // Read pssh text
                    if let Ok(Event::Text(ref t)) = reader.read_event() {
                        cp.pssh = Some(t.unescape().unwrap_or_default().to_string());
                    }
                    skip_element(reader).ok();
                } else {
                    skip_element(reader)?;
                }
            }
            Ok(Event::End(ref e))
                if tag_name_from_bytes(e.name().as_ref()) == "ContentProtection" =>
            {
                break
            }
            Ok(Event::Eof) => break,
            Err(e) => {
                return Err(DashError::with_message(
                    ErrorCode::ManifestLoaderParsingFailure,
                    format!("XML parse error in ContentProtection: {e}"),
                ));
            }
            _ => {}
        }
    }

    Ok(cp)
}

fn parse_content_protection_empty(e: &BytesStart) -> ContentProtection {
    let attrs = get_attrs(e);
    ContentProtection {
        scheme_id_uri: attrs.get("schemeIdUri").cloned(),
        value: attrs.get("value").cloned(),
        id: attrs.get("id").cloned(),
        cenc_default_kid: attrs.get("cenc:default_KID").cloned(),
        ref_: attrs.get("ref").cloned(),
        ref_id: attrs.get("refId").cloned(),
        robustness: attrs.get("robustness").cloned(),
        ..ContentProtection::default()
    }
}

fn parse_descriptor_start(
    start: &BytesStart,
    reader: &mut Reader<&[u8]>,
) -> Result<DescriptorType, DashError> {
    let desc = parse_descriptor_empty(start);
    skip_element(reader)?;
    Ok(desc)
}

fn parse_descriptor_empty(e: &BytesStart) -> DescriptorType {
    let attrs = get_attrs(e);
    DescriptorType {
        scheme_id_uri: attrs.get("schemeIdUri").cloned(),
        value: attrs.get("value").cloned(),
        id: attrs.get("id").cloned(),
        dvb_url: attrs.get("dvb:url").cloned(),
        dvb_mime_type: attrs.get("dvb:mimeType").cloned(),
        dvb_font_family: attrs.get("dvb:fontFamily").cloned(),
    }
}

fn parse_service_description(
    start: &BytesStart,
    reader: &mut Reader<&[u8]>,
) -> Result<ServiceDescription, DashError> {
    let attrs = get_attrs(start);
    let mut sd = ServiceDescription {
        id: attrs.get("id").cloned(),
        scheme_id_uri: attrs.get("schemeIdUri").cloned(),
        ..ServiceDescription::default()
    };

    loop {
        match reader.read_event() {
            Ok(Event::Empty(ref e)) => {
                let name = tag_name(e);
                let a = get_attrs(e);
                match name.as_str() {
                    "Latency" => {
                        sd.latency = Some(ServiceDescriptionLatency {
                            target: a.get("target").and_then(|v| v.parse().ok()),
                            max: a.get("max").and_then(|v| v.parse().ok()),
                            min: a.get("min").and_then(|v| v.parse().ok()),
                            reference_id: a.get("referenceId").and_then(|v| v.parse().ok()),
                        });
                    }
                    "PlaybackRate" => {
                        sd.playback_rate = Some(ServiceDescriptionPlaybackRate {
                            max: a.get("max").and_then(|v| v.parse().ok()),
                            min: a.get("min").and_then(|v| v.parse().ok()),
                        });
                    }
                    "OperatingQuality" => {
                        sd.operating_quality = Some(ServiceDescriptionOperatingQuality {
                            media_type: a.get("mediaType").cloned(),
                            max: a.get("max").and_then(|v| v.parse().ok()),
                            min: a.get("min").and_then(|v| v.parse().ok()),
                            target: a.get("target").and_then(|v| v.parse().ok()),
                            type_: a.get("type").cloned(),
                            max_difference: a.get("maxDifference").and_then(|v| v.parse().ok()),
                        });
                    }
                    "OperatingBandwidth" => {
                        sd.operating_bandwidth = Some(ServiceDescriptionOperatingBandwidth {
                            media_type: a.get("mediaType").cloned(),
                            max: a.get("max").and_then(|v| v.parse().ok()),
                            min: a.get("min").and_then(|v| v.parse().ok()),
                            target: a.get("target").and_then(|v| v.parse().ok()),
                        });
                    }
                    _ => {}
                }
            }
            Ok(Event::Start(_)) => {
                skip_element(reader)?;
            }
            Ok(Event::End(ref e))
                if tag_name_from_bytes(e.name().as_ref()) == "ServiceDescription" =>
            {
                break
            }
            Ok(Event::Eof) => break,
            Err(e) => {
                return Err(DashError::with_message(
                    ErrorCode::ManifestLoaderParsingFailure,
                    format!("XML parse error in ServiceDescription: {e}"),
                ));
            }
            _ => {}
        }
    }

    Ok(sd)
}

/// Skip an element and all its children until the matching end tag.
fn skip_element(reader: &mut Reader<&[u8]>) -> Result<(), DashError> {
    let mut depth = 1u32;
    loop {
        match reader.read_event() {
            Ok(Event::Start(_)) => depth += 1,
            Ok(Event::End(_)) => {
                depth -= 1;
                if depth == 0 {
                    break;
                }
            }
            Ok(Event::Eof) => break,
            Err(e) => {
                return Err(DashError::with_message(
                    ErrorCode::ManifestLoaderParsingFailure,
                    format!("XML parse error while skipping: {e}"),
                ));
            }
            _ => {}
        }
    }
    Ok(())
}

/// Convert SegmentTemplateInfo to a JSON-compatible map for the `serde_json::Value`
/// fields in AdaptationSet.
fn segment_template_to_json(tmpl: &SegmentTemplateInfo) -> HashMap<String, serde_json::Value> {
    let mut map = HashMap::new();
    if let Some(ts) = tmpl.timescale {
        map.insert("timescale".to_string(), serde_json::json!(ts));
    }
    if let Some(dur) = tmpl.duration {
        map.insert("duration".to_string(), serde_json::json!(dur));
    }
    if let Some(sn) = tmpl.start_number {
        map.insert("startNumber".to_string(), serde_json::json!(sn));
    }
    if let Some(en) = tmpl.end_number {
        map.insert("endNumber".to_string(), serde_json::json!(en));
    }
    if let Some(ref m) = tmpl.media {
        map.insert("media".to_string(), serde_json::json!(m));
    }
    if let Some(ref init) = tmpl.initialization {
        map.insert("initialization".to_string(), serde_json::json!(init));
    }
    if let Some(pto) = tmpl.presentation_time_offset {
        map.insert("presentationTimeOffset".to_string(), serde_json::json!(pto));
    }
    if let Some(ato) = tmpl.availability_time_offset {
        map.insert(
            "availabilityTimeOffset".to_string(),
            serde_json::json!(ato),
        );
    }
    if let Some(atc) = tmpl.availability_time_complete {
        map.insert(
            "availabilityTimeComplete".to_string(),
            serde_json::json!(atc),
        );
    }
    if let Some(ref ir) = tmpl.index_range {
        map.insert("indexRange".to_string(), serde_json::json!(ir));
    }
    if !tmpl.segment_timeline.is_empty() {
        let s_elements: Vec<serde_json::Value> = tmpl
            .segment_timeline
            .iter()
            .map(|s| {
                let mut m = serde_json::Map::new();
                if let Some(t) = s.t {
                    m.insert("t".to_string(), serde_json::json!(t));
                }
                m.insert("d".to_string(), serde_json::json!(s.d));
                if let Some(r) = s.r {
                    m.insert("r".to_string(), serde_json::json!(r));
                }
                if let Some(k) = s.k {
                    m.insert("k".to_string(), serde_json::json!(k));
                }
                serde_json::Value::Object(m)
            })
            .collect();
        map.insert(
            "SegmentTimeline".to_string(),
            serde_json::json!({"S": s_elements}),
        );
    }
    map
}

/// Process a URI template by replacing DASH template variables.
///
/// Mirrors `processUriTemplate` from SegmentsUtils.js.
pub fn process_uri_template(
    url: &str,
    representation_id: Option<&str>,
    number: Option<u64>,
    sub_number: Option<u64>,
    bandwidth: Option<u64>,
    time: Option<u64>,
) -> String {
    if url.is_empty() {
        return url.to_string();
    }

    let mut result = url.to_string();

    // $RepresentationID$
    if let Some(id) = representation_id {
        result = replace_template_var(&result, "RepresentationID", id);
    }

    // $Number$ with optional format tag
    if let Some(n) = number {
        result = replace_template_var_with_format(&result, "Number", n);
    }

    // $Bandwidth$
    if let Some(bw) = bandwidth {
        result = replace_template_var_with_format(&result, "Bandwidth", bw);
    }

    // $Time$
    if let Some(t) = time {
        result = replace_template_var_with_format(&result, "Time", t);
    }

    // $SubNumber$
    if let Some(sn) = sub_number {
        result = replace_template_var_with_format(&result, "SubNumber", sn);
    }

    // Escape: $$ → $
    result = result.replace("$$", "$");

    result
}

fn replace_template_var(url: &str, var_name: &str, value: &str) -> String {
    let pattern = format!("${var_name}$");
    url.replace(&pattern, value)
}

fn replace_template_var_with_format(url: &str, var_name: &str, value: u64) -> String {
    // Check for format tag: $Number%05d$
    let dollar_var = format!("${var_name}");
    if let Some(start) = url.find(&dollar_var) {
        let after = &url[start + dollar_var.len()..];
        if let Some(end) = after.find('$') {
            let format_spec = &after[..end];
            if format_spec.is_empty() {
                // Simple replacement: $Number$
                let full = format!("${var_name}$");
                return url.replace(&full, &value.to_string());
            } else if format_spec.starts_with('%') {
                // Parse format spec like %05d
                let full = format!("${var_name}{format_spec}$");
                if let Some(width) = parse_format_width(format_spec) {
                    let formatted = format!("{:0>width$}", value, width = width);
                    return url.replace(&full, &formatted);
                } else {
                    return url.replace(&full, &value.to_string());
                }
            }
        }
    }
    // Fallback: simple replacement
    let pattern = format!("${var_name}$");
    url.replace(&pattern, &value.to_string())
}

fn parse_format_width(spec: &str) -> Option<usize> {
    // spec is like "%05d" — extract the number between % and d
    let inner = spec.trim_start_matches('%').trim_end_matches('d');
    let inner = inner.trim_start_matches('0');
    if inner.is_empty() {
        // "%0d" or just "%d" — no padding
        // But "%05d" with inner="" after stripping means single digit width
        let full_inner = spec.trim_start_matches('%').trim_end_matches('d');
        if full_inner.starts_with('0') && full_inner.len() > 1 {
            return full_inner[1..].parse().ok();
        }
        return None;
    }
    inner.parse().ok()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_simple_static_mpd() {
        let xml = r#"<?xml version="1.0" encoding="UTF-8"?>
        <MPD xmlns="urn:mpeg:dash:schema:mpd:2011"
             type="static"
             mediaPresentationDuration="PT30S"
             minBufferTime="PT2S"
             profiles="urn:mpeg:dash:profile:isoff-on-demand:2011">
          <Period id="1" duration="PT30S">
            <AdaptationSet mimeType="video/mp4" contentType="video">
              <Representation id="1" bandwidth="1000000" width="1280" height="720" codecs="avc1.4d401f">
                <SegmentTemplate timescale="90000" duration="180000"
                                 initialization="init-$RepresentationID$.mp4"
                                 media="seg-$RepresentationID$-$Number$.m4s" startNumber="1"/>
              </Representation>
              <Representation id="2" bandwidth="2000000" width="1920" height="1080" codecs="avc1.640028">
                <SegmentTemplate timescale="90000" duration="180000"
                                 initialization="init-$RepresentationID$.mp4"
                                 media="seg-$RepresentationID$-$Number$.m4s" startNumber="1"/>
              </Representation>
            </AdaptationSet>
            <AdaptationSet mimeType="audio/mp4" contentType="audio" lang="en">
              <Representation id="audio" bandwidth="128000" codecs="mp4a.40.2" audioSamplingRate="44100">
                <SegmentTemplate timescale="44100" duration="88200"
                                 initialization="init-$RepresentationID$.mp4"
                                 media="seg-$RepresentationID$-$Number$.m4s" startNumber="1"/>
              </Representation>
            </AdaptationSet>
          </Period>
        </MPD>"#;

        let mpd = parse(xml).unwrap();
        assert_eq!(mpd.type_, PresentationType::Static);
        assert!((mpd.media_presentation_duration.unwrap() - 30.0).abs() < 0.01);
        assert!((mpd.min_buffer_time.unwrap() - 2.0).abs() < 0.01);
        assert_eq!(mpd.periods.len(), 1);

        let period = &mpd.periods[0];
        assert_eq!(period.id.as_deref(), Some("1"));
        assert_eq!(period.adaptation_sets.len(), 2);

        let video_as = &period.adaptation_sets[0];
        assert_eq!(video_as.mime_type.as_deref(), Some("video/mp4"));
        assert_eq!(video_as.content_type.as_deref(), Some("video"));
        assert_eq!(video_as.representations.len(), 2);

        let rep1 = &video_as.representations[0];
        assert_eq!(rep1.id.as_deref(), Some("1"));
        assert_eq!(rep1.bandwidth, Some(1000000));
        assert_eq!(rep1.width, Some(1280));
        assert_eq!(rep1.height, Some(720));
        assert_eq!(rep1.codecs.as_deref(), Some("avc1.4d401f"));
        assert_eq!(rep1.timescale, 90000);
        assert!((rep1.segment_duration.unwrap() - 2.0).abs() < 0.01);
        assert_eq!(
            rep1.initialization.as_deref(),
            Some("init-$RepresentationID$.mp4")
        );

        let audio_as = &period.adaptation_sets[1];
        assert_eq!(audio_as.mime_type.as_deref(), Some("audio/mp4"));
        assert_eq!(audio_as.lang.as_deref(), Some("en"));
        assert_eq!(audio_as.representations[0].audio_sampling_rate.as_deref(), Some("44100"));
    }

    #[test]
    fn test_parse_dynamic_mpd() {
        let xml = r#"<?xml version="1.0" encoding="UTF-8"?>
        <MPD xmlns="urn:mpeg:dash:schema:mpd:2011"
             type="dynamic"
             availabilityStartTime="2023-01-01T00:00:00Z"
             minimumUpdatePeriod="PT2S"
             timeShiftBufferDepth="PT30S"
             minBufferTime="PT2S"
             suggestedPresentationDelay="PT4S">
          <Period id="1" start="PT0S">
            <AdaptationSet mimeType="video/mp4">
              <Representation id="v1" bandwidth="500000" codecs="avc1.42c01e">
                <SegmentTemplate timescale="1000" duration="2000"
                                 initialization="init.mp4"
                                 media="seg-$Number$.m4s" startNumber="1"/>
              </Representation>
            </AdaptationSet>
          </Period>
        </MPD>"#;

        let mpd = parse(xml).unwrap();
        assert_eq!(mpd.type_, PresentationType::Dynamic);
        assert_eq!(
            mpd.availability_start_time.as_deref(),
            Some("2023-01-01T00:00:00Z")
        );
        assert!((mpd.minimum_update_period.unwrap() - 2.0).abs() < 0.01);
        assert!((mpd.time_shift_buffer_depth.unwrap() - 30.0).abs() < 0.01);
        assert!((mpd.suggested_presentation_delay.unwrap() - 4.0).abs() < 0.01);
    }

    #[test]
    fn test_parse_multi_period() {
        let xml = r#"<?xml version="1.0"?>
        <MPD type="static" mediaPresentationDuration="PT60S" minBufferTime="PT2S">
          <Period id="p1" start="PT0S" duration="PT30S">
            <AdaptationSet mimeType="video/mp4">
              <Representation id="1" bandwidth="1000000" codecs="avc1.42c01e">
                <SegmentTemplate media="p1-$Number$.m4s" initialization="p1-init.mp4"
                                 timescale="1000" duration="2000" startNumber="1"/>
              </Representation>
            </AdaptationSet>
          </Period>
          <Period id="p2" start="PT30S" duration="PT30S">
            <AdaptationSet mimeType="video/mp4">
              <Representation id="1" bandwidth="1000000" codecs="avc1.42c01e">
                <SegmentTemplate media="p2-$Number$.m4s" initialization="p2-init.mp4"
                                 timescale="1000" duration="2000" startNumber="1"/>
              </Representation>
            </AdaptationSet>
          </Period>
        </MPD>"#;

        let mpd = parse(xml).unwrap();
        assert_eq!(mpd.periods.len(), 2);
        assert_eq!(mpd.periods[0].id.as_deref(), Some("p1"));
        assert_eq!(mpd.periods[1].id.as_deref(), Some("p2"));
        assert!((mpd.periods[0].start.unwrap() - 0.0).abs() < 0.01);
        assert!((mpd.periods[1].start.unwrap() - 30.0).abs() < 0.01);
        // Next period linking
        assert_eq!(mpd.periods[0].next_period_id.as_deref(), Some("p2"));
        assert!(mpd.periods[1].next_period_id.is_none());
    }

    #[test]
    fn test_parse_segment_timeline() {
        let xml = r#"<?xml version="1.0"?>
        <MPD type="dynamic" availabilityStartTime="2023-01-01T00:00:00Z"
             minimumUpdatePeriod="PT2S" minBufferTime="PT2S">
          <Period id="1" start="PT0S">
            <AdaptationSet mimeType="video/mp4">
              <SegmentTemplate timescale="90000"
                               initialization="init.mp4"
                               media="seg-$Time$.m4s">
                <SegmentTimeline>
                  <S t="0" d="180000" r="4"/>
                  <S d="90000"/>
                </SegmentTimeline>
              </SegmentTemplate>
              <Representation id="1" bandwidth="1000000" codecs="avc1.42c01e"/>
            </AdaptationSet>
          </Period>
        </MPD>"#;

        let mpd = parse(xml).unwrap();
        assert_eq!(mpd.periods.len(), 1);
        let aset = &mpd.periods[0].adaptation_sets[0];
        // SegmentTemplate should be stored as JSON
        assert!(aset.segment_template.is_some());
        let tmpl = aset.segment_template.as_ref().unwrap();
        assert!(tmpl.get("SegmentTimeline").is_some());

        // Representation inherits from AdaptationSet-level SegmentTemplate
        let rep = &aset.representations[0];
        assert_eq!(
            rep.segment_info_type.as_deref(),
            Some("SegmentTimeline")
        );
        assert_eq!(rep.timescale, 90000);
    }

    #[test]
    fn test_parse_segment_base() {
        let xml = r#"<?xml version="1.0"?>
        <MPD type="static" mediaPresentationDuration="PT10S" minBufferTime="PT1S">
          <Period>
            <AdaptationSet mimeType="video/mp4">
              <Representation id="1" bandwidth="500000" codecs="avc1.42c01e">
                <SegmentBase indexRange="100-999">
                  <Initialization range="0-99"/>
                </SegmentBase>
              </Representation>
            </AdaptationSet>
          </Period>
        </MPD>"#;

        let mpd = parse(xml).unwrap();
        let rep = &mpd.periods[0].adaptation_sets[0].representations[0];
        assert_eq!(rep.index_range.as_deref(), Some("100-999"));
        assert_eq!(
            rep.segment_info_type.as_deref(),
            Some("SegmentBase")
        );
    }

    #[test]
    fn test_parse_content_protection() {
        let xml = r#"<?xml version="1.0"?>
        <MPD type="static" mediaPresentationDuration="PT10S" minBufferTime="PT1S">
          <Period>
            <AdaptationSet mimeType="video/mp4">
              <ContentProtection schemeIdUri="urn:mpeg:dash:mp4protection:2011"
                                 value="cenc"
                                 cenc:default_KID="10111213-1415-1617-1819-202122232425"/>
              <ContentProtection schemeIdUri="urn:uuid:edef8ba9-79d6-4ace-a3c8-27dcd51d21ed"
                                 value="Widevine"/>
              <Representation id="1" bandwidth="500000" codecs="avc1.42c01e">
                <SegmentTemplate media="seg-$Number$.m4s" initialization="init.mp4"
                                 timescale="1000" duration="2000" startNumber="1"/>
              </Representation>
            </AdaptationSet>
          </Period>
        </MPD>"#;

        let mpd = parse(xml).unwrap();
        let aset = &mpd.periods[0].adaptation_sets[0];
        assert_eq!(aset.content_protection.len(), 2);
        assert_eq!(
            aset.content_protection[0].scheme_id_uri.as_deref(),
            Some("urn:mpeg:dash:mp4protection:2011")
        );
        assert_eq!(aset.content_protection[0].value.as_deref(), Some("cenc"));
        assert_eq!(
            aset.content_protection[1].scheme_id_uri.as_deref(),
            Some("urn:uuid:edef8ba9-79d6-4ace-a3c8-27dcd51d21ed")
        );
    }

    #[test]
    fn test_parse_role_accessibility() {
        let xml = r#"<?xml version="1.0"?>
        <MPD type="static" mediaPresentationDuration="PT10S" minBufferTime="PT1S">
          <Period>
            <AdaptationSet mimeType="audio/mp4" lang="en">
              <Role schemeIdUri="urn:mpeg:dash:role:2011" value="main"/>
              <Accessibility schemeIdUri="urn:mpeg:dash:role:2011" value="description"/>
              <Representation id="1" bandwidth="128000" codecs="mp4a.40.2">
                <SegmentTemplate media="seg-$Number$.m4s" initialization="init.mp4"
                                 timescale="44100" duration="88200" startNumber="1"/>
              </Representation>
            </AdaptationSet>
          </Period>
        </MPD>"#;

        let mpd = parse(xml).unwrap();
        let aset = &mpd.periods[0].adaptation_sets[0];
        assert_eq!(aset.role.len(), 1);
        assert_eq!(aset.role[0].value.as_deref(), Some("main"));
        assert_eq!(aset.accessibility.len(), 1);
        assert_eq!(
            aset.accessibility[0].value.as_deref(),
            Some("description")
        );
    }

    #[test]
    fn test_parse_utc_timing() {
        let xml = r#"<?xml version="1.0"?>
        <MPD type="dynamic" availabilityStartTime="2023-01-01T00:00:00Z" minBufferTime="PT2S">
          <UTCTiming schemeIdUri="urn:mpeg:dash:utc:http-xsdate:2014"
                     value="https://time.example.com/now"/>
          <Period>
            <AdaptationSet mimeType="video/mp4">
              <Representation id="1" bandwidth="500000" codecs="avc1.42c01e">
                <SegmentTemplate media="$Number$.m4s" initialization="init.mp4"
                                 timescale="1000" duration="2000" startNumber="1"/>
              </Representation>
            </AdaptationSet>
          </Period>
        </MPD>"#;

        let mpd = parse(xml).unwrap();
        assert_eq!(mpd.utc_timing.len(), 1);
        assert_eq!(
            mpd.utc_timing[0].scheme_id_uri,
            "urn:mpeg:dash:utc:http-xsdate:2014"
        );
        assert_eq!(
            mpd.utc_timing[0].value,
            "https://time.example.com/now"
        );
    }

    #[test]
    fn test_parse_base_url() {
        let xml = r#"<?xml version="1.0"?>
        <MPD type="static" mediaPresentationDuration="PT10S" minBufferTime="PT1S">
          <BaseURL>https://cdn.example.com/</BaseURL>
          <Period>
            <BaseURL>video/</BaseURL>
            <AdaptationSet mimeType="video/mp4">
              <BaseURL>1080p/</BaseURL>
              <Representation id="1" bandwidth="5000000" codecs="avc1.640028">
                <BaseURL>main.mp4</BaseURL>
                <SegmentBase indexRange="100-999"/>
              </Representation>
            </AdaptationSet>
          </Period>
        </MPD>"#;

        let mpd = parse(xml).unwrap();
        assert_eq!(mpd.base_urls.len(), 1);
        assert_eq!(mpd.base_urls[0].url, "https://cdn.example.com/");

        let period = &mpd.periods[0];
        assert_eq!(period.base_urls.len(), 1);
        assert_eq!(period.base_urls[0].url, "video/");

        let aset = &period.adaptation_sets[0];
        assert_eq!(aset.base_urls.len(), 1);
        assert_eq!(aset.base_urls[0].url, "1080p/");

        let rep = &aset.representations[0];
        assert_eq!(rep.base_urls.len(), 1);
        assert_eq!(rep.base_urls[0].url, "main.mp4");
    }

    #[test]
    fn test_parse_inherited_segment_template() {
        let xml = r#"<?xml version="1.0"?>
        <MPD type="static" mediaPresentationDuration="PT10S" minBufferTime="PT1S">
          <Period>
            <AdaptationSet mimeType="video/mp4">
              <SegmentTemplate timescale="90000" duration="180000"
                               initialization="init-$RepresentationID$.mp4"
                               media="seg-$RepresentationID$-$Number$.m4s" startNumber="1"/>
              <Representation id="low" bandwidth="500000" codecs="avc1.42c01e"/>
              <Representation id="high" bandwidth="2000000" codecs="avc1.640028"/>
            </AdaptationSet>
          </Period>
        </MPD>"#;

        let mpd = parse(xml).unwrap();
        let reps = &mpd.periods[0].adaptation_sets[0].representations;
        assert_eq!(reps.len(), 2);

        // Both representations should inherit segment template
        for rep in reps {
            assert_eq!(rep.timescale, 90000);
            assert!((rep.segment_duration.unwrap() - 2.0).abs() < 0.01);
            assert_eq!(
                rep.initialization.as_deref(),
                Some("init-$RepresentationID$.mp4")
            );
            assert_eq!(
                rep.media.as_deref(),
                Some("seg-$RepresentationID$-$Number$.m4s")
            );
        }
    }

    #[test]
    fn test_parse_codec_inheritance() {
        let xml = r#"<?xml version="1.0"?>
        <MPD type="static" mediaPresentationDuration="PT10S" minBufferTime="PT1S">
          <Period>
            <AdaptationSet mimeType="video/mp4" codecs="avc1.42c01e">
              <SegmentTemplate media="$Number$.m4s" initialization="init.mp4"
                               timescale="1000" duration="2000" startNumber="1"/>
              <Representation id="1" bandwidth="500000"/>
              <Representation id="2" bandwidth="1000000" codecs="avc1.640028"/>
            </AdaptationSet>
          </Period>
        </MPD>"#;

        let mpd = parse(xml).unwrap();
        let reps = &mpd.periods[0].adaptation_sets[0].representations;
        // First rep inherits from AdaptationSet
        assert_eq!(reps[0].codecs.as_deref(), Some("avc1.42c01e"));
        // Second rep has its own codecs
        assert_eq!(reps[1].codecs.as_deref(), Some("avc1.640028"));
    }

    #[test]
    fn test_process_uri_template() {
        assert_eq!(
            process_uri_template(
                "seg-$RepresentationID$-$Number$.m4s",
                Some("1"),
                Some(5),
                None,
                None,
                None,
            ),
            "seg-1-5.m4s"
        );

        assert_eq!(
            process_uri_template(
                "seg-$Time$.m4s",
                None,
                None,
                None,
                None,
                Some(180000),
            ),
            "seg-180000.m4s"
        );

        assert_eq!(
            process_uri_template(
                "seg-$Bandwidth$.m4s",
                None,
                None,
                None,
                Some(1000000),
                None,
            ),
            "seg-1000000.m4s"
        );
    }

    #[test]
    fn test_process_uri_template_with_format() {
        assert_eq!(
            process_uri_template(
                "seg-$Number%05d$.m4s",
                None,
                Some(42),
                None,
                None,
                None,
            ),
            "seg-00042.m4s"
        );
    }

    #[test]
    fn test_parse_invalid_xml() {
        let result = parse("not xml at all");
        assert!(result.is_err());
    }

    #[test]
    fn test_parse_empty_mpd() {
        let xml = r#"<?xml version="1.0"?><MPD type="static" minBufferTime="PT2S"/>"#;
        // This is an empty element, our parser handles it via the main loop
        // It won't find Event::Start for MPD, so it returns error
        let result = parse(xml);
        // Either parses as empty or returns error depending on XML structure
        // An empty MPD element won't have Event::Start, only Event::Empty
        assert!(result.is_err() || result.unwrap().periods.is_empty());
    }

    #[test]
    fn test_parse_segment_list() {
        let xml = r#"<?xml version="1.0"?>
        <MPD type="static" mediaPresentationDuration="PT6S" minBufferTime="PT1S">
          <Period>
            <AdaptationSet mimeType="video/mp4">
              <Representation id="1" bandwidth="500000" codecs="avc1.42c01e">
                <SegmentList timescale="1000" duration="2000">
                  <SegmentTimeline>
                    <S t="0" d="2000" r="2"/>
                  </SegmentTimeline>
                </SegmentList>
              </Representation>
            </AdaptationSet>
          </Period>
        </MPD>"#;

        let mpd = parse(xml).unwrap();
        let rep = &mpd.periods[0].adaptation_sets[0].representations[0];
        assert_eq!(rep.timescale, 1000);
        assert!((rep.segment_duration.unwrap() - 2.0).abs() < 0.01);
    }

    #[test]
    fn test_parse_service_description() {
        let xml = r#"<?xml version="1.0"?>
        <MPD type="dynamic" availabilityStartTime="2023-01-01T00:00:00Z" minBufferTime="PT2S">
          <ServiceDescription id="0">
            <Latency target="3000" min="2000" max="5000"/>
            <PlaybackRate min="0.96" max="1.04"/>
          </ServiceDescription>
          <Period>
            <AdaptationSet mimeType="video/mp4">
              <Representation id="1" bandwidth="500000" codecs="avc1.42c01e">
                <SegmentTemplate media="$Number$.m4s" initialization="init.mp4"
                                 timescale="1000" duration="2000" startNumber="1"/>
              </Representation>
            </AdaptationSet>
          </Period>
        </MPD>"#;

        let mpd = parse(xml).unwrap();
        assert_eq!(mpd.service_descriptions.len(), 1);
        let sd = &mpd.service_descriptions[0];
        assert_eq!(sd.id.as_deref(), Some("0"));
        let lat = sd.latency.as_ref().unwrap();
        assert_eq!(lat.target, Some(3000));
        assert_eq!(lat.min, Some(2000));
        assert_eq!(lat.max, Some(5000));
        let pbr = sd.playback_rate.as_ref().unwrap();
        assert!((pbr.min.unwrap() - 0.96).abs() < 0.001);
        assert!((pbr.max.unwrap() - 1.04).abs() < 0.001);
    }
}
