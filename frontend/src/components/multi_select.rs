/// A custom multi-select dropdown component that shows selected values as
/// removable chips and opens a dropdown list when clicked.
///
/// # Behaviour
/// - Same height as a normal `<select>` when nothing is selected.
/// - Selected values appear as pills (chips) inside the trigger field.
///   Each chip has a ✕ button that removes the value.
/// - Clicking the trigger opens a dropdown list of all available options.
///   Already-selected options show a check mark.
/// - Clicking the backdrop (or re-clicking the trigger) closes the dropdown.
/// - If `values` is empty the field is disabled and shows `placeholder`.
use yew::prelude::*;

#[derive(Properties, PartialEq)]
pub struct Props {
    /// Complete list of available options.
    pub values: Vec<String>,
    /// Currently selected subset.
    pub selected: Vec<String>,
    /// Emitted whenever the selection changes.
    pub onchange: Callback<Vec<String>>,
    /// Shown inside the trigger when nothing is selected and values is empty.
    pub placeholder: &'static str,
    /// Label used for the aria-label attribute (accessibility).
    pub label: &'static str,
}

#[function_component(MultiSelect)]
pub fn multi_select(props: &Props) -> Html {
    let open = use_state(|| false);

    // Toggle dropdown open/closed when the trigger is clicked.
    let on_trigger = {
        let open = open.clone();
        Callback::from(move |e: MouseEvent| {
            e.stop_propagation();
            open.set(!*open);
        })
    };

    // Close the dropdown (called from backdrop click).
    let on_close = {
        let open = open.clone();
        Callback::from(move |e: MouseEvent| {
            e.stop_propagation();
            open.set(false);
        })
    };

    // Toggle an individual option in the selection list.
    let make_toggle = |value: String| {
        let cb = props.onchange.clone();
        let current = props.selected.clone();
        Callback::from(move |e: MouseEvent| {
            e.stop_propagation();
            let mut next = current.clone();
            if let Some(pos) = next.iter().position(|v| v == &value) {
                next.remove(pos);
            } else {
                next.push(value.clone());
            }
            cb.emit(next);
        })
    };

    // Remove a chip by value (fired from the × button inside a pill).
    let make_remove = |value: String| {
        let cb = props.onchange.clone();
        let current = props.selected.clone();
        Callback::from(move |e: MouseEvent| {
            e.stop_propagation();
            let next: Vec<String> = current.iter().filter(|v| v.as_str() != value).cloned().collect();
            cb.emit(next);
        })
    };

    let trigger_class = if *open {
        "multi-select__trigger multi-select__trigger--open"
    } else {
        "multi-select__trigger"
    };

    // Keyboard accessibility: toggle open on Enter/Space, close on Escape.
    let on_keydown = {
        let open = open.clone();
        Callback::from(move |e: KeyboardEvent| {
            match e.key().as_str() {
                "Enter" => {
                    e.prevent_default();
                    open.set(!*open);
                }
                "Escape" => {
                    open.set(false);
                }
                _ if e.code() == "Space" => {
                    e.prevent_default();
                    open.set(!*open);
                }
                _ => {}
            }
        })
    };

    html! {
        <div class="multi-select" aria-label={props.label}>
            // ── Invisible backdrop to catch outside clicks ────────────────────
            if *open {
                <div class="multi-select__backdrop" onclick={on_close} />
            }

            // ── Trigger: shows chips + placeholder + chevron ──────────────────
            <div
                class={trigger_class}
                onclick={on_trigger}
                onkeydown={on_keydown}
                tabindex="0"
                role="combobox"
                aria-expanded={open.to_string()}
                aria-haspopup="listbox"
            >
                if props.selected.is_empty() {
                    if props.values.is_empty() {
                        <span class="multi-select__placeholder">{ props.placeholder }</span>
                    } else {
                        <span class="multi-select__placeholder">{ "Any" }</span>
                    }
                } else {
                    <span class="multi-select__pills">
                        { for props.selected.iter().map(|v| {
                            let remove = make_remove(v.clone());
                            html! {
                                <span class="multi-select__pill" key={v.clone()}>
                                    { v }
                                    <button
                                        class="multi-select__pill-remove"
                                        type="button"
                                        aria-label={format!("Remove {v}")}
                                        onclick={remove}
                                    >
                                        // ✕ icon as inline SVG
                                        <svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 10 10" width="8" height="8" fill="currentColor">
                                            <path d="M1 1l8 8M9 1l-8 8" stroke="currentColor" stroke-width="1.5" stroke-linecap="round"/>
                                        </svg>
                                    </button>
                                </span>
                            }
                        })}
                    </span>
                }
                // Chevron
                <svg class="multi-select__chevron" aria-hidden="true" xmlns="http://www.w3.org/2000/svg" viewBox="0 0 10 6" width="10" height="6" fill="none">
                    <path d="M1 1l4 4 4-4" stroke="currentColor" stroke-width="1.5" stroke-linecap="round" stroke-linejoin="round"/>
                </svg>
            </div>

            // ── Dropdown options list ──────────────────────────────────────────
            if *open && !props.values.is_empty() {
                <ul class="multi-select__dropdown" role="listbox" aria-multiselectable="true">
                    { for props.values.iter().map(|v| {
                        let is_sel = props.selected.contains(v);
                        let toggle = make_toggle(v.clone());
                        let item_class = if is_sel {
                            "multi-select__option multi-select__option--selected"
                        } else {
                            "multi-select__option"
                        };
                        html! {
                            <li
                                class={item_class}
                                key={v.clone()}
                                role="option"
                                aria-selected={is_sel.to_string()}
                                onclick={toggle}
                            >
                                <span class="multi-select__option-check">
                                    if is_sel { { "✓" } }
                                </span>
                                { v }
                            </li>
                        }
                    })}
                </ul>
            }
        </div>
    }
}
