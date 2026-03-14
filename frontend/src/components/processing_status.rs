use gloo_net::http::Request;
use serde::Deserialize;
use wasm_bindgen_futures::spawn_local;
use yew::prelude::*;

#[derive(Clone, Debug, PartialEq, Deserialize)]
struct ProcessingStatusResponse {
    status: String,
}

#[derive(Properties, PartialEq)]
pub struct Props {
    pub video_id: String,
    /// The video ID the thumbnail worker is currently processing (from WS).
    #[prop_or_default]
    pub thumb_current_id: Option<String>,
    /// The video ID the sprite worker is currently processing (from WS).
    #[prop_or_default]
    pub sprite_current_id: Option<String>,
    /// The video ID the pre-cache worker is currently processing (from WS).
    #[prop_or_default]
    pub precache_current_id: Option<String>,
    /// Bumps whenever a background worker finishes a video or a batch.
    #[prop_or_default]
    pub processing_version: u32,
}

/// SVG path data for the nf-md-sync_circle icon (U+F1378), extracted from the
/// vendored SymbolsNerdFontMono-Regular.woff2. Paired with viewBox="0 -410 2048 2048".
const SYNC_CIRCLE_PATH: &str = "M0 614Q0 335 137.0 99.5Q274 -136 509.5 -273.0Q745 -410 1024.0 -410.0Q1303 -410 1538.5 -273.0Q1774 -136 1911.0 99.5Q2048 335 2048.0 614.0Q2048 893 1911.0 1128.5Q1774 1364 1538.5 1501.0Q1303 1638 1024 1638Q822 1638 632.0 1561.0Q442 1484 298.0 1340.0Q154 1196 77.0 1006.0Q0 816 0 614ZM1394 436Q1433 523 1433 614Q1433 782 1312.5 902.5Q1192 1023 1024 1023V821L702 1128L1024 1436V1229Q1192 1229 1334.0 1147.5Q1476 1066 1557.5 924.0Q1639 782 1639.0 609.0Q1639 436 1543 287ZM409 614Q409 792 505 941L654 792Q615 705 615 614Q615 446 735.5 325.5Q856 205 1024 205V407L1332 100L1024 -208V-1Q856 -1 714.0 80.5Q572 162 490.5 304.0Q409 446 409 614Z";

/// Small badge that shows the thumbnail/processing state for a video card.
///
/// - nf-md-check_circle `\u{f05e0}` (green)   — fully processed: deep thumbnail and sprite both complete
/// - nf-md-sync_circle SVG path (orange)       — a background worker is currently active (spins CCW once/s)
/// - nf-md-circle_double `\u{f0e95}` (grey)   — awaiting processing (no worker currently running)
///
/// The component fetches the authoritative status from the server on mount and
/// whenever `processing_version` changes (indicating a worker just finished a
/// video).  Between re-fetches, the WS-provided `current_id` fields give an
/// immediate "processing" indicator for the video currently being worked on.
#[function_component(ProcessingStatus)]
pub fn processing_status(props: &Props) -> Html {
    let fetched_status: UseStateHandle<Option<String>> = use_state(|| None);

    // Determine if this specific video is actively being processed right now.
    let is_processing =
        props.thumb_current_id.as_deref() == Some(props.video_id.as_str())
        || props.sprite_current_id.as_deref() == Some(props.video_id.as_str())
        || props.precache_current_id.as_deref() == Some(props.video_id.as_str());

    // Fetch the authoritative processing status from the server on mount
    // and whenever processing_version bumps (a video just finished).
    {
        let fetched_status = fetched_status.clone();
        let video_id = props.video_id.clone();
        let version = props.processing_version;
        use_effect_with((video_id, version), move |(video_id, _version)| {
            let video_id = video_id.clone();
            let fetched_status = fetched_status.clone();
            spawn_local(async move {
                let url = format!("/api/videos/{video_id}/processing-status");
                if let Ok(resp) = Request::get(&url).send().await {
                    if resp.ok() {
                        if let Ok(data) = resp.json::<ProcessingStatusResponse>().await {
                            fetched_status.set(Some(data.status));
                        }
                    }
                }
            });
            || ()
        });
    }

    // If the WS says this video is actively being worked on right now,
    // show the "processing" badge immediately regardless of the last fetch.
    let display_status = if is_processing {
        Some("processing")
    } else {
        fetched_status.as_deref()
    };

    match display_status {
        Some("processed") => html! {
            <span
                class="processing-status processing-status--processed"
                title="Fully processed"
                aria-label="Fully processed"
            >{ "\u{F05E0}" }</span>
        },
        Some("processing") => html! {
            <svg
                class="processing-status processing-status--processing"
                viewBox="0 -410 2048 2048"
                aria-label="Processing"
                role="img"
            >
                <title>{"Processing"}</title>
                <path fill="currentColor" d={SYNC_CIRCLE_PATH} />
            </svg>
        },
        Some("pending") => {
            html! {
                <span
                    class="processing-status processing-status--pending"
                    title="Awaiting processing"
                    aria-label="Awaiting processing"
                >{ "\u{f0e95}" }</span>
            }
        },
        _ => html! {},
    }
}
