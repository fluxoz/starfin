use crate::models::Element;
use yew::prelude::*;

#[derive(Properties, PartialEq)]
pub struct Props {
    pub items: Vec<Element>,
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
        <section class="grid" aria-label="Elements grid">
            { for props.items.iter().map(|item| html! {
                <article class="card" key={item.id.clone()}>
                    <div class="card__top">
                        <div class="card__title">{ item.title.clone() }</div>
                        <div class="badge">{ item.category.clone() }</div>
                    </div>
                    <div class="card__subtitle">{ item.subtitle.clone() }</div>

                    <div class="card__tags">
                        { for item.tags.iter().map(|t| html!{ <span class="tag" key={t.clone()}>{ t }</span> }) }
                    </div>

                    <div class="card__footer">
                        <div class="muted">{ format!("Score: {:.1}", item.score) }</div>
                        <button class="btn" type="button">{ "Open" }</button>
                    </div>
                </article>
            }) }
        </section>
    }
}
