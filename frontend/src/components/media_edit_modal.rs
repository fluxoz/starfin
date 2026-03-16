use crate::api;
use crate::models::Element;
use wasm_bindgen_futures::spawn_local;
use web_sys::HtmlInputElement;
use yew::prelude::*;

#[derive(Properties, PartialEq)]
pub struct MediaEditModalProps {
    pub item: Element,
    pub on_close: Callback<()>,
    pub on_saved: Callback<Element>,
}

#[function_component(MediaEditModal)]
pub fn media_edit_modal(props: &MediaEditModalProps) -> Html {
    let item = &props.item;

    let favorite = use_state(|| item.favorite);
    let tags = use_state(|| item.tags.clone());
    let actors = use_state(|| item.actors.clone());
    let categories = use_state(|| item.categories.clone());

    let tag_input = use_state(String::new);
    let actor_input = use_state(String::new);
    let category_input = use_state(String::new);

    let saving = use_state(|| false);
    let error = use_state(|| Option::<String>::None);

    // ── Favorite toggle ──────────────────────────────────────────────────────
    let on_toggle_favorite = {
        let favorite = favorite.clone();
        Callback::from(move |_: MouseEvent| {
            favorite.set(!*favorite);
        })
    };

    // ── Tag management ───────────────────────────────────────────────────────
    let on_tag_input = {
        let tag_input = tag_input.clone();
        Callback::from(move |e: InputEvent| {
            if let Some(input) = e.target_dyn_into::<HtmlInputElement>() {
                tag_input.set(input.value());
            }
        })
    };
    let on_tag_keydown = {
        let tag_input = tag_input.clone();
        let tags = tags.clone();
        Callback::from(move |e: KeyboardEvent| {
            if e.key() == "Enter" {
                e.prevent_default();
                let val = (*tag_input).trim().to_string();
                if !val.is_empty() && !tags.contains(&val) {
                    let mut t = (*tags).clone();
                    t.push(val);
                    tags.set(t);
                }
                tag_input.set(String::new());
            }
        })
    };
    let on_add_tag = {
        let tag_input = tag_input.clone();
        let tags = tags.clone();
        Callback::from(move |_: MouseEvent| {
            let val = (*tag_input).trim().to_string();
            if !val.is_empty() && !tags.contains(&val) {
                let mut t = (*tags).clone();
                t.push(val);
                tags.set(t);
            }
            tag_input.set(String::new());
        })
    };

    // ── Actor management ─────────────────────────────────────────────────────
    let on_actor_input = {
        let actor_input = actor_input.clone();
        Callback::from(move |e: InputEvent| {
            if let Some(input) = e.target_dyn_into::<HtmlInputElement>() {
                actor_input.set(input.value());
            }
        })
    };
    let on_actor_keydown = {
        let actor_input = actor_input.clone();
        let actors = actors.clone();
        Callback::from(move |e: KeyboardEvent| {
            if e.key() == "Enter" {
                e.prevent_default();
                let val = (*actor_input).trim().to_string();
                if !val.is_empty() && !actors.contains(&val) {
                    let mut a = (*actors).clone();
                    a.push(val);
                    actors.set(a);
                }
                actor_input.set(String::new());
            }
        })
    };
    let on_add_actor = {
        let actor_input = actor_input.clone();
        let actors = actors.clone();
        Callback::from(move |_: MouseEvent| {
            let val = (*actor_input).trim().to_string();
            if !val.is_empty() && !actors.contains(&val) {
                let mut a = (*actors).clone();
                a.push(val);
                actors.set(a);
            }
            actor_input.set(String::new());
        })
    };

    // ── Category management ──────────────────────────────────────────────────
    let on_category_input = {
        let category_input = category_input.clone();
        Callback::from(move |e: InputEvent| {
            if let Some(input) = e.target_dyn_into::<HtmlInputElement>() {
                category_input.set(input.value());
            }
        })
    };
    let on_category_keydown = {
        let category_input = category_input.clone();
        let categories = categories.clone();
        Callback::from(move |e: KeyboardEvent| {
            if e.key() == "Enter" {
                e.prevent_default();
                let val = (*category_input).trim().to_string();
                if !val.is_empty() && !categories.contains(&val) {
                    let mut c = (*categories).clone();
                    c.push(val);
                    categories.set(c);
                }
                category_input.set(String::new());
            }
        })
    };
    let on_add_category = {
        let category_input = category_input.clone();
        let categories = categories.clone();
        Callback::from(move |_: MouseEvent| {
            let val = (*category_input).trim().to_string();
            if !val.is_empty() && !categories.contains(&val) {
                let mut c = (*categories).clone();
                c.push(val);
                categories.set(c);
            }
            category_input.set(String::new());
        })
    };

    // ── Save ─────────────────────────────────────────────────────────────────
    let on_save = {
        let video_id = item.id.clone();
        let favorite = favorite.clone();
        let tags = tags.clone();
        let actors = actors.clone();
        let categories = categories.clone();
        let saving = saving.clone();
        let error = error.clone();
        let on_saved = props.on_saved.clone();
        Callback::from(move |e: SubmitEvent| {
            e.prevent_default();
            let video_id = video_id.clone();
            let fav = *favorite;
            let t = (*tags).clone();
            let a = (*actors).clone();
            let c = (*categories).clone();
            let saving = saving.clone();
            let error = error.clone();
            let on_saved = on_saved.clone();

            saving.set(true);
            error.set(None);

            spawn_local(async move {
                match api::update_metadata(
                    &video_id,
                    Some(fav),
                    Some(t),
                    Some(a),
                    Some(c),
                ).await {
                    Ok(updated) => {
                        saving.set(false);
                        on_saved.emit(updated);
                    }
                    Err(e) => {
                        error.set(Some(e));
                        saving.set(false);
                    }
                }
            });
        })
    };

    let on_close = props.on_close.clone();

    // ── Backdrop click closes modal ──────────────────────────────────────────
    let on_backdrop_click = {
        let on_close = on_close.clone();
        Callback::from(move |e: MouseEvent| {
            // Only close if clicking the backdrop itself, not the modal content.
            if let Some(target) = e.target_dyn_into::<web_sys::Element>() {
                let class = target.get_attribute("class").unwrap_or_default();
                if class.contains("meta-backdrop") {
                    on_close.emit(());
                }
            }
        })
    };

    // Removal callbacks for chips
    let remove_tag = {
        let tags = tags.clone();
        Callback::from(move |idx: usize| {
            let mut t = (*tags).clone();
            t.remove(idx);
            tags.set(t);
        })
    };

    let remove_actor = {
        let actors = actors.clone();
        Callback::from(move |idx: usize| {
            let mut a = (*actors).clone();
            a.remove(idx);
            actors.set(a);
        })
    };

    let remove_category = {
        let categories = categories.clone();
        Callback::from(move |idx: usize| {
            let mut c = (*categories).clone();
            c.remove(idx);
            categories.set(c);
        })
    };

    html! {
        <div class="meta-backdrop" onclick={on_backdrop_click}>
            <div class="meta-modal">
                <div class="meta-modal__header">
                    <div class="meta-modal__title">{ "EDIT METADATA" }</div>
                    <button
                        type="button"
                        class="meta-modal__close"
                        onclick={Callback::from(move |_| on_close.emit(()))}
                        aria-label="Close"
                    >
                        <svg class="meta-modal__close-icon" viewBox="0 0 24 24" xmlns="http://www.w3.org/2000/svg">
                            <line x1="6" y1="6" x2="18" y2="18" stroke="currentColor" stroke-width="2" stroke-linecap="round"/>
                            <line x1="18" y1="6" x2="6" y2="18" stroke="currentColor" stroke-width="2" stroke-linecap="round"/>
                        </svg>
                    </button>
                </div>

                <div class="meta-modal__media-title">{ item.title.clone() }</div>

                <form class="meta-modal__form" onsubmit={on_save}>
                    // ── Favorite toggle ──────────────────────────────────
                    <div class="meta-field">
                        <label class="meta-field__label">{ "FAVORITE" }</label>
                        <button
                            type="button"
                            class={if *favorite { "meta-fav-btn meta-fav-btn--active" } else { "meta-fav-btn" }}
                            onclick={on_toggle_favorite}
                            aria-label={if *favorite { "Remove from favorites" } else { "Add to favorites" }}
                            aria-pressed={favorite.to_string()}
                        >
                            <svg class="meta-fav-btn__icon" viewBox="0 0 24 24" xmlns="http://www.w3.org/2000/svg">
                                <path d="M12 21.35l-1.45-1.32C5.4 15.36 2 12.28 2 8.5 2 5.42 4.42 3 7.5 3c1.74 0 3.41.81 4.5 2.09C13.09 3.81 14.76 3 16.5 3 19.58 3 22 5.42 22 8.5c0 3.78-3.4 6.86-8.55 11.54L12 21.35z" />
                            </svg>
                            { if *favorite { "FAVORITED" } else { "NOT FAVORITED" } }
                        </button>
                    </div>

                    // ── Tags ─────────────────────────────────────────────
                    <div class="meta-field">
                        <label class="meta-field__label">{ "TAGS" }</label>
                        <div class="meta-chips">
                            { for (*tags).iter().enumerate().map(|(idx, t)| {
                                let remove = remove_tag.clone();
                                html! {
                                    <span class="meta-chip">
                                        { t.clone() }
                                        <button
                                            type="button"
                                            class="meta-chip__remove"
                                            onclick={Callback::from(move |_| remove.emit(idx))}
                                            aria-label={format!("Remove tag {}", t)}
                                        >
                                            <svg class="meta-chip__remove-icon" viewBox="0 0 24 24" xmlns="http://www.w3.org/2000/svg">
                                                <line x1="6" y1="6" x2="18" y2="18" stroke="currentColor" stroke-width="2.5" stroke-linecap="round"/>
                                                <line x1="18" y1="6" x2="6" y2="18" stroke="currentColor" stroke-width="2.5" stroke-linecap="round"/>
                                            </svg>
                                        </button>
                                    </span>
                                }
                            }) }
                        </div>
                        <div class="meta-field__row">
                            <input
                                type="text"
                                class="meta-field__input"
                                placeholder="Add a tag…"
                                value={(*tag_input).clone()}
                                oninput={on_tag_input}
                                onkeydown={on_tag_keydown}
                            />
                            <button type="button" class="meta-field__add" onclick={on_add_tag}>
                                <svg class="meta-field__add-icon" viewBox="0 0 24 24" xmlns="http://www.w3.org/2000/svg">
                                    <line x1="12" y1="5" x2="12" y2="19" stroke="currentColor" stroke-width="2.5" stroke-linecap="round"/>
                                    <line x1="5" y1="12" x2="19" y2="12" stroke="currentColor" stroke-width="2.5" stroke-linecap="round"/>
                                </svg>
                            </button>
                        </div>
                    </div>

                    // ── Actors / People ──────────────────────────────────
                    <div class="meta-field">
                        <label class="meta-field__label">{ "ACTORS / PEOPLE" }</label>
                        <div class="meta-chips">
                            { for (*actors).iter().enumerate().map(|(idx, a)| {
                                let remove = remove_actor.clone();
                                html! {
                                    <span class="meta-chip">
                                        { a.clone() }
                                        <button
                                            type="button"
                                            class="meta-chip__remove"
                                            onclick={Callback::from(move |_| remove.emit(idx))}
                                            aria-label={format!("Remove actor {}", a)}
                                        >
                                            <svg class="meta-chip__remove-icon" viewBox="0 0 24 24" xmlns="http://www.w3.org/2000/svg">
                                                <line x1="6" y1="6" x2="18" y2="18" stroke="currentColor" stroke-width="2.5" stroke-linecap="round"/>
                                                <line x1="18" y1="6" x2="6" y2="18" stroke="currentColor" stroke-width="2.5" stroke-linecap="round"/>
                                            </svg>
                                        </button>
                                    </span>
                                }
                            }) }
                        </div>
                        <div class="meta-field__row">
                            <input
                                type="text"
                                class="meta-field__input"
                                placeholder="Add a name…"
                                value={(*actor_input).clone()}
                                oninput={on_actor_input}
                                onkeydown={on_actor_keydown}
                            />
                            <button type="button" class="meta-field__add" onclick={on_add_actor}>
                                <svg class="meta-field__add-icon" viewBox="0 0 24 24" xmlns="http://www.w3.org/2000/svg">
                                    <line x1="12" y1="5" x2="12" y2="19" stroke="currentColor" stroke-width="2.5" stroke-linecap="round"/>
                                    <line x1="5" y1="12" x2="19" y2="12" stroke="currentColor" stroke-width="2.5" stroke-linecap="round"/>
                                </svg>
                            </button>
                        </div>
                    </div>

                    // ── Categories / Genres ──────────────────────────────
                    <div class="meta-field">
                        <label class="meta-field__label">{ "CATEGORIES" }</label>
                        <div class="meta-chips">
                            { for (*categories).iter().enumerate().map(|(idx, c)| {
                                let remove = remove_category.clone();
                                html! {
                                    <span class="meta-chip">
                                        { c.clone() }
                                        <button
                                            type="button"
                                            class="meta-chip__remove"
                                            onclick={Callback::from(move |_| remove.emit(idx))}
                                            aria-label={format!("Remove category {}", c)}
                                        >
                                            <svg class="meta-chip__remove-icon" viewBox="0 0 24 24" xmlns="http://www.w3.org/2000/svg">
                                                <line x1="6" y1="6" x2="18" y2="18" stroke="currentColor" stroke-width="2.5" stroke-linecap="round"/>
                                                <line x1="18" y1="6" x2="6" y2="18" stroke="currentColor" stroke-width="2.5" stroke-linecap="round"/>
                                            </svg>
                                        </button>
                                    </span>
                                }
                            }) }
                        </div>
                        <div class="meta-field__row">
                            <input
                                type="text"
                                class="meta-field__input"
                                placeholder="Add a category…"
                                value={(*category_input).clone()}
                                oninput={on_category_input}
                                onkeydown={on_category_keydown}
                            />
                            <button type="button" class="meta-field__add" onclick={on_add_category}>
                                <svg class="meta-field__add-icon" viewBox="0 0 24 24" xmlns="http://www.w3.org/2000/svg">
                                    <line x1="12" y1="5" x2="12" y2="19" stroke="currentColor" stroke-width="2.5" stroke-linecap="round"/>
                                    <line x1="5" y1="12" x2="19" y2="12" stroke="currentColor" stroke-width="2.5" stroke-linecap="round"/>
                                </svg>
                            </button>
                        </div>
                    </div>

                    if let Some(err) = &*error {
                        <div class="meta-modal__error">{ err }</div>
                    }

                    <button
                        type="submit"
                        class="meta-modal__save"
                        disabled={*saving}
                    >
                        { if *saving { "SAVING…" } else { "SAVE CHANGES" } }
                    </button>
                </form>
            </div>
        </div>
    }
}
