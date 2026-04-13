// TikTok-style vertical scroll view — swipe up/down to go to the next/previous
// random video.  Each video starts at a random position between 20-50% of its
// total duration.  The next few videos are queued in the background for
// instant transitions.
//
// Architecture:
//   - A ring buffer of 3 "slots" (prev, current, next), each with its own
//     dash.js MediaPlayer instance.
//   - Touch/wheel/keyboard input drives vertical transitions.
//   - On transition, the old player is destroyed and a new one is created
//     in the vacated slot with the next queued video.

use crate::models::Element;

use gloo_net::http::Request;
use gloo_timers::future::TimeoutFuture;
use std::cell::{Cell, RefCell};
use std::rc::Rc;
use wasm_bindgen::prelude::*;
use wasm_bindgen::JsCast;
use wasm_bindgen_futures::spawn_local;
use web_sys::{window, HtmlVideoElement, TouchEvent};
use yew::prelude::*;

// ── Constants ────────────────────────────────────────────────────────────────

/// Number of slots kept alive (previous, current, next).
const SLOT_COUNT: usize = 3;

/// Minimum swipe distance (px) to trigger a transition.
const SWIPE_THRESHOLD_PX: f64 = 60.0;

/// Duration of the slide animation (ms).
const TRANSITION_MS: u32 = 300;

/// Buffer config for scroll-view players (lower than full player for snappiness).
const SV_BUFFER_TARGET_S: f64 = 15.0;
const SV_BACK_BUFFER_S: f64 = 5.0;

/// Fallback viewport height (px) when `window().inner_height()` is unavailable.
const DEFAULT_VIEWPORT_HEIGHT_PX: f64 = 800.0;

/// Controls auto-hide timeout.
const CONTROLS_HIDE_MS: f64 = 4000.0;

/// Seek step for on-screen controls (seconds).
const SEEK_STEP_S: f64 = 10.0;

/// Stream quality options (mirrored from video_player).
const QUALITY_OPTIONS: [(&str, &str); 5] = [
    ("auto",     "Auto (ABR)"),
    ("original", "Original (Direct)"),
    ("high",     "High (Transcode)"),
    ("medium",   "Medium (720p)"),
    ("low",      "Low (480p)"),
];
const QUALITY_STORAGE_KEY: &str = "starfin_quality";

// ── dash.js interop (re-use from video_player) ──────────────────────────────

#[wasm_bindgen]
extern "C" {
    #[wasm_bindgen(js_namespace = dashjs)]
    type MediaPlayer;

    #[wasm_bindgen(js_namespace = dashjs, js_name = "MediaPlayer")]
    fn media_player_factory() -> JsValue;
}

/// Lightweight dash.js wrapper — only the methods the scroll view needs.
struct DashPlayer {
    player: JsValue,
}

impl DashPlayer {
    fn create() -> Self {
        let factory = media_player_factory();
        let player = js_sys::Reflect::apply(
            &js_sys::Reflect::get(&factory, &"create".into())
                .unwrap()
                .dyn_into::<js_sys::Function>()
                .unwrap(),
            &factory,
            &js_sys::Array::new(),
        )
        .unwrap();
        Self { player }
    }

    fn initialize(&self, video: &HtmlVideoElement, auto_play: bool) {
        let f = js_sys::Reflect::get(&self.player, &"initialize".into())
            .unwrap()
            .dyn_into::<js_sys::Function>()
            .unwrap();
        let args = js_sys::Array::new();
        args.push(video);
        args.push(&JsValue::NULL);
        args.push(&JsValue::from_bool(auto_play));
        let _ = js_sys::Reflect::apply(&f, &self.player, &args);
    }

    fn update_settings(&self, settings: &JsValue) {
        let f = js_sys::Reflect::get(&self.player, &"updateSettings".into())
            .unwrap()
            .dyn_into::<js_sys::Function>()
            .unwrap();
        let _ = f.call1(&self.player, settings);
    }

    fn attach_source(&self, url: &str, start_time: f64) {
        if let Ok(func) = js_sys::Reflect::get(&self.player, &"attachSource".into()) {
            if let Ok(func) = func.dyn_into::<js_sys::Function>() {
                let args = js_sys::Array::new();
                args.push(&JsValue::from_str(url));
                if start_time > 0.0 {
                    args.push(&JsValue::from_f64(start_time));
                }
                let _ = js_sys::Reflect::apply(&func, &self.player, &args);
            }
        }
    }

    fn seek(&self, time: f64) {
        if let Ok(f) = js_sys::Reflect::get(&self.player, &"seek".into()) {
            if let Ok(f) = f.dyn_into::<js_sys::Function>() {
                let _ = f.call1(&self.player, &JsValue::from_f64(time));
            }
        }
    }

    fn play(&self) {
        if let Ok(f) = js_sys::Reflect::get(&self.player, &"play".into()) {
            if let Ok(f) = f.dyn_into::<js_sys::Function>() {
                let _ = f.call0(&self.player);
            }
        }
    }

    fn pause(&self) {
        if let Ok(f) = js_sys::Reflect::get(&self.player, &"pause".into()) {
            if let Ok(f) = f.dyn_into::<js_sys::Function>() {
                let _ = f.call0(&self.player);
            }
        }
    }

    fn is_paused(&self) -> bool {
        if let Ok(f) = js_sys::Reflect::get(&self.player, &"isPaused".into()) {
            if let Ok(f) = f.dyn_into::<js_sys::Function>() {
                if let Ok(v) = f.call0(&self.player) {
                    return v.as_bool().unwrap_or(true);
                }
            }
        }
        true
    }

    fn on(&self, event: &str, callback: &JsValue) {
        let f = js_sys::Reflect::get(&self.player, &"on".into())
            .unwrap()
            .dyn_into::<js_sys::Function>()
            .unwrap();
        let _ = f.call2(&self.player, &JsValue::from_str(event), callback);
    }

    fn set_quality_for(&self, media_type: &str, quality_id: &str, force_replace: bool) {
        set_quality_for_raw(&self.player, media_type, quality_id, force_replace);
    }

    fn destroy(&self) {
        if let Ok(f) = js_sys::Reflect::get(&self.player, &"destroy".into()) {
            if let Ok(f) = f.dyn_into::<js_sys::Function>() {
                let _ = f.call0(&self.player);
            }
        }
    }
}

/// Standalone helper so closures that only capture a raw JsValue can lock quality
/// without duplicating the JS interop code.
fn set_quality_for_raw(player_js: &JsValue, media_type: &str, quality_id: &str, force_replace: bool) {
    if let Ok(func) = js_sys::Reflect::get(player_js, &"setRepresentationForTypeById".into()) {
        if let Ok(func) = func.dyn_into::<js_sys::Function>() {
            let args = js_sys::Array::new();
            args.push(&JsValue::from_str(media_type));
            args.push(&JsValue::from_str(quality_id));
            args.push(&JsValue::from_bool(force_replace));
            let _ = js_sys::Reflect::apply(&func, player_js, &args);
        }
    }
}

// ── Helpers ──────────────────────────────────────────────────────────────────

