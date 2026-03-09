use gloo_net::http::Request;
use serde::Deserialize;
use yew::prelude::*;

#[derive(Clone, Debug, PartialEq, Deserialize)]
struct ProcessingStatusResponse {
    status: String,
}

#[derive(Properties, PartialEq)]
pub struct Props {
    pub video_id: String,
}

/// Small badge that shows the thumbnail/processing state for a video card.
///
/// - nf-md-check_circle `\u{f05e0}` (green)   — fully processed: deep thumbnail and sprite both complete
/// - nf-md-sync_circle `\u{f1378}` (orange)   — a background worker is currently active (spins once/s)
/// - nf-md-circle_double `\u{f0e95}` (grey)   — awaiting processing (no worker currently running)
#[function_component(ProcessingStatus)]
pub fn processing_status(props: &Props) -> Html {
    let status: UseStateHandle<Option<String>> = use_state(|| None);

    {
        let status = status.clone();
        let video_id = props.video_id.clone();
        use_effect_with(video_id, move |video_id| {
            let video_id = video_id.clone();
            let status = status.clone();
            wasm_bindgen_futures::spawn_local(async move {
                let url = format!("/api/videos/{video_id}/processing-status");
                if let Ok(resp) = Request::get(&url).send().await {
                    if resp.ok() {
                        if let Ok(data) = resp.json::<ProcessingStatusResponse>().await {
                            status.set(Some(data.status));
                        }
                    }
                }
            });
            || ()
        });
    }

    match status.as_deref() {
        Some("processed") => html! {
            <span
                class="processing-status processing-status--processed"
                title="Fully processed"
                aria-label="Fully processed"
            >{ "\u{F05E0}" }</span>
        },
        Some("processing") => html! {
            <span
                class="processing-status processing-status--processing"
                title="Processing"
                aria-label="Processing"
            >{ "\u{F1378}" }</span>
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
