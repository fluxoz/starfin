use crate::models::{MetadataFilter, SortBy};
use yew::prelude::*;

#[derive(Properties, PartialEq)]
pub struct Props {
    pub query: String,
    pub sort_by: SortBy,
    pub meta_filter: MetadataFilter,
    pub on_query_change: Callback<String>,
    pub on_sort_change: Callback<SortBy>,
    pub on_filter_change: Callback<MetadataFilter>,
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

    let on_sort_change = {
        let cb = props.on_sort_change.clone();
        Callback::from(move |e: Event| {
            let select: web_sys::HtmlSelectElement = e.target_unchecked_into();
            let v = select.value();
            let s = match v.as_str() {
                "date_oldest"     => SortBy::DateAddedOldest,
                "name_asc"        => SortBy::NameAsc,
                "name_desc"       => SortBy::NameDesc,
                "rating_highest"  => SortBy::RatingHighest,
                "favorites_first" => SortBy::FavoritesFirst,
                "year_newest"     => SortBy::YearNewest,
                "year_oldest"     => SortBy::YearOldest,
                _                 => SortBy::DateAddedNewest,
            };
            cb.emit(s);
        })
    };

    // ── Metadata filter callbacks ────────────────────────────────────────────

    let on_favorites_toggle = {
        let cb = props.on_filter_change.clone();
        let mf = props.meta_filter.clone();
        Callback::from(move |_: MouseEvent| {
            let mut updated = mf.clone();
            updated.only_favorites = !updated.only_favorites;
            cb.emit(updated);
        })
    };

    let on_min_rating_change = {
        let cb = props.on_filter_change.clone();
        let mf = props.meta_filter.clone();
        Callback::from(move |e: Event| {
            let select: web_sys::HtmlSelectElement = e.target_unchecked_into();
            let mut updated = mf.clone();
            updated.min_rating = select.value().parse::<u8>().unwrap_or(0);
            cb.emit(updated);
        })
    };

    let on_tag_input = {
        let cb = props.on_filter_change.clone();
        let mf = props.meta_filter.clone();
        Callback::from(move |e: InputEvent| {
            let input: web_sys::HtmlInputElement = e.target_unchecked_into();
            let mut updated = mf.clone();
            updated.tag = input.value();
            cb.emit(updated);
        })
    };

    let on_actor_input = {
        let cb = props.on_filter_change.clone();
        let mf = props.meta_filter.clone();
        Callback::from(move |e: InputEvent| {
            let input: web_sys::HtmlInputElement = e.target_unchecked_into();
            let mut updated = mf.clone();
            updated.actor = input.value();
            cb.emit(updated);
        })
    };

    let on_category_input = {
        let cb = props.on_filter_change.clone();
        let mf = props.meta_filter.clone();
        Callback::from(move |e: InputEvent| {
            let input: web_sys::HtmlInputElement = e.target_unchecked_into();
            let mut updated = mf.clone();
            updated.category = input.value();
            cb.emit(updated);
        })
    };

    let fav_pressed = props.meta_filter.only_favorites.to_string();
    let min_rating_val = props.meta_filter.min_rating.to_string();

    html! {
        <section class="filters">
            // ── Row 1: search + sort ──────────────────────────────────────────
            <div class="filters__row">
                <label class="field field--search">
                    <span class="field__label">{ "Search" }</span>
                    <input
                        class="input"
                        type="search"
                        placeholder="Search title, director, genre, tags, actors…"
                        value={props.query.clone()}
                        oninput={on_search_input}
                    />
                </label>

                <label class="field">
                    <span class="field__label">{ "Sort by" }</span>
                    <select class="select" onchange={on_sort_change} value={props.sort_by.as_str()}>
                        <option value="date_newest">{ "Date Added (Newest)" }</option>
                        <option value="date_oldest">{ "Date Added (Oldest)" }</option>
                        <option value="name_asc">{ "Name (A → Z)" }</option>
                        <option value="name_desc">{ "Name (Z → A)" }</option>
                        <option value="rating_highest">{ "Rating (Highest)" }</option>
                        <option value="favorites_first">{ "Favorites First" }</option>
                        <option value="year_newest">{ "Year (Newest)" }</option>
                        <option value="year_oldest">{ "Year (Oldest)" }</option>
                    </select>
                </label>
            </div>

            // ── Row 2: metadata filters ───────────────────────────────────────
            <div class="filters__row filters__row--meta">
                <button
                    class="chip"
                    aria-pressed={fav_pressed}
                    onclick={on_favorites_toggle}
                    title="Show favorites only"
                >
                    { "★ Favorites" }
                </button>

                <label class="field">
                    <span class="field__label">{ "Min Rating" }</span>
                    <select class="select" onchange={on_min_rating_change} value={min_rating_val}>
                        <option value="0">{ "Any" }</option>
                        <option value="1">{ "★ 1+" }</option>
                        <option value="2">{ "★★ 2+" }</option>
                        <option value="3">{ "★★★ 3+" }</option>
                        <option value="4">{ "★★★★ 4+" }</option>
                        <option value="5">{ "★★★★★ 5" }</option>
                    </select>
                </label>

                <label class="field">
                    <span class="field__label">{ "Tag" }</span>
                    <input
                        class="input"
                        type="search"
                        placeholder="Filter by tag…"
                        value={props.meta_filter.tag.clone()}
                        oninput={on_tag_input}
                    />
                </label>

                <label class="field">
                    <span class="field__label">{ "Actor" }</span>
                    <input
                        class="input"
                        type="search"
                        placeholder="Filter by actor…"
                        value={props.meta_filter.actor.clone()}
                        oninput={on_actor_input}
                    />
                </label>

                <label class="field">
                    <span class="field__label">{ "Category" }</span>
                    <input
                        class="input"
                        type="search"
                        placeholder="Filter by category…"
                        value={props.meta_filter.category.clone()}
                        oninput={on_category_input}
                    />
                </label>
            </div>
        </section>
    }
}