/// Pick a random item from the list, avoiding `exclude_id` if possible.
fn pick_random(items: &[Element], exclude_id: Option<&str>) -> Option<Element> {
    if items.is_empty() {
        return None;
    }
    // Try up to 10 times to avoid the excluded ID.
    for _ in 0..10 {
        let idx = (js_sys::Math::random() * items.len() as f64) as usize;
        let idx = idx.min(items.len() - 1);
        if items.len() == 1 || exclude_id.is_none() || items[idx].id != exclude_id.unwrap() {
            return Some(items[idx].clone());
        }
    }
    // Fallback — just pick the first non-excluded, or any.
    Some(items[0].clone())
}

/// Compute a random start time between 20–50% of `duration_secs`.
fn random_start_time(duration_secs: u32) -> f64 {
    let d = duration_secs as f64;
    if d < 1.0 {
        return 0.0;
    }
    let pct = 0.20 + js_sys::Math::random() * 0.30; // 0.20 .. 0.50
    (d * pct).floor()
}

fn format_time(seconds: f64) -> String {
    if !seconds.is_finite() || seconds < 0.0 {
        return "0:00".to_string();
    }
    let total_secs = seconds.round() as u64;
    let hours = total_secs / 3600;
    let mins = (total_secs % 3600) / 60;
    let secs = total_secs % 60;
    if hours > 0 {
        format!("{hours}:{mins:02}:{secs:02}")
    } else {
        format!("{mins}:{secs:02}")
    }
}

fn touch_client_y(e: &TouchEvent) -> Option<f64> {
    let touches = e.touches();
    if touches.length() > 0 {
        touches.get(0).map(|t| t.client_y() as f64)
    } else {
        e.changed_touches().get(0).map(|t| t.client_y() as f64)
    }
}

fn touch_client_x(e: &TouchEvent) -> Option<f64> {
    let touches = e.touches();
    if touches.length() > 0 {
        touches.get(0).map(|t| t.client_x() as f64)
    } else {
        e.changed_touches().get(0).map(|t| t.client_x() as f64)
    }
}

/// Build settings JS for dash.js with optional quality lock.
fn make_settings_js_with_quality(quality: &str) -> JsValue {
    let auto_abr = quality == "auto";
    js_sys::eval(&format!(
        r#"({{
            debug: {{ logLevel: 1 }},
            streaming: {{
                scheduling: {{ scheduleWhilePaused: true }},
                buffer: {{
                    bufferTimeDefault: {buf},
                    bufferTimeAtTopQuality: {buf},
                    bufferTimeAtTopQualityLongForm: {buf},
                    bufferToKeep: {back},
                    bufferPruningInterval: 15,
                    avoidCurrentTimeRangePruning: true,
                    stallThreshold: 0.3,
                    reuseExistingSourceBuffers: true,
                    fastSwitchEnabled: true
                }},
                gaps: {{
                    jumpGaps: true,
                    jumpLargeGaps: true,
                    smallGapLimit: 0.8,
                    threshold: 0.5,
                    enableSeekFix: true,
                    enableStallFix: true,
                    stallSeek: 0.1
                }},
                abr: {{
                    autoSwitchBitrate: {{ video: {auto_abr}, audio: false }}
                }},
                retryAttempts: {{
                    MPD: 3,
                    MediaSegment: 3,
                    InitializationSegment: 3
                }},
                retryIntervals: {{
                    MPD: 1000,
                    MediaSegment: 1000,
                    InitializationSegment: 1000
                }},
                cacheInitSegments: true
            }}
        }})"#,
        buf = SV_BUFFER_TARGET_S,
        back = SV_BACK_BUFFER_S,
        auto_abr = auto_abr,
    ))
    .unwrap()
}

// ── Per-slot state (stored in Rc<RefCell<…>>) ────────────────────────────────

struct SlotState {
    /// The Element being played in this slot.
    element: Option<Element>,
    /// dash.js player (if initialised).
    player: Option<DashPlayer>,
}

impl SlotState {
    fn new() -> Self {
        Self {
            element: None,
            player: None,
        }
    }

    /// Destroy the dash.js player if one exists.
    fn teardown(&mut self) {
        if let Some(p) = self.player.take() {
            p.destroy();
        }
        self.element = None;
    }
}

// ── Component ────────────────────────────────────────────────────────────────

#[derive(Properties, PartialEq)]
pub struct ScrollViewProps {
    /// All available media items (used to randomly select videos).
    pub items: Vec<Element>,
    /// Called when user taps the close / back button.
    pub on_close: Callback<()>,
    /// Called when the user toggles the favorite state of the active video.
    pub on_favorite_toggle: Callback<Element>,
}

