use crate::api;
use crate::components;

use components::{filters::FiltersBar, grid::ElementsGrid, video_player::VideoPlayer};
use crate::models::{Element, SortBy};

use futures::StreamExt;
use gloo_net::websocket::{futures::WebSocket, Message};
use gloo_timers::callback::Interval;
use wasm_bindgen_futures::spawn_local;
use yew::prelude::*;

/// How often (ms) the frontend polls `/api/thumbnails/progress`.
const THUMB_PROGRESS_POLL_INTERVAL_MS: u32 = 2_000;

#[function_component(App)]
pub fn app() -> Html {
    let query = use_state(|| "".to_string());
    let sort_by = use_state(|| SortBy::DateAddedNewest);

    let items = use_state(|| Vec::<Element>::new());
    let loading = use_state(|| false);
    let error = use_state(|| Option::<String>::None);
    let selected = use_state(|| Option::<Element>::None);
    let scanning = use_state(|| false);
    let scan_progress = use_state(|| Option::<(u32, u32)>::None);

    // Thumbnail generation progress state
    let thumb_progress = use_state(|| Option::<(u32, u32)>::None);

    // Dark mode state
    let dark_mode = use_state(|| false);

    // Fetch on load and whenever query/sort changes
    {
        let query = query.clone();
        let sort_by = sort_by.clone();

        let items = items.clone();
        let loading = loading.clone();
        let error = error.clone();

        use_effect_with(((*query).clone(), (*sort_by).clone()), move |_| {
            let query = (*query).clone();
            let sort_by = (*sort_by).clone();

            loading.set(true);
            error.set(None);

            spawn_local(async move {
                match api::fetch_elements(&query, sort_by).await {
                    Ok(data) => items.set(data),
                    Err(e) => error.set(Some(e)),
                }
                loading.set(false);
            });

            || ()
        });
    }

    // Auto-refresh: re-fetch the video list every 60 seconds.
    {
        let query = query.clone();
        let sort_by = sort_by.clone();
        let items = items.clone();
        let scanning = scanning.clone();

        use_effect_with((), move |_| {
            let interval = Interval::new(60_000, move || {
                // Skip the auto-refresh if a manual scan is already in progress.
                if *scanning {
                    return;
                }
                let query = (*query).clone();
                let sort_by = (*sort_by).clone();
                let items = items.clone();
                spawn_local(async move {
                    if let Ok(data) = api::fetch_elements(&query, sort_by).await {
                        items.set(data);
                    }
                });
            });
            // Keep the interval alive for the lifetime of the component.
            move || drop(interval)
        });
    }

    // Poll for deep thumbnail generation progress every 2 seconds.
    {
        let thumb_progress = thumb_progress.clone();

        use_effect_with((), move |_| {
            let interval = Interval::new(THUMB_PROGRESS_POLL_INTERVAL_MS, move || {
                let thumb_progress = thumb_progress.clone();
                spawn_local(async move {
                    if let Ok(data) = api::fetch_thumb_progress().await {
                        if data.active {
                            thumb_progress.set(Some((data.current, data.total)));
                        } else {
                            thumb_progress.set(None);
                        }
                    }
                });
            });
            move || drop(interval)
        });
    }

    let on_query_change = {
        let query = query.clone();
        Callback::from(move |v: String| query.set(v))
    };

    let on_sort_change = {
        let sort_by = sort_by.clone();
        Callback::from(move |v: SortBy| sort_by.set(v))
    };

    let on_watch = {
        let selected = selected.clone();
        Callback::from(move |item: Element| selected.set(Some(item)))
    };

    let on_close_player = {
        let selected = selected.clone();
        Callback::from(move |_| selected.set(None))
    };

    let on_toggle_dark_mode = {
        let dark_mode = dark_mode.clone();
        Callback::from(move |_| dark_mode.set(!*dark_mode))
    };

    let on_scan = {
        let scanning = scanning.clone();
        let items = items.clone();
        let query = query.clone();
        let sort_by = sort_by.clone();
        let scan_progress = scan_progress.clone();
        Callback::from(move |_: MouseEvent| {
            if *scanning {
                return;
            }
            let scanning = scanning.clone();
            let items = items.clone();
            let query = (*query).clone();
            let sort_by = (*sort_by).clone();
            let scan_progress = scan_progress.clone();

            scanning.set(true);
            scan_progress.set(Some((0, 0)));

            spawn_local(async move {
                // Build the WebSocket URL from the current page's location.
                let ws_url = {
                    let location = web_sys::window()
                        .expect("no window")
                        .location();
                    let protocol = location.protocol().unwrap_or_default();
                    let host = location.host().unwrap_or_default();
                    let ws_proto = if protocol == "https:" { "wss" } else { "ws" };
                    format!("{ws_proto}://{host}/api/scan/ws")
                };

                if let Ok(ws) = WebSocket::open(&ws_url) {
                    // Only read from the server; split() avoids needing the type to be Unpin.
                    let (_, mut read) = ws.split();
                    while let Some(Ok(Message::Text(text))) = read.next().await {
                        match serde_json::from_str::<api::ScanProgressData>(&text) {
                            Ok(p) => scan_progress.set(Some((p.current, p.total))),
                            Err(e) => web_sys::console::warn_1(
                                &format!("scan_ws: unexpected message: {e}").into(),
                            ),
                        }
                    }
                    // WebSocket closed = scan complete.
                }

                // Refresh the media list now that the cache is updated.
                if let Ok(data) = api::fetch_elements(&query, sort_by).await {
                    items.set(data);
                }
                scan_progress.set(None);
                scanning.set(false);
            });
        })
    };

    let app_class = if *dark_mode { "app dark-mode" } else { "app" };

    // Compute progress percentage and label for the scan progress bar.
    let (scan_pct, scan_label) = match *scan_progress {
        Some((current, total)) if total > 0 => (
            (current as f64 / total as f64 * 100.0) as u32,
            format!("{} / {} files", current, total),
        ),
        Some(_) => (0, "Counting files…".to_string()),
        None => (0, String::new()),
    };

    // Compute progress percentage and label for the thumbnail generation bar.
    let (thumb_pct, thumb_label) = match *thumb_progress {
        Some((current, total)) if total > 0 => (
            (current as f64 / total as f64 * 100.0) as u32,
            format!("Thumbnails: {} / {}", current, total),
        ),
        Some(_) => (0, "Generating thumbnails…".to_string()),
        None => (0, String::new()),
    };

    html! {
        <>
            <div class={app_class}>
                // Left sidebar with Starfin branding
                <aside class="sidebar">
                    <div class="sidebar__logo">{ "STARFIN" }</div>
                    <div class="sidebar__text">{ "MEDIA COLLECTION" }</div>
                    <div class="sidebar__arrow">{ "↗" }</div>
                </aside>

                // Video player overlay — rendered on top of the library when a video is selected.
                if let Some(video) = &*selected {
                    <VideoPlayer
                        video_id={video.id.clone()}
                        title={video.title.clone()}
                        on_close={on_close_player}
                    />
                }

                <header class="topbar">
                    <div class="topbar__inner">
                        <div class="topbar__left">{ "STARFIN MEDIA SERVER" }</div>
                        <div class="topbar__right">
                            <div class="scan-area">
                                <button
                                    class={if *scanning { "scan-btn scan-btn--scanning" } else { "scan-btn" }}
                                    onclick={on_scan}
                                    disabled={*scanning}
                                    aria-label="Scan for new media"
                                >
                                    { if *scanning { "SCANNING…" } else { "SCAN MEDIA" } }
                                </button>
                                if *scanning {
                                    <div class="scan-progress" role="progressbar" aria-label="Scan progress" aria-valuenow={scan_pct.to_string()} aria-valuemin="0" aria-valuemax="100">
                                        <div class="scan-progress__track">
                                            <div class="scan-progress__fill" style={format!("width: {}%", scan_pct)} />
                                        </div>
                                        <span class="scan-progress__label">{ &scan_label }</span>
                                    </div>
                                }
                                if thumb_progress.is_some() {
                                    <div class="scan-progress" role="progressbar" aria-label="Thumbnail generation progress" aria-valuenow={thumb_pct.to_string()} aria-valuemin="0" aria-valuemax="100">
                                        <div class="scan-progress__track">
                                            <div class="scan-progress__fill" style={format!("width: {}%", thumb_pct)} />
                                        </div>
                                        <span class="scan-progress__label">{ &thumb_label }</span>
                                    </div>
                                }
                            </div>
                            <button 
                                class="theme-toggle" 
                                onclick={on_toggle_dark_mode.clone()}
                                aria-label={if *dark_mode { "Switch to light mode" } else { "Switch to dark mode" }}
                                aria-pressed={dark_mode.to_string()}
                            >
                                <span class={if *dark_mode { "theme-toggle__switch active" } else { "theme-toggle__switch" }}></span>
                            </button>
                        </div>
                    </div>

                    <FiltersBar
                        query={(*query).clone()}
                        sort_by={(*sort_by).clone()}
                        on_query_change={on_query_change}
                        on_sort_change={on_sort_change}
                    />
                </header>

            <main class="content">
                if let Some(err) = &*error {
                    <div class="notice notice--error">
                        <div class="notice__title">{ "Failed to load" }</div>
                        <div class="notice__body">{ err }</div>
                    </div>
                }

                if *loading {
                    <div class="notice notice--loading">{ "Loading…" }</div>
                } else {
                    <ElementsGrid items={(*items).clone()} on_watch={on_watch} />
                }
            </main>
            </div>
        </>
    }
}
