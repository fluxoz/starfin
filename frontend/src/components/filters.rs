use crate::models::{MetadataFilter, SortBy};
use wasm_bindgen::JsCast;
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

/// Read the currently selected values from a `<select multiple>` element.
fn selected_options(select: &web_sys::HtmlSelectElement) -> Vec<String> {
    let opts = select.selected_options();
    let mut selected = Vec::new();
    for i in 0..opts.length() {
        if let Some(item) = opts.item(i) {
            if let Ok(opt) = item.dyn_into::<web_sys::HtmlOptionElement>() {
                selected.push(opt.value());
            }
        }
    }
    selected
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
        Callback::from(move |e: Event| {
            let select: web_sys::HtmlSelectElement = e.target_unchecked_into();
            let mut updated = mf.clone();
            updated.tag = selected_options(&select);
            cb.emit(updated);
        })
    };

    let on_actor_change = {
        let cb = props.on_filter_change.clone();
        let mf = props.meta_filter.clone();
        Callback::from(move |e: Event| {
            let select: web_sys::HtmlSelectElement = e.target_unchecked_into();
            let mut updated = mf.clone();
            updated.actor = selected_options(&select);
            cb.emit(updated);
        })
    };

    let on_category_change = {
        let cb = props.on_filter_change.clone();
        let mf = props.meta_filter.clone();
        Callback::from(move |e: Event| {
            let select: web_sys::HtmlSelectElement = e.target_unchecked_into();
            let mut updated = mf.clone();
            updated.category = selected_options(&select);
            cb.emit(updated);
        })
    };

    let fav_pressed = props.meta_filter.only_favorites.to_string();
    let min_rating_val = props.meta_filter.min_rating.to_string();

    // Render a <select multiple> for a list of values.  Options that are
    // currently selected in `active` are pre-marked as selected.
    let render_multi = |values: &[String],
                        active: &[String],
                        onchange: Callback<Event>,
                        placeholder: &'static str|
     -> Html {
        if values.is_empty() {
            return html! {
                <select class="select select--multi" disabled={true} onchange={onchange}>
                    <option value="">{ placeholder }</option>
                </select>
            };
        }
        let options = values.iter().map(|v| {
            let sel = active.contains(v);
            html! { <option value={v.clone()} selected={sel}>{ v }</option> }
        });
        html! {
            <select class="select select--multi" multiple={true} onchange={onchange}>
                { for options }
            </select>
        }
    };

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
                    <span class="field__label">
                        { "Tags" }
                        if !props.meta_filter.tag.is_empty() {
                            <span class="filter-badge">{ props.meta_filter.tag.len() }</span>
                        }
                    </span>
                    { render_multi(&props.all_tags, &props.meta_filter.tag, on_tag_change, "No tags defined") }
                </label>

                <label class="field">
                    <span class="field__label">
                        { "Actors" }
                        if !props.meta_filter.actor.is_empty() {
                            <span class="filter-badge">{ props.meta_filter.actor.len() }</span>
                        }
                    </span>
                    { render_multi(&props.all_actors, &props.meta_filter.actor, on_actor_change, "No actors defined") }
                </label>

                <label class="field">
                    <span class="field__label">
                        { "Categories" }
                        if !props.meta_filter.category.is_empty() {
                            <span class="filter-badge">{ props.meta_filter.category.len() }</span>
                        }
                    </span>
                    { render_multi(&props.all_categories, &props.meta_filter.category, on_category_change, "No categories defined") }
                </label>
            </div>
        </section>
    }
}