#[function_component(ScrollView)]
pub fn scroll_view(props: &ScrollViewProps) -> Html {
    // ── Refs for the three <video> elements ─────────────────────────────────
    let video_refs: [NodeRef; SLOT_COUNT] = [use_node_ref(), use_node_ref(), use_node_ref()];

    // ── Shared slot state ───────────────────────────────────────────────────
    let slots: Rc<RefCell<Vec<SlotState>>> = use_mut_ref(|| {
        let mut v = Vec::with_capacity(SLOT_COUNT);
        for _ in 0..SLOT_COUNT {
            v.push(SlotState::new());
        }
        v
    });

    // Which slot index (0..SLOT_COUNT) is currently active / visible.
    let active_slot = use_state(|| 1_usize); // start on the "middle" slot

    // ── Transition / animation state ────────────────────────────────────────
    let translate_y = use_state(|| 0.0_f64);
    let is_animating = use_state(|| false);
    let touch_start_y = use_state(|| 0.0_f64);
    let swiping = use_state(|| false);
    let swipe_delta = use_state(|| 0.0_f64);

    // ── Playback UI state ───────────────────────────────────────────────────
    let is_playing = use_state(|| true);
    let current_time = use_state(|| 0.0_f64);
    let duration = use_state(|| 0.0_f64);
    let controls_visible = use_state(|| true);
    let last_interaction = use_mut_ref(|| js_sys::Date::now());
    let is_muted = use_state(|| false);
    let is_buffering = use_state(|| false);

    // ── Drag-to-seek state ──────────────────────────────────────────────────
    let is_dragging = use_state(|| false);
    let drag_time = use_state(|| 0.0_f64);
    let drag_closures: Rc<RefCell<Option<(Closure<dyn Fn(web_sys::MouseEvent)>, Closure<dyn Fn(web_sys::MouseEvent)>)>>> =
        use_mut_ref(|| None);

    // ── Quality state ───────────────────────────────────────────────────────
    let initial_quality = web_sys::window()
        .and_then(|w| w.local_storage().ok())
        .flatten()
        .and_then(|s| s.get_item(QUALITY_STORAGE_KEY).ok())
        .flatten()
        .filter(|q| QUALITY_OPTIONS.iter().any(|(v, _)| v == q))
        .unwrap_or_else(|| "auto".to_string());
    let selected_quality = use_state(|| initial_quality);
    let quality_menu_open = use_state(|| false);
    let quality_labels: UseStateHandle<Vec<(String, String)>> = use_state(Vec::new);

    // Title of the active video (displayed in the UI).
    let active_title = use_state(|| String::new());
    // Track active video ID for quality label fetching.
    let active_video_id = use_state(|| String::new());
    // Track whether the active video is favorited.
    let active_is_fav = use_state(|| false);

    // ── Initialise the three slots on mount / when items change ─────────────
    {
        let items = props.items.clone();
        let slots = slots.clone();
        let video_refs = video_refs.clone();
        let active_slot = active_slot.clone();
        let active_title = active_title.clone();
        let active_video_id = active_video_id.clone();
        let active_is_fav = active_is_fav.clone();
        let is_buffering = is_buffering.clone();
        let selected_quality = selected_quality.clone();

        use_effect_with(
            items.len(),
            move |_| {
                if !items.is_empty() {
                    let slot_idx = 1_usize; // active = middle
                    active_slot.set(slot_idx);

                    // Pick 3 random videos.
                    let elem0 = pick_random(&items, None);
                    let elem1 = pick_random(&items, elem0.as_ref().map(|e| e.id.as_str()));
                    let elem2 = pick_random(&items, elem1.as_ref().map(|e| e.id.as_str()));

                    let elems = [elem0, elem1, elem2];

                    // Store elements in slot state.
                    {
                        let mut s = slots.borrow_mut();
                        for i in 0..SLOT_COUNT {
                            s[i].teardown();
                            s[i].element = elems[i].clone();
                        }
                    }

                    if let Some(ref e) = elems[slot_idx] {
                        active_title.set(e.title.clone());
                        active_video_id.set(e.id.clone());
                        active_is_fav.set(e.favorite);
                    }

                    // Read quality at init time.
                    let quality_str = (*selected_quality).clone();

                    // Initialise dash.js for each slot (with a small delay so the
                    // DOM has rendered the <video> elements).
                    let slots_init = slots.clone();
                    let video_refs_init = video_refs.clone();
                    let is_buffering_init = is_buffering.clone();

                    spawn_local(async move {
                        TimeoutFuture::new(80).await;

                        for i in 0..SLOT_COUNT {
                            let elem = {
                                let s = slots_init.borrow();
                                s[i].element.clone()
                            };
                            if let Some(elem) = elem {
                                if let Some(video) = video_refs_init[i].cast::<HtmlVideoElement>() {
                                    let _ = video.set_attribute("playsinline", "");
                                    let start = random_start_time(elem.duration_secs);
                                    let url = format!("/api/videos/{}/manifest.mpd", elem.id);

                                    let player = DashPlayer::create();

                                    // Autoplay-blocked handler.
                                    let vid = video.clone();
                                    let pjs = player.player.clone();
                                    let on_not_allowed =
                                        Closure::<dyn Fn()>::new(move || {
                                            vid.set_muted(true);
                                            if let Ok(f) =
                                                js_sys::Reflect::get(&pjs, &"play".into())
                                            {
                                                if let Ok(f) =
                                                    f.dyn_into::<js_sys::Function>()
                                                {
                                                    let _ = f.call0(&pjs);
                                                }
                                            }
                                        });
                                    player.on(
                                        "playbackNotAllowed",
                                        on_not_allowed.as_ref().unchecked_ref(),
                                    );
                                    on_not_allowed.forget();

                                    // Buffering indicators (only for the active slot).
                                    if i == slot_idx {
                                        let ib = is_buffering_init.clone();
                                        let on_stall = Closure::<dyn Fn()>::new(move || {
                                            ib.set(true);
                                        });
                                        player.on("bufferStalled", on_stall.as_ref().unchecked_ref());
                                        on_stall.forget();

                                        let ib2 = is_buffering_init.clone();
                                        let on_loaded = Closure::<dyn Fn()>::new(move || {
                                            ib2.set(false);
                                        });
                                        player.on("bufferLoaded", on_loaded.as_ref().unchecked_ref());
                                        on_loaded.forget();
                                    }

                                    player.initialize(&video, i == slot_idx);
                                    player.update_settings(&make_settings_js_with_quality(&quality_str));

                                    // If quality is not "auto", lock to the selected representation
                                    // after stream initialises.
                                    if quality_str != "auto" {
                                        let pjs_q = player.player.clone();
                                        let qs = quality_str.clone();
                                        let on_stream = Closure::once(Box::new(move || {
                                            set_quality_for_raw(&pjs_q, "video", &qs, false);
                                        }) as Box<dyn FnOnce()>);
                                        player.on("streamInitialized", on_stream.as_ref().unchecked_ref());
                                        on_stream.forget();
                                    }

                                    player.attach_source(&url, start);

                                    // Non-active slots: pause after stream initialised.
                                    if i != slot_idx {
                                        let pjs2 = player.player.clone();
                                        let on_init =
                                            Closure::once(Box::new(move || {
                                                if let Ok(f) = js_sys::Reflect::get(
                                                    &pjs2,
                                                    &"pause".into(),
                                                ) {
                                                    if let Ok(f) =
                                                        f.dyn_into::<js_sys::Function>()
                                                    {
                                                        let _ = f.call0(&pjs2);
                                                    }
                                                }
                                            })
                                                as Box<dyn FnOnce()>);
                                        player.on(
                                            "streamInitialized",
                                            on_init.as_ref().unchecked_ref(),
                                        );
                                        on_init.forget();
                                    }

                                    slots_init.borrow_mut()[i].player = Some(player);
                                }
                            }
                        }
                    });
                }

                // Cleanup on unmount.
                let slots_cleanup = slots;
                move || {
                    let mut s = slots_cleanup.borrow_mut();
                    for slot in s.iter_mut() {
                        slot.teardown();
                    }
                }
            },
        );
    }

    // ── Polling interval for current_time / duration / muted / playing ──────
    {
        let video_refs = video_refs.clone();
        let active_slot = active_slot.clone();
        let current_time = current_time.clone();
        let duration = duration.clone();
        let is_playing = is_playing.clone();
        let is_muted = is_muted.clone();
        let controls_visible = controls_visible.clone();
        let last_interaction = last_interaction.clone();
        let is_dragging = is_dragging.clone();

        use_effect(move || {
            let interval = gloo_timers::callback::Interval::new(150, move || {
                let idx = *active_slot;
                if let Some(video) = video_refs[idx].cast::<HtmlVideoElement>() {
                    let dur = video.duration();
                    // Don't overwrite current_time while the user is dragging the
                    // progress bar — use the drag_time instead.
                    if !*is_dragging {
                        let ct = video.current_time();
                        current_time.set(ct);
                    }
                    if dur.is_finite() && dur > 0.0 {
                        duration.set(dur);
                    }
                    is_playing.set(!video.paused());
                    is_muted.set(video.muted());

                    // Auto-hide controls.
                    let now = js_sys::Date::now();
                    let last = *last_interaction.borrow();
                    if now - last > CONTROLS_HIDE_MS && *controls_visible {
                        controls_visible.set(false);
                    }
                }
            });

            move || drop(interval)
        });
    }

    // ── Fetch quality labels when active video changes ─────────────────────
    {
        let active_video_id = active_video_id.clone();
        let quality_labels = quality_labels.clone();
        use_effect_with((*active_video_id).clone(), move |video_id| {
            let video_id = video_id.clone();
            let quality_labels = quality_labels.clone();
            if !video_id.is_empty() {
                spawn_local(async move {
                    let url = format!("/api/videos/{}/quality-info", video_id);
                    if let Ok(resp) = Request::get(&url).send().await {
                        if resp.ok() {
                            if let Ok(items) = resp.json::<Vec<serde_json::Value>>().await {
                                let labels: Vec<(String, String)> = items
                                    .iter()
                                    .filter_map(|item| {
                                        let v = item.get("value")?.as_str()?.to_string();
                                        let l = item.get("label")?.as_str()?.to_string();
                                        Some((v, l))
                                    })
                                    .collect();
                                quality_labels.set(labels);
                            }
                        }
                    }
                });
            }
            || ()
        });
    }

    // ── Live quality switch — applies to ALL slots ──────────────────────────
    {
        let slots = slots.clone();
        let selected_quality = selected_quality.clone();
        use_effect_with((*selected_quality).clone(), move |quality| {
            let quality = quality.clone();
            let auto_abr = quality == "auto";
            let abr_settings = js_sys::eval(&format!(
                r#"({{ streaming: {{ abr: {{ autoSwitchBitrate: {{ video: {auto_abr}, audio: false }} }} }} }})"#,
                auto_abr = auto_abr,
            )).unwrap();

            let s = slots.borrow();
            for slot in s.iter() {
                if let Some(ref player) = slot.player {
                    player.update_settings(&abr_settings);
                    if !auto_abr {
                        player.set_quality_for("video", &quality, true);
                    }
                }
            }
            || ()
        });
    }

    // ── Transition helper ───────────────────────────────────────────────────
    let do_transition = {
        let slots = slots.clone();
        let video_refs = video_refs.clone();
        let active_slot = active_slot.clone();
        let active_title = active_title.clone();
        let active_video_id = active_video_id.clone();
        let active_is_fav = active_is_fav.clone();
        let is_animating = is_animating.clone();
        let translate_y = translate_y.clone();
        let items = props.items.clone();
        let is_buffering = is_buffering.clone();
        let selected_quality = selected_quality.clone();

        Rc::new(move |direction: i32| {
            // direction: -1 = swipe up (next), +1 = swipe down (prev)
            if *is_animating || items.is_empty() {
                return;
            }

            let cur = *active_slot;
            let target = if direction < 0 {
                // next
                (cur + 1) % SLOT_COUNT
            } else {
                // prev
                (cur + SLOT_COUNT - 1) % SLOT_COUNT
            };

            // Check if target slot has a video.
            {
                let s = slots.borrow();
                if s[target].element.is_none() {
                    return;
                }
            }

            is_animating.set(true);
            is_buffering.set(false);

            // Animate: slide to target.
            // Each slot is 100vh tall. The offset is relative to the active slot.
            let offset = if direction < 0 { -100.0 } else { 100.0 };
            translate_y.set(offset);

            // After animation completes:
            let slots_post = slots.clone();
            let video_refs_post = video_refs.clone();
            let active_slot_post = active_slot.clone();
            let active_title_post = active_title.clone();
            let active_video_id_post = active_video_id.clone();
            let active_is_fav_post = active_is_fav.clone();
            let is_animating_post = is_animating.clone();
            let translate_y_post = translate_y.clone();
            let items_post = items.clone();
            let target_slot = target;
            let quality_str = (*selected_quality).clone();

            spawn_local(async move {
                TimeoutFuture::new(TRANSITION_MS).await;

                // Pause old active slot.
                {
                    let s = slots_post.borrow();
                    if let Some(ref p) = s[cur].player {
                        p.pause();
                    }
                }

                // Play the new active slot.
                {
                    let s = slots_post.borrow();
                    if let Some(ref p) = s[target_slot].player {
                        p.play();
                    }
                    if let Some(ref e) = s[target_slot].element {
                        active_title_post.set(e.title.clone());
                        active_video_id_post.set(e.id.clone());
                        active_is_fav_post.set(e.favorite);
                    }
                }

                // Figure out which slot to recycle (the one that's now "behind").
                let recycle_idx = if direction < 0 {
                    // We moved forward: recycle the slot that was behind old active.
                    (cur + SLOT_COUNT - 1) % SLOT_COUNT
                } else {
                    // We moved backward: recycle the slot that was ahead of old active.
                    (cur + 1) % SLOT_COUNT
                };

                // Teardown the recycled slot and set up a new video.
                let new_elem = {
                    let s = slots_post.borrow();
                    let exclude = s[target_slot].element.as_ref().map(|e| e.id.clone());
                    pick_random(&items_post, exclude.as_deref())
                };

                if let Some(elem) = new_elem {
                    // Teardown old.
                    slots_post.borrow_mut()[recycle_idx].teardown();

                    if let Some(video) = video_refs_post[recycle_idx].cast::<HtmlVideoElement>() {
                        let _ = video.set_attribute("playsinline", "");
                        // Reset the video src so old content doesn't flash.
                        video.remove_attribute("src").ok();
                        video.load();

                        let start = random_start_time(elem.duration_secs);
                        let url = format!("/api/videos/{}/manifest.mpd", elem.id);

                        slots_post.borrow_mut()[recycle_idx].element = Some(elem);

                        // Small delay for DOM to settle.
                        TimeoutFuture::new(50).await;

                        let player = DashPlayer::create();

                        // Autoplay-blocked handler.
                        let vid = video.clone();
                        let pjs = player.player.clone();
                        let on_not_allowed = Closure::<dyn Fn()>::new(move || {
                            vid.set_muted(true);
                            if let Ok(f) = js_sys::Reflect::get(&pjs, &"play".into()) {
                                if let Ok(f) = f.dyn_into::<js_sys::Function>() {
                                    let _ = f.call0(&pjs);
                                }
                            }
                        });
                        player.on("playbackNotAllowed", on_not_allowed.as_ref().unchecked_ref());
                        on_not_allowed.forget();

                        // Pause after init (it's not the active slot).
                        let pjs2 = player.player.clone();
                        let on_init = Closure::once(Box::new(move || {
                            if let Ok(f) = js_sys::Reflect::get(&pjs2, &"pause".into()) {
                                if let Ok(f) = f.dyn_into::<js_sys::Function>() {
                                    let _ = f.call0(&pjs2);
                                }
                            }
                        }) as Box<dyn FnOnce()>);
                        player.on("streamInitialized", on_init.as_ref().unchecked_ref());
                        on_init.forget();

                        player.initialize(&video, false);
                        player.update_settings(&make_settings_js_with_quality(&quality_str));

                        // If quality is not "auto", lock to the selected representation
                        // after stream initialises.
                        if quality_str != "auto" {
                            let pjs_q = player.player.clone();
                            let qs = quality_str.clone();
                            let on_stream = Closure::once(Box::new(move || {
                                set_quality_for_raw(&pjs_q, "video", &qs, false);
                            }) as Box<dyn FnOnce()>);
                            player.on("streamInitialized", on_stream.as_ref().unchecked_ref());
                            on_stream.forget();
                        }

                        player.attach_source(&url, start);

                        slots_post.borrow_mut()[recycle_idx].player = Some(player);
                    }
                }

                // Snap: remove animation, reposition so active is back at center.
                active_slot_post.set(target_slot);
                translate_y_post.set(0.0);
                is_animating_post.set(false);
            });
        })
    };

    // ── Touch handlers ──────────────────────────────────────────────────────
    let on_touchstart = {
        let touch_start_y = touch_start_y.clone();
        let swiping = swiping.clone();
        let swipe_delta = swipe_delta.clone();
        let controls_visible = controls_visible.clone();
        let last_interaction = last_interaction.clone();
        Callback::from(move |e: TouchEvent| {
            if let Some(y) = touch_client_y(&e) {
                touch_start_y.set(y);
                swiping.set(true);
                swipe_delta.set(0.0);
                controls_visible.set(true);
                *last_interaction.borrow_mut() = js_sys::Date::now();
            }
        })
    };

    let on_touchmove = {
        let touch_start_y = touch_start_y.clone();
        let swiping = swiping.clone();
        let swipe_delta = swipe_delta.clone();
        let is_animating = is_animating.clone();
        Callback::from(move |e: TouchEvent| {
            if !*swiping || *is_animating {
                return;
            }
            e.prevent_default();
            if let Some(y) = touch_client_y(&e) {
                let delta = y - *touch_start_y;
                swipe_delta.set(delta);
            }
        })
    };

    let on_touchend = {
        let swiping = swiping.clone();
        let swipe_delta = swipe_delta.clone();
        let do_transition = do_transition.clone();
        Callback::from(move |_e: TouchEvent| {
            swiping.set(false);
            let delta = *swipe_delta;
            swipe_delta.set(0.0);
            if delta.abs() >= SWIPE_THRESHOLD_PX {
                if delta < 0.0 {
                    do_transition(-1); // swipe up → next
                } else {
                    do_transition(1); // swipe down → prev
                }
            }
        })
    };

    // ── Wheel handler (desktop) ─────────────────────────────────────────────
    let on_wheel = {
        let do_transition = do_transition.clone();
        let is_animating = is_animating.clone();
        let controls_visible = controls_visible.clone();
        let last_interaction = last_interaction.clone();
        Callback::from(move |e: web_sys::WheelEvent| {
            e.prevent_default();
            controls_visible.set(true);
            *last_interaction.borrow_mut() = js_sys::Date::now();
            if *is_animating {
                return;
            }
            let dy = e.delta_y();
            if dy > 30.0 {
                do_transition(-1); // scroll down → next
            } else if dy < -30.0 {
                do_transition(1); // scroll up → prev
            }
        })
    };

    // ── Keyboard handler ────────────────────────────────────────────────────
    {
        let do_transition = do_transition.clone();
        let slots = slots.clone();
        let active_slot = active_slot.clone();
        let video_refs = video_refs.clone();
        let controls_visible = controls_visible.clone();
        let last_interaction = last_interaction.clone();
        let is_muted = is_muted.clone();
        let on_close = props.on_close.clone();

        use_effect(move || {
            let handler = Closure::<dyn Fn(web_sys::KeyboardEvent)>::new(move |e: web_sys::KeyboardEvent| {
                controls_visible.set(true);
                *last_interaction.borrow_mut() = js_sys::Date::now();

                match e.key().as_str() {
                    "ArrowDown" | "j" => {
                        e.prevent_default();
                        do_transition(-1);
                    }
                    "ArrowUp" | "k" => {
                        e.prevent_default();
                        do_transition(1);
                    }
                    " " | "Spacebar" => {
                        e.prevent_default();
                        let idx = *active_slot;
                        let s = slots.borrow();
                        if let Some(ref p) = s[idx].player {
                            if p.is_paused() { p.play(); } else { p.pause(); }
                        }
                    }
                    "ArrowLeft" => {
                        e.prevent_default();
                        let idx = *active_slot;
                        if let Some(video) = video_refs[idx].cast::<HtmlVideoElement>() {
                            let t = (video.current_time() - SEEK_STEP_S).max(0.0);
                            let s = slots.borrow();
                            if let Some(ref p) = s[idx].player {
                                p.seek(t);
                            }
                        }
                    }
                    "ArrowRight" => {
                        e.prevent_default();
                        let idx = *active_slot;
                        if let Some(video) = video_refs[idx].cast::<HtmlVideoElement>() {
                            let dur = video.duration();
                            let t = video.current_time() + SEEK_STEP_S;
                            let t = if dur.is_finite() { t.min(dur) } else { t };
                            let s = slots.borrow();
                            if let Some(ref p) = s[idx].player {
                                p.seek(t);
                            }
                        }
                    }
                    "m" => {
                        let idx = *active_slot;
                        if let Some(video) = video_refs[idx].cast::<HtmlVideoElement>() {
                            let new_muted = !video.muted();
                            video.set_muted(new_muted);
                            is_muted.set(new_muted);
                        }
                    }
                    "Escape" => {
                        on_close.emit(());
                    }
                    _ => {}
                }
            });

            let win = window().unwrap();
            let _ = win.add_event_listener_with_callback("keydown", handler.as_ref().unchecked_ref());

            let handler_ref = handler.as_ref().unchecked_ref::<js_sys::Function>().clone();
            move || {
                let _ = window().unwrap().remove_event_listener_with_callback("keydown", &handler_ref);
                drop(handler);
            }
        });
    }

    // ── Button callbacks ────────────────────────────────────────────────────
    let on_close_click = {
        let on_close = props.on_close.clone();
        Callback::from(move |_: MouseEvent| on_close.emit(()))
    };

    let on_play_pause = {
        let slots = slots.clone();
        let active_slot = active_slot.clone();
        let controls_visible = controls_visible.clone();
        let last_interaction = last_interaction.clone();
        Callback::from(move |_: MouseEvent| {
            controls_visible.set(true);
            *last_interaction.borrow_mut() = js_sys::Date::now();
            let idx = *active_slot;
            let s = slots.borrow();
            if let Some(ref p) = s[idx].player {
                if p.is_paused() {
                    p.play();
                } else {
                    p.pause();
                }
            }
        })
    };

    let on_seek_back = {
        let slots = slots.clone();
        let active_slot = active_slot.clone();
        let video_refs = video_refs.clone();
        let controls_visible = controls_visible.clone();
        let last_interaction = last_interaction.clone();
        Callback::from(move |_: MouseEvent| {
            controls_visible.set(true);
            *last_interaction.borrow_mut() = js_sys::Date::now();
            let idx = *active_slot;
            if let Some(video) = video_refs[idx].cast::<HtmlVideoElement>() {
                let t = (video.current_time() - SEEK_STEP_S).max(0.0);
                let s = slots.borrow();
                if let Some(ref p) = s[idx].player {
                    p.seek(t);
                }
            }
        })
    };

    let on_seek_fwd = {
        let slots = slots.clone();
        let active_slot = active_slot.clone();
        let video_refs = video_refs.clone();
        let controls_visible = controls_visible.clone();
        let last_interaction = last_interaction.clone();
        Callback::from(move |_: MouseEvent| {
            controls_visible.set(true);
            *last_interaction.borrow_mut() = js_sys::Date::now();
            let idx = *active_slot;
            if let Some(video) = video_refs[idx].cast::<HtmlVideoElement>() {
                let dur = video.duration();
                let t = video.current_time() + SEEK_STEP_S;
                let t = if dur.is_finite() { t.min(dur) } else { t };
                let s = slots.borrow();
                if let Some(ref p) = s[idx].player {
                    p.seek(t);
                }
            }
        })
    };

    let on_mute_toggle = {
        let active_slot = active_slot.clone();
        let video_refs = video_refs.clone();
        let is_muted = is_muted.clone();
        let controls_visible = controls_visible.clone();
        let last_interaction = last_interaction.clone();
        Callback::from(move |_: MouseEvent| {
            controls_visible.set(true);
            *last_interaction.borrow_mut() = js_sys::Date::now();
            let idx = *active_slot;
            if let Some(video) = video_refs[idx].cast::<HtmlVideoElement>() {
                let new_val = !video.muted();
                video.set_muted(new_val);
                is_muted.set(new_val);
            }
        })
    };

    let on_next = {
        let do_transition = do_transition.clone();
        Callback::from(move |_: MouseEvent| {
            do_transition(-1);
        })
    };

    let on_prev = {
        let do_transition = do_transition.clone();
        Callback::from(move |_: MouseEvent| {
            do_transition(1);
        })
    };

    let on_tap_area = {
        let controls_visible = controls_visible.clone();
        let last_interaction = last_interaction.clone();
        Callback::from(move |_: MouseEvent| {
            controls_visible.set(true);
            *last_interaction.borrow_mut() = js_sys::Date::now();
        })
    };

    // ── Progress bar drag-to-seek (mouse) ──────────────────────────────────
    let progress_ref = use_node_ref();
    let on_progress_mousedown = {
        let slots = slots.clone();
        let active_slot = active_slot.clone();
        let video_refs = video_refs.clone();
        let progress_ref = progress_ref.clone();
        let is_dragging = is_dragging.clone();
        let drag_time = drag_time.clone();
        let current_time = current_time.clone();
        let duration = duration.clone();
        let controls_visible = controls_visible.clone();
        let last_interaction = last_interaction.clone();
        let drag_closures = drag_closures.clone();

        Callback::from(move |e: MouseEvent| {
            e.prevent_default();
            e.stop_propagation();
            controls_visible.set(true);
            *last_interaction.borrow_mut() = js_sys::Date::now();

            let progress_el = match progress_ref.cast::<web_sys::HtmlElement>() { Some(el) => el, None => return };
            let idx = *active_slot;
            let video = match video_refs[idx].cast::<HtmlVideoElement>() { Some(v) => v, None => return };
            let video_duration = video.duration();
            if !video_duration.is_finite() || video_duration <= 0.0 { return; }

            let rect = progress_el.get_bounding_client_rect();
            let click_x = e.client_x() as f64 - rect.left();
            let width = rect.width();
            if width <= 0.0 { return; }

            let seek_ratio = (click_x / width).clamp(0.0, 1.0);
            let initial_seek_time = seek_ratio * video_duration;

            is_dragging.set(true);
            drag_time.set(initial_seek_time);
            current_time.set(initial_seek_time);

            let shared_seek_time: Rc<Cell<f64>> = Rc::new(Cell::new(initial_seek_time));
            let shared_move = shared_seek_time.clone();
            let shared_up = shared_seek_time.clone();

            let progress_ref_move = progress_ref.clone();
            let duration_for_move = *duration;
            let drag_time_move = drag_time.clone();
            let current_time_move = current_time.clone();
            let is_dragging_up = is_dragging.clone();
            let video_refs_up = video_refs.clone();
            let active_slot_up = active_slot.clone();
            let slots_up = slots.clone();
            let last_interaction_move = last_interaction.clone();
            let drag_closures_up = drag_closures.clone();

            let on_mousemove = Closure::<dyn Fn(web_sys::MouseEvent)>::new(move |e: web_sys::MouseEvent| {
                e.prevent_default();
                *last_interaction_move.borrow_mut() = js_sys::Date::now();
                if let Some(el) = progress_ref_move.cast::<web_sys::HtmlElement>() {
                    let rect = el.get_bounding_client_rect();
                    let x = e.client_x() as f64 - rect.left();
                    let ratio = (x / rect.width()).clamp(0.0, 1.0);
                    let t = duration_for_move * ratio;
                    shared_move.set(t);
                    drag_time_move.set(t);
                    current_time_move.set(t);
                }
            });

            let on_mouseup = Closure::<dyn Fn(web_sys::MouseEvent)>::new(move |_e: web_sys::MouseEvent| {
                is_dragging_up.set(false);
                let t = shared_up.get();
                let idx = *active_slot_up;
                if let Some(_video) = video_refs_up[idx].cast::<HtmlVideoElement>() {
                    let s = slots_up.borrow();
                    if let Some(ref p) = s[idx].player {
                        p.seek(t);
                    }
                }
                // Remove window listeners.
                if let Some((ref mv, ref up)) = *drag_closures_up.borrow() {
                    if let Some(win) = window() {
                        let _ = win.remove_event_listener_with_callback("mousemove", mv.as_ref().unchecked_ref());
                        let _ = win.remove_event_listener_with_callback("mouseup", up.as_ref().unchecked_ref());
                    }
                }
                *drag_closures_up.borrow_mut() = None;
            });

            if let Some(win) = window() {
                let _ = win.add_event_listener_with_callback("mousemove", on_mousemove.as_ref().unchecked_ref());
                let _ = win.add_event_listener_with_callback("mouseup", on_mouseup.as_ref().unchecked_ref());
                *drag_closures.borrow_mut() = Some((on_mousemove, on_mouseup));
            }
        })
    };

    // ── Progress bar drag-to-seek (touch) ───────────────────────────────────
    let on_progress_touchstart = {
        let slots = slots.clone();
        let active_slot = active_slot.clone();
        let video_refs = video_refs.clone();
        let progress_ref = progress_ref.clone();
        let is_dragging = is_dragging.clone();
        let drag_time = drag_time.clone();
        let current_time = current_time.clone();
        let duration = duration.clone();
        let controls_visible = controls_visible.clone();
        let last_interaction = last_interaction.clone();

        Callback::from(move |e: TouchEvent| {
            // Do NOT call e.prevent_default() — Yew registers this as a passive listener.
            e.stop_propagation();
            controls_visible.set(true);
            *last_interaction.borrow_mut() = js_sys::Date::now();

            let client_x = match touch_client_x(&e) { Some(x) => x, None => return };
            let progress_el = match progress_ref.cast::<web_sys::HtmlElement>() { Some(el) => el, None => return };
            let idx = *active_slot;
            let video = match video_refs[idx].cast::<HtmlVideoElement>() { Some(v) => v, None => return };
            let video_duration = video.duration();
            if !video_duration.is_finite() || video_duration <= 0.0 { return; }

            let rect = progress_el.get_bounding_client_rect();
            let touch_x = client_x - rect.left();
            let width = rect.width();
            if width <= 0.0 { return; }

            let seek_ratio = (touch_x / width).clamp(0.0, 1.0);
            let initial_seek_time = seek_ratio * video_duration;

            is_dragging.set(true);
            drag_time.set(initial_seek_time);
            current_time.set(initial_seek_time);

            let shared_seek_time: Rc<Cell<f64>> = Rc::new(Cell::new(initial_seek_time));
            let shared_move = shared_seek_time.clone();
            let shared_end = shared_seek_time.clone();

            let progress_ref_move = progress_ref.clone();
            let duration_for_move = *duration;
            let drag_time_move = drag_time.clone();
            let current_time_move = current_time.clone();
            let is_dragging_end = is_dragging.clone();
            let video_refs_end = video_refs.clone();
            let active_slot_end = active_slot.clone();
            let slots_end = slots.clone();
            let last_interaction_move = last_interaction.clone();

            // Must register with { passive: false } to allow preventDefault in touchmove.
            let on_touchmove = Closure::<dyn Fn(TouchEvent)>::new(move |e: TouchEvent| {
                e.prevent_default();
                *last_interaction_move.borrow_mut() = js_sys::Date::now();
                if let Some(x) = touch_client_x(&e) {
                    if let Some(el) = progress_ref_move.cast::<web_sys::HtmlElement>() {
                        let rect = el.get_bounding_client_rect();
                        let lx = x - rect.left();
                        let ratio = (lx / rect.width()).clamp(0.0, 1.0);
                        let t = duration_for_move * ratio;
                        shared_move.set(t);
                        drag_time_move.set(t);
                        current_time_move.set(t);
                    }
                }
            });

            let closures_ref: Rc<RefCell<Option<(Closure<dyn Fn(TouchEvent)>, Closure<dyn Fn(TouchEvent)>)>>> =
                Rc::new(RefCell::new(None));
            let closures_end = closures_ref.clone();

            let on_touchend = Closure::<dyn Fn(TouchEvent)>::new(move |_e: TouchEvent| {
                is_dragging_end.set(false);
                let t = shared_end.get();
                let idx = *active_slot_end;
                if let Some(_video) = video_refs_end[idx].cast::<HtmlVideoElement>() {
                    let s = slots_end.borrow();
                    if let Some(ref p) = s[idx].player {
                        p.seek(t);
                    }
                }
                if let Some((ref mv, ref up)) = *closures_end.borrow() {
                    if let Some(win) = window() {
                        let _ = win.remove_event_listener_with_callback("touchmove", mv.as_ref().unchecked_ref());
                        let _ = win.remove_event_listener_with_callback("touchend", up.as_ref().unchecked_ref());
                    }
                }
                *closures_end.borrow_mut() = None;
            });

            if let Some(win) = window() {
                // Register touchmove as non-passive so we can preventDefault.
                let opts = web_sys::AddEventListenerOptions::new();
                opts.set_passive(false);
                let _ = win.add_event_listener_with_callback_and_add_event_listener_options(
                    "touchmove",
                    on_touchmove.as_ref().unchecked_ref(),
                    &opts,
                );
                let _ = win.add_event_listener_with_callback("touchend", on_touchend.as_ref().unchecked_ref());
                *closures_ref.borrow_mut() = Some((on_touchmove, on_touchend));
            }
        })
    };

    // ── Quality callbacks ───────────────────────────────────────────────────
    let on_quality_toggle = {
        let quality_menu_open = quality_menu_open.clone();
        let controls_visible = controls_visible.clone();
        let last_interaction = last_interaction.clone();
        Callback::from(move |e: MouseEvent| {
            e.stop_propagation();
            controls_visible.set(true);
            *last_interaction.borrow_mut() = js_sys::Date::now();
            quality_menu_open.set(!*quality_menu_open);
        })
    };

    let on_quality_select = {
        let selected_quality = selected_quality.clone();
        let quality_menu_open = quality_menu_open.clone();
        Callback::from(move |quality: String| {
            quality_menu_open.set(false);
            // Persist to localStorage.
            if let Some(storage) = web_sys::window()
                .and_then(|w| w.local_storage().ok())
                .flatten()
            {
                let _ = storage.set_item(QUALITY_STORAGE_KEY, &quality);
            }
            selected_quality.set(quality);
        })
    };

    // ── Favorite callback ───────────────────────────────────────────────────
    let on_fav_toggle = {
        let slots = slots.clone();
        let active_slot = active_slot.clone();
        let active_is_fav = active_is_fav.clone();
        let on_favorite_toggle = props.on_favorite_toggle.clone();
        Callback::from(move |e: MouseEvent| {
            e.stop_propagation();
            let idx = *active_slot;
            let elem = slots.borrow()[idx].element.clone();
            if let Some(elem) = elem {
                // Optimistic update of the visible icon.
                active_is_fav.set(!*active_is_fav);
                on_favorite_toggle.emit(elem);
            }
        })
    };

    // ── Render ──────────────────────────────────────────────────────────────
    let active = *active_slot;
    let progress_pct = if *duration > 0.0 {
        (if *is_dragging { *drag_time } else { *current_time } / *duration * 100.0).clamp(0.0, 100.0)
    } else {
        0.0
    };

    // Quality button label.
    let current_quality_label: String = {
        let cur = selected_quality.as_str();
        let fallback = QUALITY_OPTIONS.iter().find(|(v, _)| *v == cur).map(|(_, l)| l.to_string())
            .unwrap_or_else(|| QUALITY_OPTIONS[0].1.to_string());
        if quality_labels.is_empty() {
            fallback
        } else {
            quality_labels.iter().find(|(v, _)| v.as_str() == cur).map(|(_, l)| l.clone())
                .unwrap_or(fallback)
        }
    };

    let controls_class = if *controls_visible {
        "sv-controls"
    } else {
        "sv-controls sv-controls--hidden"
    };

    // Compute the visual offset for the 3-slot container.
    // The container has 3 stacked slots each 100vh tall.
    // We position so that the active slot is visible, then apply translate_y
    // for animation + swipe_delta for live dragging.
    let base_offset = -(active as f64 * 100.0); // e.g., slot 1 → -100vh
    let anim_offset = *translate_y;
    let drag_offset_vh = if *swiping && *swipe_delta != 0.0 {
        // Convert px delta to vh.
        if let Some(win) = window() {
            let vh = win.inner_height().ok().and_then(|v| v.as_f64()).unwrap_or(DEFAULT_VIEWPORT_HEIGHT_PX);
            *swipe_delta / vh * 100.0
        } else {
            0.0
        }
    } else {
        0.0
    };
    let total_offset = base_offset + anim_offset + drag_offset_vh;

    let container_style = if *is_animating {
        format!(
            "transform: translateY({}vh); transition: transform {}ms ease-out;",
            total_offset, TRANSITION_MS
        )
    } else {
        format!("transform: translateY({}vh);", total_offset)
    };

    html! {
        <div class="sv-overlay"
            ontouchstart={on_touchstart}
            ontouchmove={on_touchmove}
            ontouchend={on_touchend}
            onwheel={on_wheel}
            onclick={on_tap_area}
        >
            // Slots container — 3 × 100vh tall
            <div class="sv-slots" style={container_style}>
                { for (0..SLOT_COUNT).map(|i| {
                    html! {
                        <div class="sv-slot">
                            <video
                                ref={video_refs[i].clone()}
                                class="sv-video"
                                playsinline={true}
                            />
                        </div>
                    }
                })}
            </div>

            // Buffering spinner
            if *is_buffering {
                <div class="sv-buffering">
                    <div class="sv-buffering__spinner"></div>
                </div>
            }

            // Title overlay (top)
            <div class={if *controls_visible { "sv-title" } else { "sv-title sv-title--hidden" }}>
                <button class="sv-back-btn" onclick={on_close_click}>
                    { icon_arrow_back() }
                </button>
                <span class="sv-title__text">{ (*active_title).clone() }</span>
                <button
                    class={if *active_is_fav { "sv-fav-btn sv-fav-btn--active" } else { "sv-fav-btn" }}
                    onclick={on_fav_toggle}
                    aria-label={if *active_is_fav { "Remove from favorites" } else { "Add to favorites" }}
                    aria-pressed={(*active_is_fav).to_string()}
                >
                    { icon_favorite() }
                </button>
            </div>

            // On-screen controls
            <div class={controls_class}>
                // Progress bar with drag-to-seek
                <div ref={progress_ref}
                    class={if *is_dragging { "sv-progress sv-progress--dragging" } else { "sv-progress" }}
                    onmousedown={on_progress_mousedown}
                    ontouchstart={on_progress_touchstart}
                >
                    <div class="sv-progress__filled" style={format!("width: {}%", progress_pct)} />
                    <div class={if *is_dragging { "sv-progress__thumb sv-progress__thumb--dragging" } else { "sv-progress__thumb" }}
                         style={format!("left: {}%", progress_pct)} />
                </div>

                <div class="sv-controls__row">
                    <div class="sv-controls__left">
                        <span class="sv-time">
                            { format_time(if *is_dragging { *drag_time } else { *current_time }) }
                            { " / " }
                            { format_time(*duration) }
                        </span>
                    </div>

                    <div class="sv-controls__center">
                        <button class="sv-btn" onclick={on_prev} aria-label="Previous video">
                            { icon_chevron_up() }
                        </button>
                        <button class="sv-btn" onclick={on_seek_back} aria-label="Seek back 10s">
                            { icon_seek_back() }
                        </button>
                        <button class="sv-btn sv-btn--play" onclick={on_play_pause} aria-label="Play/Pause">
                            if *is_playing {
                                { icon_pause() }
                            } else {
                                { icon_play() }
                            }
                        </button>
                        <button class="sv-btn" onclick={on_seek_fwd} aria-label="Seek forward 10s">
                            { icon_seek_fwd() }
                        </button>
                        <button class="sv-btn" onclick={on_next} aria-label="Next video">
                            { icon_chevron_down() }
                        </button>
                    </div>

                    <div class="sv-controls__right">
                        // Quality selector
                        <div class="sv-quality">
                            <button class="sv-btn sv-btn--text" onclick={on_quality_toggle} title="Stream quality">
                                { current_quality_label }
                            </button>
                            if *quality_menu_open {
                                <div class="sv-quality__menu">
                                    { for QUALITY_OPTIONS.iter().map(|(value, label)| {
                                        let on_select = on_quality_select.clone();
                                        let is_active = selected_quality.as_str() == *value;
                                        let vs = value.to_string();
                                        let display_label = if quality_labels.is_empty() {
                                            label.to_string()
                                        } else {
                                            quality_labels.iter().find(|(v, _)| v.as_str() == *value).map(|(_, l)| l.clone()).unwrap_or_else(|| label.to_string())
                                        };
                                        html! {
                                            <button class={if is_active { "sv-quality__option sv-quality__option--active" } else { "sv-quality__option" }}
                                                onclick={Callback::from(move |e: MouseEvent| { e.stop_propagation(); on_select.emit(vs.clone()); })}>
                                                { display_label }
                                            </button>
                                        }
                                    })}
                                </div>
                            }
                        </div>
                        <button class="sv-btn" onclick={on_mute_toggle} aria-label="Toggle mute">
                            if *is_muted {
                                { icon_volume_off() }
                            } else {
                                { icon_volume_on() }
                            }
                        </button>
                    </div>
                </div>
            </div>
        </div>
    }
}

