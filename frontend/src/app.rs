use crate::api;
use crate::components;

use components::{filters::FiltersBar, grid::ElementsGrid};
use crate::models::{Element, Filters, SortBy};

use wasm_bindgen_futures::spawn_local;
use yew::prelude::*;

#[function_component(App)]
pub fn app() -> Html {
    let query = use_state(|| "".to_string());
    let filters = use_state(Filters::default);
    let sort_by = use_state(|| SortBy::Relevance);

    let items = use_state(|| Vec::<Element>::new());
    let loading = use_state(|| false);
    let error = use_state(|| Option::<String>::None);

    // Fetch on load and whenever query/filters/sort changes
    {
        let query = query.clone();
        let filters = filters.clone();
        let sort_by = sort_by.clone();

        let items = items.clone();
        let loading = loading.clone();
        let error = error.clone();

        use_effect_with(((*query).clone(), (*filters).clone(), (*sort_by).clone()), move |_| {
            let query = (*query).clone();
            let filters = (*filters).clone();
            let sort_by = (*sort_by).clone();

            loading.set(true);
            error.set(None);

            spawn_local(async move {
                match api::fetch_elements(&query, &filters, sort_by).await {
                    Ok(data) => items.set(data),
                    Err(e) => error.set(Some(e)),
                }
                loading.set(false);
            });

            || ()
        });
    }

    let on_query_change = {
        let query = query.clone();
        Callback::from(move |v: String| query.set(v))
    };

    let on_filters_change = {
        let filters = filters.clone();
        Callback::from(move |v: Filters| filters.set(v))
    };

    let on_sort_change = {
        let sort_by = sort_by.clone();
        Callback::from(move |v: SortBy| sort_by.set(v))
    };

    html! {
        <div class="app">
            <header class="topbar">
                <div class="topbar__inner">
                    <div class="brand">
                        <div class="brand__logo">{ "Starfin" }</div>
                        <div class="brand__sub">{ "Your personal video library" }</div>
                    </div>

                    <FiltersBar
                        query={(*query).clone()}
                        filters={(*filters).clone()}
                        sort_by={(*sort_by).clone()}
                        on_query_change={on_query_change}
                        on_filters_change={on_filters_change}
                        on_sort_change={on_sort_change}
                    />
                </div>
            </header>

            <main class="content">
                if let Some(err) = &*error {
                    <div class="notice notice--error">
                        <div class="notice__title">{ "Failed to load" }</div>
                        <div class="notice__body">{ err }</div>
                    </div>
                }

                if *loading {
                    <div class="notice notice--loading">{ "Loading…" }</div>
                } else {
                    <ElementsGrid items={(*items).clone()} />
                }
            </main>
        </div>
    }
}
