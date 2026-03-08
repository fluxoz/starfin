use crate::api;
use crate::components;

use components::{filters::FiltersBar, grid::ElementsGrid, video_player::VideoPlayer};
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
    let selected = use_state(|| Option::<Element>::None);
    
    // Dark mode state
    let dark_mode = use_state(|| false);

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

    let on_watch = {
        let selected = selected.clone();
        Callback::from(move |item: Element| selected.set(Some(item)))
    };

    let on_close_player = {
        let selected = selected.clone();
        Callback::from(move |_| selected.set(None))
    };
    
    let on_toggle_dark_mode = {
        let dark_mode = dark_mode.clone();
        Callback::from(move |_| dark_mode.set(!*dark_mode))
    };
    
    let app_class = if *dark_mode { "app dark-mode" } else { "app" };

    html! {
        <>
            <div class={app_class}>
                // Left sidebar with Starfin branding
                <aside class="sidebar">
                    <div class="sidebar__logo">{ "STARFIN" }</div>
                    <div class="sidebar__text">{ "MEDIA COLLECTION" }</div>
                    <div class="sidebar__arrow">{ "↗" }</div>
                </aside>

                // Video player overlay — rendered on top of the library when a video is selected.
                if let Some(video) = &*selected {
                    <VideoPlayer
                        video_id={video.id.clone()}
                        title={video.title.clone()}
                        on_close={on_close_player}
                    />
                }

                <header class="topbar">
                    <div class="topbar__inner">
                        <div class="topbar__left">{ "STARFIN MEDIA SERVER" }</div>
                        <div class="topbar__center">{ "PERSONAL VIDEO LIBRARY" }</div>
                        <div class="topbar__right">
                            <div class="theme-toggle" onclick={on_toggle_dark_mode.clone()}>
                                <span class={if *dark_mode { "theme-toggle__switch active" } else { "theme-toggle__switch" }}></span>
                                <span class="theme-toggle__label">{ "THEME" }</span>
                            </div>
                        </div>
                    </div>

                    <FiltersBar
                        query={(*query).clone()}
                        filters={(*filters).clone()}
                        sort_by={(*sort_by).clone()}
                        on_query_change={on_query_change}
                        on_filters_change={on_filters_change}
                        on_sort_change={on_sort_change}
                    />
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
                    <ElementsGrid items={(*items).clone()} on_watch={on_watch} />
                }
            </main>
            </div>
        </>
    }
}

