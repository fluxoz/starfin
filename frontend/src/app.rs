use crate::api;
use crate::components;

use components::{
    filters::FiltersBar,
    grid::ElementsGrid,
    password_modal::PasswordModal,
    video_player::VideoPlayer,
};
use crate::models::{Element, SortBy};

use futures::StreamExt;
use gloo_net::websocket::{futures::WebSocket, Message};
use gloo_timers::callback::Interval;
use wasm_bindgen::prelude::*;
use wasm_bindgen_futures::spawn_local;
use yew::prelude::*;

/// Tracks the authentication state of the application.
#[derive(Clone, PartialEq)]
enum AuthState {
    /// Still loading the auth status from the server.
    Loading,
    /// Password protection is not enabled — proceed normally.
    Disabled,
    /// Password protection is on but no password has been set yet.
    NeedsSetup,
    /// Password protection is on and the user needs to enter it.
    Locked,
    /// The user has been authenticated.
    Authenticated,
}

#[function_component(App)]
pub fn app() -> Html {
    let auth_state = use_state(|| AuthState::Loading);

    // Check auth status on mount.
    {
        let auth_state = auth_state.clone();
        use_effect_with((), move |_| {
            spawn_local(async move {
                match api::fetch_auth_status().await {
                    Ok(status) => {
                        if !status.password_protection {
                            auth_state.set(AuthState::Disabled);
                        } else if status.authenticated {
                            auth_state.set(AuthState::Authenticated);
                        } else if !status.password_set {
                            auth_state.set(AuthState::NeedsSetup);
                        } else {
                            auth_state.set(AuthState::Locked);
                        }
                    }
                    Err(_) => {
                        // If we can't reach the auth endpoint, assume no protection.
                        auth_state.set(AuthState::Disabled);
                    }
                }
            });
            || ()
        });
    }

    let on_authenticated = {
        let auth_state = auth_state.clone();
        Callback::from(move |_| auth_state.set(AuthState::Authenticated))
    };

    // Show the password modal when locked or needs setup.
    match &*auth_state {
        AuthState::Loading => {
            return html! {
                <div class="pw-backdrop">
                    <div class="pw-modal">
                        <div class="pw-modal__logo">{ "STARFIN" }</div>
                        <div class="pw-modal__subtitle">{ "Loading…" }</div>
                    </div>
                </div>
            };
        }
        AuthState::NeedsSetup => {
            return html! {
                <PasswordModal password_set={false} on_authenticated={on_authenticated} />
            };
        }
        AuthState::Locked => {
            return html! {
                <PasswordModal password_set={true} on_authenticated={on_authenticated} />
            };
        }
        AuthState::Disabled | AuthState::Authenticated => {
            // Continue to render the main app below.
        }
    }

    html! { <AppInner /> }
}

