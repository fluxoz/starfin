use crate::api;
use crate::components;

use components::{
    filters::FiltersBar,
    grid::ElementsGrid,
    media_edit_modal::MediaEditModal,
    password_modal::PasswordModal,
    scroll_view::ScrollView,
    video_player::VideoPlayer,
};
use crate::models::{Element, MetadataFilter, SortBy};

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

    let show_logout = *auth_state == AuthState::Authenticated;

    let on_logout = {
        let auth_state = auth_state.clone();
        Callback::from(move |_: ()| auth_state.set(AuthState::Locked))
    };

    html! { <AppInner show_logout={show_logout} on_logout={on_logout} /> }
}

/// Returns the CSS-grid column count for the given viewport width.
/// Mirrors the breakpoints in `main.css`:
///   < 520 px → 1 column, 520–899 px → 2 columns, ≥ 900 px → 3 columns.
fn col_count_for_width(win_width: f64) -> usize {
    if win_width >= 900.0 { 3 } else if win_width >= 520.0 { 2 } else { 1 }
}

/// Props for the inner application component.
#[derive(Properties, PartialEq)]
struct AppInnerProps {
    /// Whether to show the logout button (password protection is on and user
    /// is authenticated).
    #[prop_or_default]
    pub show_logout: bool,
    /// Called when the user clicks the logout button.
    #[prop_or_default]
    pub on_logout: Callback<()>,
}

