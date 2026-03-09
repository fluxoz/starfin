use gloo_net::http::Request;
use gloo_timers::callback::Interval;
use serde::Deserialize;
use std::cell::{Cell, RefCell};
use std::rc::Rc;
use wasm_bindgen::JsCast;
use wasm_bindgen_futures::spawn_local;
use yew::prelude::*;

/// How many milliseconds each sprite frame is shown during the hover preview.
const FRAME_INTERVAL_MS: u32 = 500;

/// Maximum number of polling iterations when waiting for the sprite image to load.
const IMAGE_LOAD_MAX_POLLS: u32 = 200; // 200 × 50 ms = 10 s

#[derive(Clone, Debug, PartialEq, Deserialize)]
struct ThumbnailInfo {
    pub url: String,
    pub sprite_width: u32,
    pub sprite_height: u32,
    pub thumb_width: u32,
    pub thumb_height: u32,
    pub columns: u32,
    pub rows: u32,
    pub interval: f64,
}

#[derive(Clone, Debug, PartialEq, Deserialize)]
struct SpriteStatus {
    pub ready: bool,
}

#[derive(Properties, PartialEq)]
pub struct VideoCardThumbProps {
    pub video_id: String,
}

/// Holds the loaded sprite data so it can be reused across hover sessions.
/// Stored behind Rc<RefCell<>> so it is not used as a Yew dependency directly
/// (HtmlImageElement doesn't implement PartialEq).
struct SpriteData {
    info: ThumbnailInfo,
    image: web_sys::HtmlImageElement,
}

