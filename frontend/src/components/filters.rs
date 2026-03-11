use crate::models::SortBy;
use yew::prelude::*;

#[derive(Properties, PartialEq)]
pub struct Props {
    pub query: String,
    pub sort_by: SortBy,
    pub on_query_change: Callback<String>,
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

    let on_sort_change = {
        let cb = props.on_sort_change.clone();
        Callback::from(move |e: Event| {
            let select: web_sys::HtmlSelectElement = e.target_unchecked_into();
            let v = select.value();
            let s = match v.as_str() {
                "date_oldest" => SortBy::DateAddedOldest,
                "name_asc" => SortBy::NameAsc,
                _ => SortBy::DateAddedNewest,
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
                        <option value="date_newest">{ "Date Added (Newest)" }</option>
                        <option value="date_oldest">{ "Date Added (Oldest)" }</option>
                        <option value="name_asc">{ "Name (A → Z)" }</option>
                    </select>
                </label>
            </div>
        </section>
    }
}
