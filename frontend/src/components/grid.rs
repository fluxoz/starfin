use crate::components::video_card_thumb::VideoCardThumb;
use crate::models::Element;
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
                            <div class="badge">{ item.genre.clone() }</div>
                        </div>
                        <div class="card__subtitle">{ item.description.clone() }</div>

                        <div class="card__meta">
                            if item.year > 0 {
                                <span class="muted">{ item.year }</span>
                                <span class="muted">{ "·" }</span>
                            }
                            if item.duration_secs > 0 {
                                <span class="muted">{ format_duration(item.duration_secs) }</span>
                                <span class="muted">{ "·" }</span>
                            }
                            if !item.director.is_empty() {
                                <span class="muted">{ item.director.clone() }</span>
                            }
                        </div>

                        <div class="card__tags">
                            { for item.tags.iter().map(|t| html!{ <span class="tag" key={t.clone()}>{ t }</span> }) }
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