// ── SVG Icons ────────────────────────────────────────────────────────────────

fn icon_arrow_back() -> Html {
    html! {
        <svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 24 24" width="24" height="24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round">
            <polyline points="15 18 9 12 15 6" />
        </svg>
    }
}

fn icon_play() -> Html {
    html! {
        <svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 24 24" width="28" height="28" fill="currentColor">
            <polygon points="6,3 20,12 6,21" />
        </svg>
    }
}

fn icon_pause() -> Html {
    html! {
        <svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 24 24" width="28" height="28" fill="currentColor">
            <rect x="5" y="3" width="4" height="18" />
            <rect x="15" y="3" width="4" height="18" />
        </svg>
    }
}

fn icon_seek_back() -> Html {
    html! {
        <svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 24 24" width="22" height="22" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round">
            <path d="M12.5 8L7.5 12l5 4" />
            <path d="M17.5 8L12.5 12l5 4" />
        </svg>
    }
}

fn icon_seek_fwd() -> Html {
    html! {
        <svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 24 24" width="22" height="22" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round">
            <path d="M6.5 8l5 4-5 4" />
            <path d="M11.5 8l5 4-5 4" />
        </svg>
    }
}

fn icon_chevron_up() -> Html {
    html! {
        <svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 24 24" width="22" height="22" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round">
            <polyline points="18 15 12 9 6 15" />
        </svg>
    }
}