#[function_component(AppInner)]
fn app_inner() -> Html {
    let query = use_state(|| "".to_string());
    let sort_by = use_state(|| SortBy::DateAddedNewest);

    // Mutable refs that always hold the *current* query and sort order so that
    // the auto-refresh interval (which is set up only once on mount) can read
    // the latest values without capturing stale UseStateHandle Rc pointers.
    let query_ref = use_mut_ref(|| "".to_string());
    let sort_by_ref = use_mut_ref(|| SortBy::DateAddedNewest);

    let items = use_state(|| Vec::<Element>::new());
    let loading = use_state(|| false);
    let error = use_state(|| Option::<String>::None);
    let selected = use_state(|| Option::<Element>::None);
    let scanning = use_state(|| false);
    let scan_progress = use_state(|| Option::<(u32, u32)>::None);

    // Thumbnail generation progress state: (current, total, phase)
    let thumb_progress = use_state(|| Option::<(u32, u32, String)>::None);

    // Sprite generation progress state: (current, total)
    let sprite_progress = use_state(|| Option::<(u32, u32)>::None);

    // The video ID currently being processed by the thumbnail worker (from WS).
    let thumb_current_id = use_state(|| Option::<String>::None);

    // The video ID currently being processed by the sprite worker (from WS).
    let sprite_current_id = use_state(|| Option::<String>::None);

    // Monotonically increasing version counter; bumps whenever a background
    // worker's `current_id` changes or a batch transitions active → inactive.
    // Child components use this to know when to re-fetch per-video state
    // (processing badges, thumbnails, sprite availability).
    let processing_version = use_state(|| 0_u32);

    // Dark mode state — default to the system's prefers-color-scheme setting
    let dark_mode = use_state(|| {
        web_sys::window()
            .and_then(|w| w.match_media("(prefers-color-scheme: dark)").ok().flatten())
            .map(|mql| mql.matches())
            .unwrap_or(false)
    });

    // Scroll-to-top button visibility state
    let show_scroll_top = use_state(|| false);

    // Hardware acceleration renderer name
    let hwaccel_label = use_state(|| Option::<String>::None);

    // Fetch the renderer name once on mount.
    {
        let hwaccel_label = hwaccel_label.clone();
        use_effect_with((), move |_| {
            spawn_local(async move {
                if let Ok(info) = api::fetch_hwaccel().await {
                    hwaccel_label.set(Some(info.label));
                }
            });
            || ()
        });
    }

    // Listen for scroll events on the window to toggle the back-to-top button.
    {
        let show_scroll_top = show_scroll_top.clone();
        use_effect_with((), move |_| {
            let window = web_sys::window().expect("no window");

            let cb = Closure::<dyn Fn()>::new(move || {
                if let Some(w) = web_sys::window() {
                    let scroll_y = w.scroll_y().unwrap_or(0.0);
                    show_scroll_top.set(scroll_y > 300.0);
                }
            });

            let func: &js_sys::Function = cb.as_ref().unchecked_ref();
            let _ = window.add_event_listener_with_callback("scroll", func);
            let func_clone = func.clone();

            move || {
                if let Some(w) = web_sys::window() {
                    let _ = w.remove_event_listener_with_callback("scroll", &func_clone);
                }
                drop(cb);
            }
        });
    }

    // Fetch on load and whenever query/sort changes.
    // Also keeps query_ref / sort_by_ref in sync so the auto-refresh interval
    // can read the current filter values.
    {
        let query = query.clone();
        let sort_by = sort_by.clone();
        let query_ref = query_ref.clone();
        let sort_by_ref = sort_by_ref.clone();

        let items = items.clone();
        let loading = loading.clone();
        let error = error.clone();

        use_effect_with(((*query).clone(), (*sort_by).clone()), move |_| {
            let query = (*query).clone();
            let sort_by = (*sort_by).clone();

            // Keep refs current so the interval can read the latest values.
            *query_ref.borrow_mut() = query.clone();
            *sort_by_ref.borrow_mut() = sort_by.clone();

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
        let query_ref = query_ref.clone();
        let sort_by_ref = sort_by_ref.clone();
        let items = items.clone();
        let scanning = scanning.clone();

        use_effect_with((), move |_| {
            let interval = Interval::new(60_000, move || {
                // Skip the auto-refresh if a manual scan is already in progress.
                if *scanning {
                    return;
                }
                // Read the current filter values from the shared refs.
                // These are always up-to-date because the fetch effect above
                // writes to them on every query/sort change.  Using plain
                // UseStateHandle clones here would capture the Rc from mount
                // time and always see the initial empty-string query, which
                // is what caused the visible "reset" every 60 seconds.
                let query = query_ref.borrow().clone();
                let sort_by = *sort_by_ref.borrow();

                // If the user has an active search or a non-default sort,
                // skip the background refresh so their filtered results are
                // not disturbed.
                if !query.is_empty() || sort_by != SortBy::DateAddedNewest {
                    return;
                }

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

    // Connect to /api/progress/ws on mount and keep it open.
    // This streams live thumb + sprite progress updates without polling.
    {
        let thumb_progress = thumb_progress.clone();
        let sprite_progress = sprite_progress.clone();
        let thumb_current_id = thumb_current_id.clone();
        let sprite_current_id = sprite_current_id.clone();
        let processing_version = processing_version.clone();
        use_effect_with((), move |_| {
            spawn_local(async move {
                let ws_url = {
                    let location = web_sys::window()
                        .expect("no window")
                        .location();
                    let protocol = location.protocol().unwrap_or_default();
                    let host = location.host().unwrap_or_default();
                    let ws_proto = if protocol == "https:" { "wss" } else { "ws" };
                    format!("{ws_proto}://{host}/api/progress/ws")
                };
                if let Ok(ws) = WebSocket::open(&ws_url) {
                    let (_, mut read) = ws.split();

                    // Local tracking for detecting transitions across WS frames.
                    let mut prev_thumb_id: Option<String> = None;
                    let mut prev_sprite_id: Option<String> = None;
                    let mut prev_thumb_active = false;
                    let mut prev_sprite_active = false;
                    let mut version: u32 = 0;
                    // Skip the first message to avoid a spurious version bump
                    // when prev_* values transition from their initial defaults.
                    let mut initialized = false;

                    while let Some(Ok(Message::Text(text))) = read.next().await {
                        if let Ok(update) = serde_json::from_str::<api::ProgressUpdate>(&text) {
                            // ── Progress bar state ───────────────────────────
                            if update.thumb.active {
                                thumb_progress.set(Some((
                                    update.thumb.current,
                                    update.thumb.total,
                                    update.thumb.phase.clone(),
                                )));
                            } else {
                                thumb_progress.set(None);
                            }
                            if update.sprite.active {
                                sprite_progress.set(Some((
                                    update.sprite.current,
                                    update.sprite.total,
                                )));
                            } else {
                                sprite_progress.set(None);
                            }

                            // ── Per-video processing IDs ────────────────────
                            let new_thumb_id = update.thumb.current_id.clone();
                            let new_sprite_id = update.sprite.current_id.clone();

                            thumb_current_id.set(new_thumb_id.clone());
                            sprite_current_id.set(new_sprite_id.clone());

                            // ── Bump processing_version on meaningful transitions
                            // Skip the very first WS message to avoid a spurious
                            // bump when prev_* values are their initial defaults.
                            if !initialized {
                                initialized = true;
                            } else {
                                let thumb_id_changed = new_thumb_id != prev_thumb_id;
                                let sprite_id_changed = new_sprite_id != prev_sprite_id;
                                let thumb_went_inactive = prev_thumb_active && !update.thumb.active;
                                let sprite_went_inactive = prev_sprite_active && !update.sprite.active;

                                if thumb_id_changed || sprite_id_changed
                                    || thumb_went_inactive || sprite_went_inactive
                                {
                                    version += 1;
                                    processing_version.set(version);
                                }
                            }

                            prev_thumb_id = new_thumb_id;
                            prev_sprite_id = new_sprite_id;
                            prev_thumb_active = update.thumb.active;
                            prev_sprite_active = update.sprite.active;
                        }
                    }
                    // WebSocket closed (server restart, etc.) — silently stop updating.
                }
            });
            || ()
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

    let on_scroll_top = Callback::from(move |_: MouseEvent| {
        if let Some(window) = web_sys::window() {
            let opts = web_sys::ScrollToOptions::new();
            opts.set_top(0.0);
            opts.set_behavior(web_sys::ScrollBehavior::Smooth);
            window.scroll_to_with_scroll_to_options(&opts);
        }
    });

    let on_random = {
        let items = items.clone();
        let selected = selected.clone();
        Callback::from(move |_: MouseEvent| {
            let list = (*items).clone();
            if list.is_empty() {
                return;
            }
            let idx = (js_sys::Math::random() * list.len() as f64) as usize;
            selected.set(Some(list[idx].clone()));
        })
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
    let (thumb_pct, thumb_label) = match (*thumb_progress).clone() {
        Some((current, total, ref phase)) if total > 0 => {
            let label = if phase == "deep" {
                format!("Deep thumbnails: {} / {}", current, total)
            } else {
                format!("Quick thumbnails: {} / {}", current, total)
            };
            ((current as f64 / total as f64 * 100.0) as u32, label)
        }
        Some(_) => (0, "Generating thumbnails…".to_string()),
        None => (0, String::new()),
    };

    // Compute progress percentage and label for the sprite generation bar.
    let (sprite_pct, sprite_label) = match *sprite_progress {
        Some((current, total)) if total > 0 => (
            (current as f64 / total as f64 * 100.0) as u32,
            format!("Sprites: {} / {}", current, total),
        ),
        Some(_) => (0, "Generating sprites…".to_string()),
        None => (0, String::new()),
    };

    let any_bg_active = thumb_progress.is_some() || sprite_progress.is_some() || *scanning;

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
                        <div class="topbar__left">
                            { "STARFIN MEDIA SERVER" }
                            if let Some(label) = (*hwaccel_label).clone() {
                                <span class="topbar__renderer">{ label }</span>
                            }
                        </div>
                        <div class="topbar__right">
                            <button
                                class="random-btn"
                                onclick={on_random}
                                disabled={(*items).is_empty()}
                                aria-label="Play a random media file"
                            >
                                { "?" }
                            </button>
                            <div class="scan-area">
                                <button
                                    class={if *scanning { "scan-btn scan-btn--scanning" } else { "scan-btn" }}
                                    onclick={on_scan}
                                    disabled={*scanning}
                                    aria-label="Scan for new media"
                                >
                                    { if *scanning { "SCANNING…" } else { "SCAN MEDIA" } }
                                </button>
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

                if any_bg_active {
                    <div class="processes-panel">
                        <div class="processes-panel__title">{ "BACKGROUND PROCESSES" }</div>
                        if *scanning {
                            <div class="process-row">
                                <span class="process-row__label">{ "Media Scan" }</span>
                                <div class="process-row__bar-wrap">
                                    <div class="process-row__bar-fill" style={format!("width: {}%", scan_pct)} />
                                </div>
                                <span class="process-row__count">{ &scan_label }</span>
                            </div>
                        }
                        if thumb_progress.is_some() {
                            <div class="process-row">
                                <span class="process-row__label">{ "Thumbnails" }</span>
                                <div class="process-row__bar-wrap">
                                    <div class="process-row__bar-fill" style={format!("width: {}%", thumb_pct)} />
                                </div>
                                <span class="process-row__count">{ &thumb_label }</span>
                            </div>
                        }
                        if sprite_progress.is_some() {
                            <div class="process-row">
                                <span class="process-row__label">{ "Sprites" }</span>
                                <div class="process-row__bar-wrap">
                                    <div class="process-row__bar-fill" style={format!("width: {}%", sprite_pct)} />
                                </div>
                                <span class="process-row__count">{ &sprite_label }</span>
                            </div>
                        }
                    </div>
                }

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
                    <ElementsGrid
                        items={(*items).clone()}
                        on_watch={on_watch}
                        thumb_current_id={(*thumb_current_id).clone()}
                        sprite_current_id={(*sprite_current_id).clone()}
                        processing_version={*processing_version}
                    />
                }
            </main>
            </div>

                <button
                    class={if *show_scroll_top { "scroll-top-btn scroll-top-btn--visible" } else { "scroll-top-btn" }}
                    onclick={on_scroll_top}
                    aria-label="Scroll to top"
                >
                    { "▲" }
                </button>
        </>
    }
}
