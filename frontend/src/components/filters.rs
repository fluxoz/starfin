use crate::models::{Filters, SortBy};
use yew::prelude::*;

#[derive(Properties, PartialEq)]
pub struct Props {
    pub query: String,
    pub filters: Filters,
    pub sort_by: SortBy,
    pub on_query_change: Callback<String>,
    pub on_filters_change: Callback<Filters>,
    pub on_sort_change: Callback<SortBy>,
}

#[function_component(FiltersBar)]
pub fn filters_bar(props: &Props) -> Html {
    let on_search_input = {
        let cb = props.on_query_change.clone();
        Callback::from(move |e: InputEvent| {
            let input: web_sys::HtmlInputElement = e.target_unchecked_into();
            cb.emit(input.value());
        })
    };

    let on_genre_change = {
        let filters = props.filters.clone();
        let cb = props.on_filters_change.clone();
        Callback::from(move |e: Event| {
            let select: web_sys::HtmlSelectElement = e.target_unchecked_into();
            let v = select.value();
            let mut next = filters.clone();
            next.genre = if v.is_empty() { None } else { Some(v) };
            cb.emit(next);
        })
    };

    let on_favorites_toggle = {
        let filters = props.filters.clone();
        let cb = props.on_filters_change.clone();
        Callback::from(move |_| {
            let mut next = filters.clone();
            next.only_favorites = !next.only_favorites;
            cb.emit(next);
        })
    };

    let on_sort_change = {
        let cb = props.on_sort_change.clone();
        Callback::from(move |e: Event| {
            let select: web_sys::HtmlSelectElement = e.target_unchecked_into();
            let v = select.value();
            let s = match v.as_str() {
                "name_asc" => SortBy::NameAsc,
                "rating_desc" => SortBy::RatingDesc,
                _ => SortBy::Relevance,
            };
            cb.emit(s);
        })
    };

    html! {
        <section class="filters">
            <div class="filters__row">
                <label class="field field--search">
                    <span class="field__label">{ "Search" }</span>
                    <input
                        class="input"
                        type="search"
                        placeholder="Search title, director, genre, tags…"
                        value={props.query.clone()}
                        oninput={on_search_input}
                    />
                </label>

                <label class="field">
                    <span class="field__label">{ "Sort by" }</span>
                    <select class="select" onchange={on_sort_change} value={props.sort_by.as_str()}>
                        <option value="relevance">{ "Relevance" }</option>
                        <option value="name_asc">{ "Name (A → Z)" }</option>
                        <option value="rating_desc">{ "Rating (high → low)" }</option>
                    </select>
                </label>
            </div>

            <div class="filters__row filters__row--secondary">
                <label class="field">
                    <span class="field__label">{ "Genre" }</span>
                    <select class="select" onchange={on_genre_change} value={props.filters.genre.clone().unwrap_or_default()}>
                        <option value="">{ "All" }</option>
                        <option value="Action">{ "Action" }</option>
                        <option value="Crime">{ "Crime" }</option>
                        <option value="Drama">{ "Drama" }</option>
                        <option value="Sci-Fi">{ "Sci-Fi" }</option>
                    </select>
                </label>

                <button class="chip" type="button" onclick={on_favorites_toggle} aria-pressed={props.filters.only_favorites.to_string()}>
                {
                    if props.filters.only_favorites { "Favorites: On" } else { "Favorites: Off" }
                }
                </button>
            </div>
        </section>
    }
}
