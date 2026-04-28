use gloo_net::http::Request;
use serde::Deserialize;
use wasm_bindgen_futures::spawn_local;
use yew::prelude::*;

#[derive(Clone, Debug, PartialEq, Deserialize)]
struct CacheStatusResponse {
    fully_cached: bool,
}

#[derive(Properties, PartialEq)]
pub struct Props {
    pub video_id: String,
    /// Active cache strategy string (e.g. "on-demand", "balanced", "aggressive").
    /// The badge is only shown when this equals "aggressive".
    pub cache_strategy: String,
    /// Bumps whenever a background worker finishes a video or a batch.
    /// Used to trigger a re-fetch of the cache status.
    #[prop_or_default]
    pub processing_version: u32,
}

/// SVG path data for the nf-md-check-decagram icon (U+F0854), representing a
/// verified / fully-cached badge.  Paired with viewBox="0 0 24 24".
const CHECK_DECAGRAM_PATH: &str = "M23 12l-2.44-2.78.34-3.68-3.61-.82-1.89-3.18L12 3 8.6 1.54 6.71 4.72l-3.61.81.34 3.68L1 12l2.44 2.78-.34 3.69 3.61.82 1.89 3.18L12 21l3.4 1.46 1.89-3.18 3.61-.82-.34-3.68L23 12zm-12 2.83l-3.5-3.5 1.41-1.41L11 13l5.59-5.59 1.41 1.41L11 14.83z";

/// Small badge that appears on a video card in aggressive cache mode once the
/// pre-cache worker has fully transcoded every segment of the video.
///
/// - nf-md-check-decagram SVG path (cyan) — all segments are cached (fully cached)
/// - Renders nothing if not in aggressive mode or not yet fully cached.
///
/// The component fetches `/api/videos/{id}/cache-status` on mount and whenever
/// `processing_version` bumps (indicating a worker just finished a video).
#[function_component(CachedBadge)]
pub fn cached_badge(props: &Props) -> Html {
    // Only active in aggressive mode.
    if props.cache_strategy != "aggressive" {
        return html! {};
    }

    let fully_cached: UseStateHandle<bool> = use_state(|| false);

    {
        let fully_cached = fully_cached.clone();
        let video_id = props.video_id.clone();
        let version = props.processing_version;
        use_effect_with(
            (video_id.clone(), version),
            move |(video_id, _version)| {
                let video_id = video_id.clone();
                let fully_cached = fully_cached.clone();
                spawn_local(async move {
                    let url = format!("/api/videos/{video_id}/cache-status");
                    if let Ok(resp) = Request::get(&url).send().await {
                        if resp.ok() {
                            if let Ok(data) = resp.json::<CacheStatusResponse>().await {
                                fully_cached.set(data.fully_cached);
                            }
                        }
                    }
                });
                || ()
            },
        );
    }

    if *fully_cached {
        html! {
            <svg
                class="cached-badge"
                viewBox="0 0 24 24"
                aria-label="Fully cached"
                role="img"
            >
                <title>{"Fully cached"}</title>
                <path fill="currentColor" d={CHECK_DECAGRAM_PATH} />
            </svg>
        }
    } else {
        html! {}
    }
}
