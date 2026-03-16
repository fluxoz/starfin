use crate::components::processing_status::ProcessingStatus;
use crate::components::video_card_thumb::VideoCardThumb;
use crate::models::Element;
use chrono::DateTime;
use yew::prelude::*;

#[derive(Properties, PartialEq)]
pub struct Props {
    pub items: Vec<Element>,
    pub on_watch: Callback<Element>,
    pub on_edit: Callback<Element>,
    pub on_favorite_toggle: Callback<Element>,
    /// The video IDs the thumbnail worker is currently processing (from WS).
    #[prop_or_default]
    pub thumb_current_id: Vec<String>,
    /// The video IDs the sprite worker is currently processing (from WS).
    #[prop_or_default]
    pub sprite_current_id: Vec<String>,
    /// The video ID the pre-cache worker is currently processing (from WS).
    #[prop_or_default]
    pub precache_current_id: Option<String>,
    /// Height in pixels of the virtual spacer above the rendered window.
    #[prop_or_default]
    pub top_pad: f64,
    /// Height in pixels of the virtual spacer below the rendered window.
    #[prop_or_default]
    pub bottom_pad: f64,
}

#[derive(Properties, PartialEq)]
struct CardProps {
    pub item: Element,
    pub on_watch: Callback<Element>,
    pub on_edit: Callback<Element>,
    pub on_favorite_toggle: Callback<Element>,
    /// Whether the thumbnail worker is currently processing this video.
    #[prop_or_default]
    pub is_thumb_processing: bool,
    /// Whether the sprite worker is currently processing this video.
    #[prop_or_default]
    pub is_sprite_processing: bool,
    /// Whether the pre-cache worker is currently processing this video.
    #[prop_or_default]
    pub is_precache_processing: bool,
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
    let item_for_edit = item.clone();
    let item_for_fav = item.clone();
    let on_watch = props.on_watch.clone();
    let on_edit = props.on_edit.clone();
    let on_favorite_toggle = props.on_favorite_toggle.clone();

    // Per-card version that bumps only when THIS card's video transitions from
    // actively-processing to idle.  Passed to VideoCardThumb (thumbnail URL
    // cache-bust) and ProcessingStatus (status re-fetch), so neither component
    // reacts to processing events for other videos.
    let local_version = use_state(|| 0_u32);
    let prev_processing = use_mut_ref(|| false);

    {
        let local_version = local_version.clone();
        let prev_processing = prev_processing.clone();
        use_effect_with(
            (props.is_thumb_processing, props.is_sprite_processing, props.is_precache_processing),
            move |(is_thumb, is_sprite, is_precache)| {
                let is_now = *is_thumb || *is_sprite || *is_precache;
                let was = *prev_processing.borrow();
                if was && !is_now {
                    local_version.set(*local_version + 1);
                }
                *prev_processing.borrow_mut() = is_now;
                || ()
            },
        );
    }

    let is_fav = item.favorite;

