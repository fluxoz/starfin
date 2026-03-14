use crate::components::processing_status::ProcessingStatus;
use crate::components::video_card_thumb::VideoCardThumb;
use crate::models::Element;
use chrono::DateTime;
use yew::prelude::*;

#[derive(Properties, PartialEq)]
pub struct Props {
    pub items: Vec<Element>,
    pub on_watch: Callback<Element>,
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

#[derive(Properties, PartialEq)]
struct CardProps {
    pub item: Element,
    pub on_watch: Callback<Element>,
    /// Whether the thumbnail worker is currently processing this video.
    #[prop_or_default]
    pub is_thumb_processing: bool,
    /// Whether the sprite worker is currently processing this video.
    #[prop_or_default]
    pub is_sprite_processing: bool,
    /// Whether the pre-cache worker is currently processing this video.
    #[prop_or_default]
    pub is_precache_processing: bool,
    /// Bumps whenever a background worker finishes a video or a batch.
    #[prop_or_default]
    pub processing_version: u32,
}

fn format_duration(secs: u32) -> String {
    let h = secs / 3600;
    let m = (secs % 3600) / 60;
    if h > 0 {
        format!("{}h {}m", h, m)
    } else {
        format!("{}m", m)
    }
}

fn format_date(timestamp: u64) -> Option<String> {
    let secs = i64::try_from(timestamp).ok()?;
    DateTime::from_timestamp(secs, 0)
        .map(|dt| dt.format("%b %d, %Y").to_string())
}

#[function_component(VideoCard)]
fn video_card(props: &CardProps) -> Html {
    let item = &props.item;
    let item_clone = item.clone();
    let on_watch = props.on_watch.clone();

    html! {
        <article class="card">
            <VideoCardThumb
                video_id={item.id.clone()}
                processing_version={props.processing_version}
            />

            <div class="card__top">
                <div class="card__title">{ item.title.clone() }</div>
                <ProcessingStatus
                    video_id={item.id.clone()}
                    is_thumb_processing={props.is_thumb_processing}
                    is_sprite_processing={props.is_sprite_processing}
                    is_precache_processing={props.is_precache_processing}
                    processing_version={props.processing_version}
                />
            </div>

            <div class="card__meta">
                if item.duration_secs > 0 {
                    <span class="card__meta-item card__meta-item--highlight">{ format_duration(item.duration_secs) }</span>
                }
                if item.year > 0 {
                    if item.duration_secs > 0 {
                        <span class="card__meta-sep">{ "·" }</span>
                    }
                    <span class="card__meta-item">{ item.year }</span>
                }
                if let Some(date_str) = format_date(item.date_added) {
                    if item.duration_secs > 0 || item.year > 0 {
                        <span class="card__meta-sep">{ "·" }</span>
                    }
                    <span class="card__meta-item">{ format!("Added {}", date_str) }</span>
                }
            </div>

            <div class="card__footer">
                if item.rating > 0.0 {
                    <div class="muted">{ format!("★ {:.1}", item.rating) }</div>
                } else {
                    <div />
                }
                <button
                    class="btn btn--watch"
                    type="button"
                    onclick={Callback::from(move |_| on_watch.emit(item_clone.clone()))}
                >
                    { "▶ Watch" }
                </button>
            </div>
        </article>
    }
}

#[function_component(ElementsGrid)]
pub fn elements_grid(props: &Props) -> Html {
    if props.items.is_empty() {
        return html! {
            <div class="empty">
                <div class="empty__title">{ "No results" }</div>
                <div class="empty__body">{ "Try adjusting your search or filters." }</div>
            </div>
        };
    }

    html! {
        <section class="grid" aria-label="Videos grid">
            { for props.items.iter().map(|item| {
                let is_thumb_processing =
                    props.thumb_current_id.as_deref() == Some(item.id.as_str());
                let is_sprite_processing =
                    props.sprite_current_id.as_deref() == Some(item.id.as_str());
                let is_precache_processing =
                    props.precache_current_id.as_deref() == Some(item.id.as_str());

                html! {
                    <VideoCard
                        key={item.id.clone()}
                        item={item.clone()}
                        on_watch={props.on_watch.clone()}
                        is_thumb_processing={is_thumb_processing}
                        is_sprite_processing={is_sprite_processing}
                        is_precache_processing={is_precache_processing}
                        processing_version={props.processing_version}
                    />
                }
            }) }
        </section>
    }
}
