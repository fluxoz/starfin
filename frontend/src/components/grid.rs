use crate::components::video_card_thumb::VideoCardThumb;
use crate::models::Element;
use chrono::DateTime;
use yew::prelude::*;

#[derive(Properties, PartialEq)]
pub struct Props {
    pub items: Vec<Element>,
    pub on_watch: Callback<Element>,
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
    DateTime::from_timestamp(timestamp as i64, 0)
        .map(|dt| dt.format("%b %d, %Y").to_string())
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
                let item_clone = item.clone();
                let on_watch = props.on_watch.clone();

                html! {
                    <article class="card" key={item.id.clone()}>
                        <VideoCardThumb video_id={item.id.clone()} />

                        <div class="card__top">
                            <div class="card__title">{ item.title.clone() }</div>
                        </div>

                        <div class="card__meta">
                            if item.duration_secs > 0 {
                                <span class="card__meta-item card__meta-item--highlight">{ format_duration(item.duration_secs) }</span>
                            }
                            if item.year > 0 {
                                <span class="card__meta-item">{ item.year }</span>
                            }
                            if let Some(date_str) = format_date(item.date_added) {
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
            }) }
        </section>
    }
}
