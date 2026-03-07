use crate::models::Element;
use yew::prelude::*;

#[derive(Properties, PartialEq)]
pub struct Props {
    pub items: Vec<Element>,
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
            { for props.items.iter().map(|item| html! {
                <article class="card" key={item.id.clone()}>
                    <div class="card__top">
                        <div class="card__title">{ item.title.clone() }</div>
                        <div class="badge">{ item.genre.clone() }</div>
                    </div>
                    <div class="card__subtitle">{ item.description.clone() }</div>

                    <div class="card__meta">
                        <span class="muted">{ item.year }</span>
                        <span class="muted">{ "·" }</span>
                        <span class="muted">{ format_duration(item.duration_secs) }</span>
                        <span class="muted">{ "·" }</span>
                        <span class="muted">{ item.director.clone() }</span>
                    </div>

                    <div class="card__tags">
                        { for item.tags.iter().map(|t| html!{ <span class="tag" key={t.clone()}>{ t }</span> }) }
                    </div>

                    <div class="card__footer">
                        <div class="muted">{ format!("★ {:.1}", item.rating) }</div>
                        <button class="btn" type="button">{ "Watch" }</button>
                    </div>
                </article>
            }) }
        </section>
    }
}