#[function_component(VideoCardThumb)]
pub fn video_card_thumb(props: &VideoCardThumbProps) -> Html {
    let canvas_ref = use_node_ref();

    // Whether the mouse is currently over the thumbnail area.
    let hovering = use_state(|| false);

    // Current frame index for the animation.
    let frame_index = use_state(|| 0_u32);

    // Cached sprite data (fetched once, then re-used).
    // We keep this in Rc<RefCell<>> so we can share it without PartialEq.
    let sprite_data: UseStateHandle<Option<Rc<RefCell<SpriteData>>>> = use_state(|| None);

    // A simple flag that tracks whether the sprite has been loaded.
    // This IS used as a Yew dependency for effects.
    let sprite_loaded = use_state(|| false);

    // Whether we are currently loading the sprite (to avoid duplicate fetches).
    let loading = use_state(|| false);

    // Whether loading the sprite failed or the sprite is simply not available yet.
    let load_failed = use_state(|| false);

    let thumbnail_url = format!("/api/videos/{}/thumbnail", props.video_id);

    // ── Fetch sprite on first hover (only if sprite is already cached) ───────
    {
        let hovering = hovering.clone();
        let loading = loading.clone();
        let load_failed = load_failed.clone();
        let sprite_data = sprite_data.clone();
        let sprite_loaded = sprite_loaded.clone();
        let video_id = props.video_id.clone();

        use_effect_with(
            (*hovering, *sprite_loaded, *loading, *load_failed),
            move |(is_hovering, is_loaded, is_loading, has_failed)| {
                if *is_hovering && !*is_loaded && !*is_loading && !*has_failed {
                    loading.set(true);
                    let sprite_data = sprite_data.clone();
                    let sprite_loaded = sprite_loaded.clone();
                    let loading = loading.clone();
                    let load_failed = load_failed.clone();
                    spawn_local(async move {
                        // First, check if the sprite is already cached on the
                        // server.  This is a cheap filesystem check that never
                        // triggers ffmpeg, so it returns almost instantly.
                        match check_sprite_status(&video_id).await {
                            Ok(status) if status.ready => { /* sprite available — continue */ }
                            Ok(_) => {
                                // Sprite not ready yet.  Mark as failed so we
                                // don't re-check on every hover.
                                loading.set(false);
                                load_failed.set(true);
                                return;
                            }
                            Err(_) => {
                                // Check itself failed (e.g. network error).
                                // Don't mark as permanently failed — a future
                                // hover will retry.
                                loading.set(false);
                                return;
                            }
                        }

                        match fetch_thumbnail_info(&video_id).await {
                            Ok(info) => {
                                let img = web_sys::HtmlImageElement::new().unwrap();
                                img.set_cross_origin(Some("anonymous"));
                                img.set_src(&info.url);

                                // Poll until the image has loaded (with timeout)
                                let mut polls = 0_u32;
                                loop {
                                    if img.complete() && img.natural_width() > 0 {
                                        break;
                                    }
                                    polls += 1;
                                    if polls >= IMAGE_LOAD_MAX_POLLS {
                                        loading.set(false);
                                        load_failed.set(true);
                                        return;
                                    }
                                    gloo_timers::future::TimeoutFuture::new(50).await;
                                }

                                sprite_data.set(Some(Rc::new(RefCell::new(SpriteData {
                                    info,
                                    image: img,
                                }))));
                                sprite_loaded.set(true);
                                loading.set(false);
                            }
                            Err(_) => {
                                loading.set(false);
                                load_failed.set(true);
                            }
                        }
                    });
                }
            },
        );
    }

    // ── Frame animation interval ─────────────────────────────────────────────
    {
        let hovering = hovering.clone();
        let frame_index = frame_index.clone();
        let sprite_data = sprite_data.clone();
        let sprite_loaded = sprite_loaded.clone();

        use_effect_with(
            (*hovering, *sprite_loaded),
            move |(is_hovering, is_loaded)| {
                let mut _interval_guard: Option<Interval> = None;

                if *is_hovering && *is_loaded {
                    if let Some(sd) = &*sprite_data {
                        let total_frames = {
                            let sd = sd.borrow();
                            sd.info.columns * sd.info.rows
                        };

                        frame_index.set(0);

                        // Use an Rc<Cell<>> counter so the interval callback
                        // can read/write the current value without relying on
                        // the UseStateHandle's deref (which returns the value
                        // at the time the effect last ran).
                        let counter = Rc::new(Cell::new(0_u32));
                        let counter_inner = counter.clone();

                        _interval_guard = Some(Interval::new(FRAME_INTERVAL_MS, move || {
                            let next = (counter_inner.get() + 1) % total_frames;
                            counter_inner.set(next);
                            frame_index.set(next);
                        }));
                    }
                }

                move || drop(_interval_guard)
            },
        );
    }

    // ── Draw the current frame onto the canvas ───────────────────────────────
    {
        let canvas_ref = canvas_ref.clone();
        let sprite_data = sprite_data.clone();
        let frame_index = frame_index.clone();
        let hovering = hovering.clone();
        let sprite_loaded = sprite_loaded.clone();

        use_effect_with(
            (*frame_index, *hovering, *sprite_loaded),
            move |(idx, is_hovering, _is_loaded)| {
                if !*is_hovering {
                    return;
                }
                if let Some(sd) = &*sprite_data {
                    let sd = sd.borrow();
                    if let Some(canvas) =
                        canvas_ref.cast::<web_sys::HtmlCanvasElement>()
                    {
                        if let Ok(Some(ctx)) = canvas.get_context("2d") {
                            if let Ok(ctx) =
                                ctx.dyn_into::<web_sys::CanvasRenderingContext2d>()
                            {
                                let col = *idx % sd.info.columns;
                                let row = *idx / sd.info.columns;
                                let sx = (col * sd.info.thumb_width) as f64;
                                let sy = (row * sd.info.thumb_height) as f64;

                                ctx.clear_rect(
                                    0.0,
                                    0.0,
                                    canvas.width() as f64,
                                    canvas.height() as f64,
                                );
                                let _ = ctx.draw_image_with_html_image_element_and_sw_and_sh_and_dx_and_dy_and_dw_and_dh(
                                    &sd.image,
                                    sx, sy,
                                    sd.info.thumb_width as f64, sd.info.thumb_height as f64,
                                    0.0, 0.0,
                                    canvas.width() as f64, canvas.height() as f64,
                                );
                            }
                        }
                    }
                }
            },
        );
    }

    // ── Event handlers ───────────────────────────────────────────────────────
    let on_mouse_enter = {
        let hovering = hovering.clone();
        Callback::from(move |_: MouseEvent| {
            hovering.set(true);
        })
    };

    let on_mouse_leave = {
        let hovering = hovering.clone();
        Callback::from(move |_: MouseEvent| {
            hovering.set(false);
        })
    };

    let show_canvas = *hovering && *sprite_loaded;

    html! {
        <div
            class="card__thumb"
            style={format!("background-image: url('{thumbnail_url}')")}
            onmouseenter={on_mouse_enter}
            onmouseleave={on_mouse_leave}
        >
            <canvas
                ref={canvas_ref}
                class={if show_canvas { "card__preview-canvas card__preview-canvas--visible" } else { "card__preview-canvas" }}
                width="320"
                height="180"
            />
        </div>
    }
}

/// Check whether the sprite sheet for a video is already cached on the server.
/// This endpoint never triggers ffmpeg — it is a lightweight filesystem check.
async fn check_sprite_status(video_id: &str) -> Result<SpriteStatus, String> {
    let url = format!("/api/videos/{video_id}/thumbnails/sprite-status");
    let resp = Request::get(&url)
        .send()
        .await
        .map_err(|e| format!("fetch error: {e:?}"))?;

    if !resp.ok() {
        return Err(format!("HTTP {} for {url}", resp.status()));
    }

    resp.json()
        .await
        .map_err(|e| format!("JSON parse error: {e:?}"))
}

async fn fetch_thumbnail_info(video_id: &str) -> Result<ThumbnailInfo, String> {
    let url = format!("/api/videos/{video_id}/thumbnails/info");
    let resp = Request::get(&url)
        .send()
        .await
        .map_err(|e| format!("fetch error: {e:?}"))?;

    if !resp.ok() {
        return Err(format!("HTTP {} for {url}", resp.status()));
    }

    resp.json()
        .await
        .map_err(|e| format!("JSON parse error: {e:?}"))
}