    html! {
        <article class="card">
            <div class="card__thumb-wrap">
                <VideoCardThumb
                    video_id={item.id.clone()}
                    title={item.title.clone()}
                    processing_version={*local_version}
                />
                <button
                    type="button"
                    class={if is_fav { "card__fav card__fav--active" } else { "card__fav" }}
                    onclick={Callback::from(move |e: MouseEvent| {
                        e.stop_propagation();
                        on_favorite_toggle.emit(item_for_fav.clone());
                    })}
                    aria-label={if is_fav { "Remove from favorites" } else { "Add to favorites" }}
                    aria-pressed={is_fav.to_string()}
                >
                    <svg class="card__fav-icon" viewBox="0 0 24 24" xmlns="http://www.w3.org/2000/svg">
                        <path d="M12 21.35l-1.45-1.32C5.4 15.36 2 12.28 2 8.5 2 5.42 4.42 3 7.5 3c1.74 0 3.41.81 4.5 2.09C13.09 3.81 14.76 3 16.5 3 19.58 3 22 5.42 22 8.5c0 3.78-3.4 6.86-8.55 11.54L12 21.35z" />
                    </svg>
                </button>
            </div>

            <div class="card__top">
                <div class="card__title">{ item.title.clone() }</div>
                <ProcessingStatus
                    video_id={item.id.clone()}
                    is_thumb_processing={props.is_thumb_processing}
                    is_sprite_processing={props.is_sprite_processing}
                    is_precache_processing={props.is_precache_processing}
                    processing_version={*local_version}
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

            if !item.categories.is_empty() || !item.tags.is_empty() || !item.actors.is_empty() {
                <div class="card__tags">
                    { for item.categories.iter().map(|c| html! { <span class="badge">{ c.clone() }</span> }) }
                    { for item.tags.iter().map(|t| html! { <span class="tag">{ t.clone() }</span> }) }
                    { for item.actors.iter().map(|a| html! { <span class="tag">{ a.clone() }</span> }) }
                </div>
            }

            <div class="card__footer">
                <button
                    class="btn btn--edit"
                    type="button"
                    onclick={Callback::from(move |_| on_edit.emit(item_for_edit.clone()))}
                >
                    <svg class="btn__icon" viewBox="0 0 24 24" xmlns="http://www.w3.org/2000/svg">
                        <path d="M3 17.25V21h3.75L17.81 9.94l-3.75-3.75L3 17.25zM20.71 7.04a1 1 0 000-1.41l-2.34-2.34a1 1 0 00-1.41 0l-1.83 1.83 3.75 3.75 1.83-1.83z" fill="currentColor"/>
                    </svg>
                    { " Edit" }
                </button>
                <div class="card__footer-right">
                    if item.rating > 0.0 {
                        <div class="muted card__rating">
                            <svg class="card__rating-icon" viewBox="0 0 24 24" xmlns="http://www.w3.org/2000/svg">
                                <path d="M12 2l3.09 6.26L22 9.27l-5 4.87 1.18 6.88L12 17.77l-6.18 3.25L7 14.14 2 9.27l6.91-1.01L12 2z" fill="currentColor"/>
                            </svg>
                            { format!("{:.1}", item.rating) }
                        </div>
                    }
                    <button
                        class="btn btn--watch"
                        type="button"
                        onclick={Callback::from(move |_| on_watch.emit(item_clone.clone()))}
                    >
                        <svg class="btn__icon" viewBox="0 0 24 24" xmlns="http://www.w3.org/2000/svg">
                            <path d="M8 5v14l11-7z" fill="currentColor"/>
                        </svg>
                        { " Watch" }
                    </button>
                </div>
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
        <>
            // Top spacer maintains document height for items scrolled above the window.
            if props.top_pad > 0.0 {
                <div aria-hidden="true" style={format!("height:{}px", props.top_pad as u32)} />
            }
            <section class="grid" aria-label="Videos grid">
                { for props.items.iter().map(|item| {
                    let is_thumb_processing =
                        props.thumb_current_id.iter().any(|id| id == &item.id);
                    let is_sprite_processing =
                        props.sprite_current_id.iter().any(|id| id == &item.id);
                    let is_precache_processing =
                        props.precache_current_id.as_deref() == Some(item.id.as_str());

                    html! {
                        <VideoCard
                            key={item.id.clone()}
                            item={item.clone()}
                            on_watch={props.on_watch.clone()}
                            on_edit={props.on_edit.clone()}
                            on_favorite_toggle={props.on_favorite_toggle.clone()}
                            is_thumb_processing={is_thumb_processing}
                            is_sprite_processing={is_sprite_processing}
                            is_precache_processing={is_precache_processing}
                        />
                    }
                }) }
            </section>
            // Bottom spacer maintains document height for items below the window.
            if props.bottom_pad > 0.0 {
                <div aria-hidden="true" style={format!("height:{}px", props.bottom_pad as u32)} />
            }
        </>
    }
}
