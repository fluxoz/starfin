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
/// - `✓` (green)  — deep analysis complete, best-quality thumbnail ready
/// - `⟳` (amber)  — quick thumbnail done, deep pass still pending
/// - `○` (grey)   — no thumbnail generated yet (awaiting processing)
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
            >{ "✓" }</span>
        },
        Some("processing") => html! {
            <span
                class="processing-status processing-status--processing"
                title="Processing"
                aria-label="Processing"
            >{ "⟳" }</span>
        },
        Some("pending") => html! {
            <span
                class="processing-status processing-status--pending"
                title="Awaiting processing"
                aria-label="Awaiting processing"
            >{ "○" }</span>
        },
        _ => html! {},
    }
}
