use gloo_net::http::Request;
use js_sys::{Array, Function, Promise, Uint8Array};
use wasm_bindgen::prelude::*;
use wasm_bindgen::JsCast;
use wasm_bindgen_futures::{spawn_local, JsFuture};
use web_sys::{HtmlVideoElement, MediaSource, SourceBuffer};
use yew::prelude::*;

// ── Low-level helpers ────────────────────────────────────────────────────────

/// Returns a [`JsFuture`] that resolves the next time `event` fires on a
/// [`MediaSource`].  Uses `set_onsourceopen` so no extra web-sys features are
/// needed beyond `MediaSource` itself.
fn sourceopen_future(ms: &MediaSource) -> JsFuture {
    let p = Promise::new(&mut |resolve: Function, _: Function| {
        let cb = Closure::once_into_js(move || {
            resolve.call0(&JsValue::NULL).ok();
        });
        ms.set_onsourceopen(Some(cb.unchecked_ref()));
    });
    JsFuture::from(p)
}

/// Returns a [`JsFuture`] that resolves the next time `updateend` fires on a
/// [`SourceBuffer`].  Must be registered *before* calling `append_buffer` so
/// the event is never missed.
fn updateend_future(sb: &SourceBuffer) -> JsFuture {
    let p = Promise::new(&mut |resolve: Function, _: Function| {
        let cb = Closure::once_into_js(move || {
            resolve.call0(&JsValue::NULL).ok();
        });
        sb.set_onupdateend(Some(cb.unchecked_ref()));
    });
    JsFuture::from(p)
}

/// Fetch raw bytes from a URL via the browser's native fetch.
/// Returns an error if the request fails **or** the server responds with a
/// non-2xx status code (so that 404 / 5xx error bodies are never mistaken
/// for valid segment data).
async fn fetch_bytes(url: &str) -> Result<Vec<u8>, String> {
    let resp = Request::get(url)
        .send()
        .await
        .map_err(|e| format!("fetch error: {e:?}"))?;
    if !resp.ok() {
        return Err(format!("HTTP {} for {url}", resp.status()));
    }
    resp.binary().await.map_err(|e| format!("binary error: {e:?}"))
}

/// Parse an HLS playlist and return `(init_uri, segment_uris)`.
///
/// The backend already rewrites all URIs to absolute API paths, so no
/// base-URL resolution is needed here.
fn parse_m3u8(text: &str) -> (Option<String>, Vec<String>) {
    let mut init_uri: Option<String> = None;
    let mut segs: Vec<String> = Vec::new();

    for line in text.lines() {
        let line = line.trim();
        if let Some(rest) = line.strip_prefix("#EXT-X-MAP:URI=\"") {
            if let Some(uri) = rest.strip_suffix('"') {
                init_uri = Some(uri.to_owned());
            }
        } else if !line.starts_with('#') && !line.is_empty() {
            segs.push(line.to_owned());
        }
    }
    (init_uri, segs)
}

/// Arm both the `updateend` (success) and `error` (failure) futures, then
/// call `appendBuffer`.  Awaiting the returned future blocks until the
/// SourceBuffer finishes processing.
///
/// Using `Promise.race` between the two event handlers means that a
/// SourceBuffer decode error is surfaced immediately as an `Err` rather than
/// being silently swallowed.  Without this, a decode error fires `error` then
/// `updateend`; the old code would see `updateend` and return `Ok(())`, never
/// detecting the failure.  On Chromium-based browsers a decode error also
/// triggers an internal `endOfStream("decode")` call which transitions the
/// `MediaSource` to `"ended"`, causing the *next* `appendBuffer` to throw
/// `InvalidStateError`.
async fn append_segment(sb: &SourceBuffer, data: &[u8]) -> Result<(), String> {
    // If the SourceBuffer is currently updating, wait for it to finish.
    while sb.updating() {
        updateend_future(sb)
            .await
            .map_err(|e| format!("waiting for update: {e:?}"))?;
    }

    // Register *both* listeners before calling appendBuffer so neither event
    // can be missed.
    //   • updateend_p resolves → append succeeded
    //   • error_p   rejects  → SourceBuffer decode error
    let updateend_p = Promise::new(&mut |resolve: Function, _: Function| {
        let cb = Closure::once_into_js(move || {
            resolve.call0(&JsValue::NULL).ok();
        });
        sb.set_onupdateend(Some(cb.unchecked_ref()));
    });
    let error_p = Promise::new(&mut |_: Function, reject: Function| {
        let cb = Closure::once_into_js(move || {
            reject.call0(&JsValue::NULL).ok();
        });
        sb.set_onerror(Some(cb.unchecked_ref()));
    });
    let race = Promise::race(&Array::of2(updateend_p.as_ref(), error_p.as_ref()));

    let arr = Uint8Array::from(data);
    if let Err(e) = sb.append_buffer_with_array_buffer_view(arr.unchecked_ref()) {
        // appendBuffer threw synchronously (e.g. InvalidStateError because the
        // MediaSource is no longer open).  Clear both handlers before returning.
        sb.set_onupdateend(None);
        sb.set_onerror(None);
        return Err(format!("appendBuffer: {e:?}"));
    }

    let result = JsFuture::from(race).await;
    // Clean up whichever handler did not fire.
    sb.set_onupdateend(None);
    sb.set_onerror(None);
    result.map_err(|e| format!("SourceBuffer decode error: {e:?}"))?;
    Ok(())
}