fn icon_chevron_down() -> Html {
    html! {
        <svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 24 24" width="22" height="22" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round">
            <polyline points="6 9 12 15 18 9" />
        </svg>
    }
}

fn icon_volume_on() -> Html {
    html! {
        <svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 24 24" width="22" height="22" fill="currentColor">
            <path d="M3 9v6h4l5 5V4L7 9H3z"/>
            <path d="M16.5 12c0-1.77-1.02-3.29-2.5-4.03v8.05c1.48-.73 2.5-2.25 2.5-4.02z" />
        </svg>
    }
}

fn icon_volume_off() -> Html {
    html! {
        <svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 24 24" width="22" height="22" fill="currentColor">
            <path d="M3 9v6h4l5 5V4L7 9H3z"/>
            <line x1="23" y1="9" x2="17" y2="15" stroke="currentColor" stroke-width="2"/>
            <line x1="17" y1="9" x2="23" y2="15" stroke="currentColor" stroke-width="2"/>
        </svg>
    }
}

fn icon_favorite() -> Html {
    html! {
        <svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 24 24" width="22" height="22" fill="currentColor" aria-hidden="true">
            <path d="M12 21.35l-1.45-1.32C5.4 15.36 2 12.28 2 8.5 2 5.42 4.42 3 7.5 3c1.74 0 3.41.81 4.5 2.09C13.09 3.81 14.76 3 16.5 3 19.58 3 22 5.42 22 8.5c0 3.78-3.4 6.86-8.55 11.54L12 21.35z"/>
        </svg>
    }
}
