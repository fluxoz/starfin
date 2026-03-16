use crate::components::multi_select::MultiSelect;
use crate::models::{MetadataFilter, SortBy};
use yew::prelude::*;

#[derive(Properties, PartialEq)]
pub struct Props {
    pub query: String,
    pub sort_by: SortBy,
    pub meta_filter: MetadataFilter,
    /// All unique tag values present in the library.
    pub all_tags: Vec<String>,
    /// All unique actor values present in the library.
    pub all_actors: Vec<String>,
    /// All unique category values present in the library.
    pub all_categories: Vec<String>,
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

    let on_tag_change = {
        let cb = props.on_filter_change.clone();
        let mf = props.meta_filter.clone();
        Callback::from(move |vals: Vec<String>| {
            let mut updated = mf.clone();
            updated.tag = vals;
            cb.emit(updated);
        })
    };

    let on_actor_change = {
        let cb = props.on_filter_change.clone();
        let mf = props.meta_filter.clone();
        Callback::from(move |vals: Vec<String>| {
            let mut updated = mf.clone();
            updated.actor = vals;
            cb.emit(updated);
        })
    };

    let on_category_change = {
        let cb = props.on_filter_change.clone();
        let mf = props.meta_filter.clone();
        Callback::from(move |vals: Vec<String>| {
            let mut updated = mf.clone();
            updated.category = vals;
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
                    <svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 16 15"
                        width="12" height="12" fill="currentColor" aria-hidden="true"
                        style="vertical-align: -1px; margin-right: 4px;">
                        <path d="M8 0l2.2 4.6 5 .7-3.6 3.5.9 5L8 11.5l-4.5 2.3.9-5L.8 5.3l5-.7z"/>
                    </svg>
                    { "Favorites" }
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

                <div class="field">
                    <span class="field__label">{ "Tags" }</span>
                    <MultiSelect
                        values={props.all_tags.clone()}
                        selected={props.meta_filter.tag.clone()}
                        onchange={on_tag_change}
                        placeholder="No tags defined"
                        label="Filter by tags"
                    />
                </div>

                <div class="field">
                    <span class="field__label">{ "Actors" }</span>
                    <MultiSelect
                        values={props.all_actors.clone()}
                        selected={props.meta_filter.actor.clone()}
                        onchange={on_actor_change}
                        placeholder="No actors defined"
                        label="Filter by actors"
                    />
                </div>

                <div class="field">
                    <span class="field__label">{ "Categories" }</span>
                    <MultiSelect
                        values={props.all_categories.clone()}
                        selected={props.meta_filter.category.clone()}
                        onchange={on_category_change}
                        placeholder="No categories defined"
                        label="Filter by categories"
                    />
                </div>
            </div>
        </section>
    }
}