// ── Component ────────────────────────────────────────────────────────────────

#[derive(Properties, PartialEq)]
pub struct VideoPlayerProps {
    pub video_id: String,
    pub title: String,
    pub on_close: Callback<()>,
}

#[function_component(VideoPlayer)]
pub fn video_player(props: &VideoPlayerProps) -> Html {
    let video_ref = use_node_ref();
    // Human-readable status shown while buffering.
    let status = use_state(|| "Preparing stream…".to_string());
    let error = use_state(|| Option::<String>::None);

    {
        let video_ref = video_ref.clone();
        let video_id = props.video_id.clone();
        let status = status.clone();
        let error = error.clone();

        use_effect_with(props.video_id.clone(), move |_| {
            spawn_local(async move {
                if let Err(msg) = run_player(video_ref, &video_id, status).await {
                    error.set(Some(msg));
                }
            });
            || ()
        });
    }

    let on_close = props.on_close.clone();
    let title = props.title.clone();

    html! {
        <div class="player-overlay">
            <div class="player-header">
                <button
                    class="btn btn--back"
                    onclick={Callback::from(move |_| on_close.emit(()))}
                >
                    { "← Back to library" }
                </button>
                <span class="player-title">{ title }</span>
            </div>

            if let Some(err) = &*error {
                <div class="notice notice--error">
                    <div class="notice__title">{ "Playback error" }</div>
                    <div class="notice__body">{ err }</div>
                </div>
            }

            if !(*status).is_empty() && (*error).is_none() {
                <div class="player-status">{ &*status }</div>
            }

            <video
                ref={video_ref}
                controls={true}
                class="video-el"
            />
        </div>
    }
}

// ── Player logic (async) ─────────────────────────────────────────────────────

/// All async work for setting up and feeding the MSE player.
/// Separated from the component to keep error handling clean.
async fn run_player(
    video_ref: NodeRef,
    video_id: &str,
    status: UseStateHandle<String>,
) -> Result<(), String> {
    let playlist_url = format!("/api/videos/{video_id}/playlist.m3u8");

    let video = video_ref
        .cast::<HtmlVideoElement>()
        .ok_or("video element unavailable")?;

    // ── Safari: native HLS support via <video src="playlist.m3u8"> ───────────
    // `canPlayType` returns "" (no), "maybe", or "probably".
    if !video.can_play_type("application/vnd.apple.mpegurl").is_empty() {
        video.set_src(&playlist_url);
        status.set(String::new());
        return Ok(());
    }

    // ── Other browsers: fMP4 HLS via the Media Source Extensions API ─────────
    // H.264 Baseline 3.1 / AAC-LC – the most universally supported combination.
    let mime = r#"video/mp4; codecs="avc1.42E01E,mp4a.40.2""#;
    if !MediaSource::is_type_supported(mime) {
        return Err(
            "Your browser does not support the required video codec (H.264 + AAC in fMP4)."
                .into(),
        );
    }

    // Fetch and parse the HLS playlist.
    status.set("Fetching playlist…".into());
    let playlist_bytes = fetch_bytes(&playlist_url).await?;
    let playlist_text = String::from_utf8(playlist_bytes)
        .map_err(|e| format!("playlist UTF-8: {e}"))?;
    let (init_uri, seg_uris) = parse_m3u8(&playlist_text);

    if seg_uris.is_empty() {
        return Err("Playlist contains no segments.".into());
    }

    // Create a MediaSource and attach it to the <video> element via an object URL.
    let ms = MediaSource::new().map_err(|e| format!("MediaSource::new: {e:?}"))?;
    let obj_url =
        web_sys::Url::create_object_url_with_source(&ms).map_err(|e| format!("createObjectURL: {e:?}"))?;
    video.set_src(&obj_url);

    // Wait until the MediaSource transitions to "open".
    sourceopen_future(&ms).await.map_err(|e| format!("sourceopen: {e:?}"))?;

    let sb = ms
        .add_source_buffer(mime)
        .map_err(|e| format!("addSourceBuffer: {e:?}"))?;

    // Append the fMP4 initialisation segment (codec + track info).
    if let Some(init_url) = init_uri {
        status.set("Loading init segment…".into());
        let data = fetch_bytes(&init_url).await?;
        append_segment(&sb, &data).await?;
    }

    // Stream the first few media segments so playback can begin quickly.
    let initial_count = 2.min(seg_uris.len());
    for (i, url) in seg_uris[..initial_count].iter().enumerate() {
        status.set(format!("Buffering segment {}/{}…", i + 1, seg_uris.len()));
        let data = fetch_bytes(url).await?;
        append_segment(&sb, &data).await?;
    }

    // Playback is ready – clear the status overlay.
    status.set(String::new());

    // Fetch and append the remaining segments in the background.
    for (i, url) in seg_uris[initial_count..].iter().enumerate() {
        let seg_num = i + initial_count;
        let data = fetch_bytes(url).await.map_err(|e| {
            format!("Segment {seg_num} fetch failed: {e}")
        })?;
        append_segment(&sb, &data).await?;
    }

    ms.end_of_stream().map_err(|e| format!("endOfStream: {e:?}"))?;
    web_sys::Url::revoke_object_url(&obj_url).ok();
    Ok(())
}