#[function_component(AppInner)]
fn app_inner(props: &AppInnerProps) -> Html {
    /// Number of cards kept in the DOM at any one time (the virtual window).
    /// 30 items = 10 rows (3 cols) keeps the DOM very lean: only visible cards
    /// plus a small buffer above/below are ever present.
    const WINDOW_SIZE: usize = 30;
    /// Initial estimate for one card's rendered height plus the grid row-gap (16 px).
    /// Updated after the first real render by measuring an actual `.card` element.
    const CARD_ROW_HEIGHT_ESTIMATE: f64 = 440.0;
    /// Duration (ms) of the content fade-out when returning to the top.
    /// Must match the `transition: opacity` duration in `.content--fading` (main.css).
    const SCROLL_TOP_FADE_MS: u32 = 200;
    /// Extra delay (ms) after the virtual window reset to allow Yew to flush
    /// the DOM update before fading back in, preventing a brief flash of
    /// the wrong content at the top of the page.
    const SCROLL_TOP_SETTLE_MS: u32 = 50;
    let query = use_state(|| "".to_string());
    let sort_by = use_state(|| SortBy::DateAddedNewest);
    let meta_filter = use_state(MetadataFilter::default);

    // Mutable refs that always hold the *current* query and sort order so that
    // the auto-refresh interval (which is set up only once on mount) can read
    // the latest values without capturing stale UseStateHandle Rc pointers.
    let query_ref = use_mut_ref(|| "".to_string());
    let sort_by_ref = use_mut_ref(|| SortBy::DateAddedNewest);
    let meta_filter_ref = use_mut_ref(MetadataFilter::default);

    let items = use_state(|| Vec::<Element>::new());
    /// Unfiltered list of all items returned by the last API fetch.
    /// Used to populate tag/actor/category dropdown options.
    let raw_items = use_state(|| Vec::<Element>::new());
    let loading = use_state(|| false);
    let error = use_state(|| Option::<String>::None);
    let selected = use_state(|| Option::<Element>::None);
    let editing = use_state(|| Option::<Element>::None);
    let scroll_mode = use_state(|| false);
    let scanning = use_state(|| false);
    let scan_progress = use_state(|| Option::<(u32, u32)>::None);

    // Thumbnail generation progress state: (current, total, phase)
    let thumb_progress = use_state(|| Option::<(u32, u32, String)>::None);

    // Sprite generation progress state: (current, total)
    let sprite_progress = use_state(|| Option::<(u32, u32)>::None);

    // Pre-cache progress state: (current, total)
    let precache_progress = use_state(|| Option::<(u32, u32)>::None);

    // The video IDs currently being processed by the thumbnail worker (from WS).
    let thumb_current_id = use_state(Vec::<String>::new);

    // The video IDs currently being processed by the sprite worker (from WS).
    let sprite_current_id = use_state(Vec::<String>::new);

    // The video ID currently being pre-cached (from WS).
    let precache_current_id = use_state(|| Option::<String>::None);

    // localStorage key used to persist the user's theme preference.
    const THEME_STORAGE_KEY: &str = "starfin_theme";

    // Dark mode state — use a saved user preference from localStorage if present,
    // otherwise fall back to the system's prefers-color-scheme setting.
    let dark_mode = use_state(|| {
        let saved = web_sys::window()
            .and_then(|w| w.local_storage().ok())
            .flatten()
            .and_then(|s| s.get_item(THEME_STORAGE_KEY).ok())
            .flatten();
        match saved.as_deref() {
            Some("dark")  => true,
            Some("light") => false,
            _ => web_sys::window()
                .and_then(|w| w.match_media("(prefers-color-scheme: dark)").ok().flatten())
                .map(|mql| mql.matches())
                .unwrap_or(false),
        }
    });

    // Scroll-to-top button visibility state
    let show_scroll_top = use_state(|| false);

    // Tracks whether the scroll-to-top transition is in progress (fade-out
    // phase before the virtual window is reset and the page jumps to the top).
    let returning_to_top = use_state(|| false);

    // Virtual window: index of the first item currently rendered.  Only items
    // [window_start .. window_start + WINDOW_SIZE] are in the DOM; spacers
    // above and below fill the remaining document height so scroll position is
    // preserved as the window slides.
    let window_start = use_state(|| 0_usize);
    // UseRef mirror — readable inside the scroll closure (created once at
    // mount) without stale-capture issues.
    let window_start_ref = use_mut_ref(|| 0_usize);

    // Tracks total items length for the scroll closure.
    let total_items_len_ref = use_mut_ref(|| 0_usize);

    // Measured height (px) of one rendered card row (card height + 16 px gap).
    // Starts as an estimate; updated once the first card is in the DOM.
    let card_row_height_ref = use_mut_ref(|| CARD_ROW_HEIGHT_ESTIMATE);

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

    // Keep window_start_ref in sync with window_start so the scroll closure
    // (created once at mount) can always read the latest value.
    {
        let window_start_ref = window_start_ref.clone();
        use_effect_with(*window_start, move |&start| {
            *window_start_ref.borrow_mut() = start;
            || ()
        });
    }

    // Keep total_items_len_ref in sync with the items list length.
    {
        let total_items_len_ref = total_items_len_ref.clone();
        use_effect_with((*items).len(), move |&len| {
            *total_items_len_ref.borrow_mut() = len;
            || ()
        });
    }

    // Measure the actual card row height once items are first rendered so the
    // spacer calculations are accurate.
    {
        let card_row_height_ref = card_row_height_ref.clone();
        use_effect_with((*items).len() > 0, move |&has_items| {
            if has_items {
                if let Some(doc) = web_sys::window().and_then(|w| w.document()) {
                    if let Some(card) = doc.query_selector(".card").ok().flatten() {
                        let h = card.get_bounding_client_rect().height();
                        if h > 0.0 {
                            // 16 px is the grid row-gap from main.css
                            *card_row_height_ref.borrow_mut() = h + 16.0;
                        }
                    }
                }
            }
            || ()
        });
    }

    // Listen for scroll events: toggle the back-to-top button and slide the
    // virtual window so that only WINDOW_SIZE cards are in the DOM at a time.
    {
        let show_scroll_top = show_scroll_top.clone();
        let window_start = window_start.clone();
        let window_start_ref = window_start_ref.clone();
        let total_items_len_ref = total_items_len_ref.clone();
        let card_row_height_ref = card_row_height_ref.clone();
        let query_ref = query_ref.clone();
        use_effect_with((), move |_| {
            let window = web_sys::window().expect("no window");

            let cb = Closure::<dyn Fn()>::new(move || {
                if let Some(w) = web_sys::window() {
                    let scroll_y = w.scroll_y().unwrap_or(0.0);
                    show_scroll_top.set(scroll_y > 300.0);

                    // Virtual windowing: only active when no search filter is
                    // in use.  `query_ref` (a UseRef) is read here because
                    // UseStateHandle captures would be stale in this closure.
                    if query_ref.borrow().is_empty() {
                        let total = *total_items_len_ref.borrow();
                        if total > WINDOW_SIZE {
                            let row_height = *card_row_height_ref.borrow();
                            let win_width = w
                                .inner_width()
                                .ok()
                                .and_then(|v| v.as_f64())
                                .unwrap_or(1280.0);
                            let col_count = col_count_for_width(win_width);

                            // How many pixels of the rendered grid section are
                            // above the top of the viewport.
                            let scrolled_past_grid = w
                                .document()
                                .and_then(|d| d.query_selector(".grid").ok().flatten())
                                .map(|el| (-el.get_bounding_client_rect().top()).max(0.0))
                                .unwrap_or(0.0);

                            // Rows of rendered cards above the viewport.
                            let rows_above =
                                (scrolled_past_grid / row_height).floor() as usize;
                            // Corresponding absolute item index.
                            let current_start = *window_start_ref.borrow();
                            let items_above = rows_above * col_count;
                            let absolute_top = current_start + items_above;

                            // Keep 1 row of look-behind above the viewport.
                            let new_start = absolute_top
                                .saturating_sub(col_count)
                                // Align to a row boundary for clean window edges.
                                / col_count * col_count;
                            let new_start =
                                new_start.min(total.saturating_sub(WINDOW_SIZE));

                            if new_start != current_start {
                                *window_start_ref.borrow_mut() = new_start;
                                window_start.set(new_start);
                            }
                        }
                    }
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

    // Fetch on load and whenever query/sort/meta_filter changes.
    // Also keeps query_ref / sort_by_ref / meta_filter_ref in sync so the
    // auto-refresh interval (which is set up only once on mount) can read
    // the latest values without capturing stale UseStateHandle Rc pointers.
    {
        let query = query.clone();
        let sort_by = sort_by.clone();
        let meta_filter = meta_filter.clone();
        let query_ref = query_ref.clone();
        let sort_by_ref = sort_by_ref.clone();
        let meta_filter_ref = meta_filter_ref.clone();

        let items = items.clone();
        let raw_items = raw_items.clone();
        let loading = loading.clone();
        let error = error.clone();
        let window_start = window_start.clone();
        let window_start_ref = window_start_ref.clone();

        use_effect_with(
            ((*query).clone(), (*sort_by).clone(), (*meta_filter).clone()),
            move |_| {
                let query = (*query).clone();
                let sort_by = (*sort_by).clone();
                let mf = (*meta_filter).clone();

                // Keep refs current so the interval can read the latest values.
                *query_ref.borrow_mut() = query.clone();
                *sort_by_ref.borrow_mut() = sort_by.clone();
                *meta_filter_ref.borrow_mut() = mf.clone();

                // Reset the virtual window to the top on every filter/sort change.
                window_start.set(0);
                *window_start_ref.borrow_mut() = 0;

                loading.set(true);
                error.set(None);

                spawn_local(async move {
                    match api::fetch_all_videos().await {
                        Ok(all) => {
                            let filtered = api::apply_filters(&all, &query, sort_by, &mf);
                            raw_items.set(all);
                            items.set(filtered);
                        }
                        Err(e) => error.set(Some(e)),
                    }
                    loading.set(false);
                });

                || ()
            },
        );
    }

    // Auto-refresh: re-fetch the video list every 60 seconds.
    {
        let query_ref = query_ref.clone();
        let sort_by_ref = sort_by_ref.clone();
        let meta_filter_ref = meta_filter_ref.clone();
        let items = items.clone();
        let raw_items = raw_items.clone();
        let scanning = scanning.clone();

        use_effect_with((), move |_| {
            let interval = Interval::new(60_000, move || {
                // Skip the auto-refresh if a manual scan is already in progress.
                if *scanning {
                    return;
                }
                // Read the current filter values from the shared refs.
                // These are always up-to-date because the fetch effect above
                // writes to them on every query/sort/meta_filter change.  Using
                // plain UseStateHandle clones here would capture the Rc from
                // mount time and always see the initial empty-string query.
                let query = query_ref.borrow().clone();
                let sort_by = *sort_by_ref.borrow();
                let mf = meta_filter_ref.borrow().clone();

                // If the user has an active search, sort, or metadata filter,
                // skip the background refresh so their filtered results are
                // not disturbed.
                if !query.is_empty() || sort_by != SortBy::DateAddedNewest || mf.is_active() {
                    return;
                }

                let items = items.clone();
                let raw_items = raw_items.clone();
                spawn_local(async move {
                    if let Ok(all) = api::fetch_all_videos().await {
                        let filtered = api::apply_filters(&all, &query, sort_by, &mf);
                        raw_items.set(all);
                        items.set(filtered);
                    }
                });
            });
            // Keep the interval alive for the lifetime of the component.
            move || drop(interval)
        });
    }

    // Connect to /api/progress/ws on mount and keep it open.
    // This streams live thumb + sprite + precache progress updates without polling.
    {
        let thumb_progress = thumb_progress.clone();
        let sprite_progress = sprite_progress.clone();
        let precache_progress = precache_progress.clone();
        let thumb_current_id = thumb_current_id.clone();
        let sprite_current_id = sprite_current_id.clone();
        let precache_current_id = precache_current_id.clone();
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
                            if update.precache.active {
                                precache_progress.set(Some((
                                    update.precache.current,
                                    update.precache.total,
                                )));
                            } else {
                                precache_progress.set(None);
                            }

                            // ── Per-video processing IDs ────────────────────
                            thumb_current_id.set(update.thumb.current_ids.clone());
                            sprite_current_id.set(update.sprite.current_ids.clone());
                            precache_current_id.set(update.precache.current_id.clone());
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

    let on_filter_change = {
        let meta_filter = meta_filter.clone();
        Callback::from(move |v: MetadataFilter| meta_filter.set(v))
    };

    let on_watch = {
        let selected = selected.clone();
        Callback::from(move |item: Element| selected.set(Some(item)))
    };

    let on_close_player = {
        let selected = selected.clone();
        Callback::from(move |_| selected.set(None))
    };

    let on_edit = {
        let editing = editing.clone();
        Callback::from(move |item: Element| editing.set(Some(item)))
    };

    let on_close_edit = {
        let editing = editing.clone();
        Callback::from(move |_| editing.set(None))
    };

    let on_metadata_saved = {
        let items = items.clone();
        let editing = editing.clone();
        Callback::from(move |updated: Element| {
            let mut list = (*items).clone();
            if let Some(pos) = list.iter().position(|e| e.id == updated.id) {
                list[pos] = updated;
            }
            items.set(list);
            editing.set(None);
        })
    };

    let on_favorite_toggle = {
        let items = items.clone();
        Callback::from(move |item: Element| {
            let new_fav = !item.favorite;
            let vid = item.id.clone();
            let items = items.clone();

            // Optimistic update: flip immediately so the UI feels instant.
            {
                let mut list = (*items).clone();
                if let Some(pos) = list.iter().position(|e| e.id == vid) {
                    list[pos].favorite = new_fav;
                }
                items.set(list);
            }

            spawn_local(async move {
                match api::update_metadata(
                    &vid,
                    Some(new_fav),
                    None,
                    None,
                    None,
                    None,
                ).await {
                    Ok(updated) => {
                        // Reconcile with the server response.
                        let mut list = (*items).clone();
                        if let Some(pos) = list.iter().position(|e| e.id == updated.id) {
                            list[pos] = updated;
                        }
                        items.set(list);
                    }
                    Err(_) => {
                        // Roll back the optimistic update on error.
                        let mut list = (*items).clone();
                        if let Some(pos) = list.iter().position(|e| e.id == vid) {
                            list[pos].favorite = !new_fav;
                        }
                        items.set(list);
                    }
                }
            });
        })
    };

    let on_toggle_dark_mode = {
        let dark_mode = dark_mode.clone();
        Callback::from(move |_| {
            let new_value = !*dark_mode;
            // Persist the user's explicit choice to localStorage.
            if let Some(storage) = web_sys::window()
                .and_then(|w| w.local_storage().ok())
                .flatten()
            {
                let _ = storage.set_item(THEME_STORAGE_KEY, if new_value { "dark" } else { "light" });
            }
            dark_mode.set(new_value);
        })
    };

    let on_scroll_top = {
        let window_start = window_start.clone();
        let window_start_ref = window_start_ref.clone();
        let returning_to_top = returning_to_top.clone();
        Callback::from(move |_: MouseEvent| {
            if *returning_to_top {
                return;
            }
            let window_start = window_start.clone();
            let window_start_ref = window_start_ref.clone();
            let returning_to_top = returning_to_top.clone();
            // Fade out the content first, then jump to the top so the virtual
            // window reset happens while the page is invisible (no tearing).
            returning_to_top.set(true);
            spawn_local(async move {
                // Wait for the CSS fade-out to complete (matches the
                // transition duration in .content--fading).
                gloo_timers::future::TimeoutFuture::new(SCROLL_TOP_FADE_MS).await;
                // Reset the virtual window to the first WINDOW_SIZE cards.
                window_start.set(0);
                *window_start_ref.borrow_mut() = 0;
                // Jump instantly to the top — content is already invisible.
                if let Some(window) = web_sys::window() {
                    let opts = web_sys::ScrollToOptions::new();
                    opts.set_top(0.0);
                    opts.set_behavior(web_sys::ScrollBehavior::Auto);
                    window.scroll_to_with_scroll_to_options(&opts);
                }
                // Allow Yew to apply the window reset before fading back in.
                gloo_timers::future::TimeoutFuture::new(SCROLL_TOP_SETTLE_MS).await;
                returning_to_top.set(false);
            });
        })
    };

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

    let on_scroll_mode = {
        let scroll_mode = scroll_mode.clone();
        Callback::from(move |_: MouseEvent| {
            scroll_mode.set(true);
        })
    };

    let on_close_scroll = {
        let scroll_mode = scroll_mode.clone();
        Callback::from(move |_| {
            scroll_mode.set(false);
        })
    };

    let on_logout = {
        let on_logout_prop = props.on_logout.clone();
        Callback::from(move |_: MouseEvent| {
            let cb = on_logout_prop.clone();
            spawn_local(async move {
                let _ = api::logout().await;
                cb.emit(());
            });
        })
    };

    let on_scan = {
        let scanning = scanning.clone();
        let items = items.clone();
        let raw_items = raw_items.clone();
        let query = query.clone();
        let sort_by = sort_by.clone();
        let meta_filter = meta_filter.clone();
        let scan_progress = scan_progress.clone();
        Callback::from(move |_: MouseEvent| {
            if *scanning {
                return;
            }
            let scanning = scanning.clone();
            let items = items.clone();
            let raw_items = raw_items.clone();
            let query = (*query).clone();
            let sort_by = (*sort_by).clone();
            let mf = (*meta_filter).clone();
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
                    let (_, mut read) = ws.split();

                    // Start from the full unfiltered list so that items
                    // hidden by active filters are preserved.
                    let mut accumulated: Vec<crate::models::Element> = (*raw_items).clone();

                    while let Some(Ok(Message::Text(text))) = read.next().await {
                        match serde_json::from_str::<api::ScanProgressData>(&text) {
                            Ok(p) => {
                                scan_progress.set(Some((p.current, p.total)));
                                // Stream the new item into the grid immediately.
                                if let Some(new_item) = p.item {
                                    if let Some(pos) = accumulated.iter().position(|e| e.id == new_item.id) {
                                        accumulated[pos] = new_item;
                                    } else {
                                        accumulated.push(new_item);
                                    }
                                    raw_items.set(accumulated.clone());
                                    items.set(api::apply_filters(
                                        &accumulated,
                                        &query,
                                        sort_by,
                                        &mf,
                                    ));
                                }
                            }
                            Err(e) => web_sys::console::warn_1(
                                &format!("scan_ws: unexpected message: {e}").into(),
                            ),
                        }
                    }
                    // WebSocket closed = scan complete.
                }

                // Final authoritative refresh once the scan is fully committed.
                if let Ok(all) = api::fetch_all_videos().await {
                    let filtered = api::apply_filters(&all, &query, sort_by, &mf);
                    raw_items.set(all);
                    items.set(filtered);
                }
                scan_progress.set(None);
                scanning.set(false);
            });
        })
    };

    let app_class = if *dark_mode { "app dark-mode" } else { "app" };

    // Derive the unique sorted tag/actor/category values from the full
    // (unfiltered) library for populating the multi-select dropdowns.
    let all_tags = {
        let mut set = std::collections::BTreeSet::new();
        for item in (*raw_items).iter() {
            for t in &item.tags { set.insert(t.to_lowercase()); }
        }
        set.into_iter().collect::<Vec<_>>()
    };
    let all_actors = {
        let mut set = std::collections::BTreeSet::new();
        for item in (*raw_items).iter() {
            for a in &item.actors { set.insert(a.to_lowercase()); }
        }
        set.into_iter().collect::<Vec<_>>()
    };
    let all_categories = {
        let mut set = std::collections::BTreeSet::new();
        for item in (*raw_items).iter() {
            for c in &item.categories { set.insert(c.to_lowercase()); }
        }
        set.into_iter().collect::<Vec<_>>()
    };

    // Virtual windowing: when no search/metadata filter is active and the library
    // is large enough, only render the WINDOW_SIZE cards around the current scroll
    // position.  Spacer divs above and below preserve document height so the
    // scrollbar stays proportional.  When any filter is active, all matching
    // results are shown without windowing.
    let mf = &*meta_filter;
    let is_filtered = !(*query).is_empty()
        || mf.only_favorites
        || mf.min_rating > 0
        || !mf.tag.is_empty()
        || !mf.actor.is_empty()
        || !mf.category.is_empty();
    let total = (*items).len();
    let (displayed_items, top_pad, bottom_pad) = if is_filtered || total <= WINDOW_SIZE {
        ((*items).clone(), 0.0_f64, 0.0_f64)
    } else {
        let start = *window_start;
        let end = (start + WINDOW_SIZE).min(total);
        let slice = (*items)[start..end].to_vec();

        // Column count mirrors the CSS grid breakpoints in main.css.
        let win_width = web_sys::window()
            .and_then(|w| w.inner_width().ok().and_then(|v| v.as_f64()))
            .unwrap_or(1280.0);
        let col_count = col_count_for_width(win_width);
        let row_height = *card_row_height_ref.borrow();

        // Spacer heights = number of rows hidden above / below × row height.
        // top_rows: floor division — window_start is always row-aligned.
        let top_rows = start / col_count;
        let bottom_items = total.saturating_sub(end);
        // bottom_rows: ceiling division — partial bottom rows still need space.
        let bottom_rows = (bottom_items + col_count - 1) / col_count;

        (slice, top_rows as f64 * row_height, bottom_rows as f64 * row_height)
    };

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

    // Compute progress percentage and label for the segment pre-cache bar.
    let (precache_pct, precache_label) = match *precache_progress {
        Some((current, total)) if total > 0 => (
            (current as f64 / total as f64 * 100.0) as u32,
            format!("Segments: {} / {}", current, total),
        ),
        Some(_) => (0, "Pre-caching segments…".to_string()),
        None => (0, String::new()),
    };

    let any_bg_active = thumb_progress.is_some() || sprite_progress.is_some() || precache_progress.is_some() || *scanning;

    // Look up the current favorite state from items (stays in sync with toggles,
    // unlike `selected` which is a snapshot of the element at selection time).
    let selected_is_fav = selected.as_ref().map(|video| {
        (*items).iter().find(|e| e.id == video.id).map(|e| e.favorite).unwrap_or(video.favorite)
    }).unwrap_or(false);

    html! {
        <>
            <div class={app_class}>
                // Left sidebar with Starfin branding
                <aside class="sidebar">
                    <div class="sidebar__logo">{ "STARFIN" }</div>
                    <div class="sidebar__text">{ "MEDIA COLLECTION" }</div>
                    <div class="sidebar__arrow">
                        <svg class="sidebar__arrow-svg" xmlns="http://www.w3.org/2000/svg" viewBox="0 0 24 24" aria-hidden="true">
                            <line x1="5" y1="19" x2="19" y2="5" stroke="currentColor" stroke-width="2.5" stroke-linecap="round"/>
                            <polyline points="9 5 19 5 19 15" fill="none" stroke="currentColor" stroke-width="2.5" stroke-linejoin="round" stroke-linecap="round"/>
                        </svg>
                    </div>
                </aside>

                // Video player overlay — rendered on top of the library when a video is selected.
                if let Some(video) = &*selected {
                    <VideoPlayer
                        video_id={video.id.clone()}
                        title={video.title.clone()}
                        on_close={on_close_player}
                        is_favorite={selected_is_fav}
                        on_favorite_toggle={{
                            let on_favorite_toggle = on_favorite_toggle.clone();
                            let items = items.clone();
                            let video_id = video.id.clone();
                            Callback::from(move |_| {
                                if let Some(elem) = (*items).iter().find(|e| e.id == video_id).cloned() {
                                    on_favorite_toggle.emit(elem);
                                }
                            })
                        }}
                    />
                }

                // Scroll view overlay — TikTok-style random video scroller.
                if *scroll_mode {
                    <ScrollView
                        items={(*items).clone()}
                        on_close={on_close_scroll.clone()}
                        on_favorite_toggle={on_favorite_toggle.clone()}
                    />
                }

                // Media edit modal — rendered on top of the library when editing.
                if let Some(edit_item) = &*editing {
                    <MediaEditModal
                        item={edit_item.clone()}
                        on_close={on_close_edit.clone()}
                        on_saved={on_metadata_saved.clone()}
                        all_tags={all_tags.clone()}
                        all_actors={all_actors.clone()}
                        all_categories={all_categories.clone()}
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
                                class="topbar-btn"
                                onclick={on_random}
                                disabled={(*items).is_empty()}
                                aria-label="Play a random media file"
                            >
                                { "RANDOM" }
                            </button>
                            <button
                                class="topbar-btn"
                                onclick={on_scroll_mode}
                                disabled={(*items).is_empty()}
                                aria-label="Open scroll view"
                            >
                                { "SCROLL" }
                            </button>
                            <button
                                class={if *scanning { "topbar-btn topbar-btn--disabled" } else { "topbar-btn" }}
                                onclick={on_scan}
                                disabled={*scanning}
                                aria-label="Scan for new media"
                            >
                                { if *scanning { "SCANNING…" } else { "SCAN MEDIA" } }
                            </button>
                            if props.show_logout {
                                <button
                                    class="topbar-btn"
                                    onclick={on_logout}
                                    aria-label="Log out"
                                >
                                    { "LOGOUT" }
                                </button>
                            }
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
                        meta_filter={(*meta_filter).clone()}
                        all_tags={all_tags}
                        all_actors={all_actors}
                        all_categories={all_categories}
                        on_query_change={on_query_change}
                        on_sort_change={on_sort_change}
                        on_filter_change={on_filter_change}
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
                        if precache_progress.is_some() {
                            <div class="process-row">
                                <span class="process-row__label">{ "Pre-cache" }</span>
                                <div class="process-row__bar-wrap">
                                    <div class="process-row__bar-fill" style={format!("width: {}%", precache_pct)} />
                                </div>
                                <span class="process-row__count">{ &precache_label }</span>
                            </div>
                        }
                    </div>
                }

            <main class={if *returning_to_top { "content content--fading" } else { "content" }}>
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
                        items={displayed_items}
                        on_watch={on_watch}
                        on_edit={on_edit}
                        on_favorite_toggle={on_favorite_toggle}
                        thumb_current_id={(*thumb_current_id).clone()}
                        sprite_current_id={(*sprite_current_id).clone()}
                        precache_current_id={(*precache_current_id).clone()}
                        top_pad={top_pad}
                        bottom_pad={bottom_pad}
                    />
                }
            </main>
            </div>

                <button
                    class={if *show_scroll_top { "scroll-top-btn scroll-top-btn--visible" } else { "scroll-top-btn" }}
                    onclick={on_scroll_top}
                    aria-label="Scroll to top"
                >
                    <svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 24 24" fill="currentColor" aria-hidden="true">
                        <path d="M4 12l8-8 8 8-1.41 1.41L13 7.83V20h-2V7.83l-5.59 5.58L4 12z"/>
                    </svg>
                </button>
        </>
    }
}
