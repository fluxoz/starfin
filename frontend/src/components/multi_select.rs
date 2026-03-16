/// A custom multi-select dropdown component.
///
/// # Layout
/// ```text
/// [tag-a ×] [tag-b ×]          ← removable pill row (only shown when selections exist)
/// ┌─ Choose… ──────────────── ▾ ┐  ← trigger — constant height, opens dropdown on click
/// └─────────────────────────────┘
///   ○ option-a
///   ● option-b  (checked)
///   ○ option-c
/// ```
///
/// Selected values appear as pill buttons **above** the trigger.  Clicking a
/// pill's × button removes it.  The trigger always shows placeholder text so
/// its height never changes.  Clicking outside (or pressing Escape) closes the
/// dropdown.
use yew::prelude::*;

#[derive(Properties, PartialEq)]
pub struct Props {
    /// Complete list of available options.
    pub values: Vec<String>,
    /// Currently selected subset.
    pub selected: Vec<String>,
    /// Emitted whenever the selection changes.
    pub onchange: Callback<Vec<String>>,
    /// Shown inside the trigger when `values` is empty.
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

    // Toggle an individual option in the dropdown list.
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

    // Remove a specific pill (fired from the × button).
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

    // Placeholder text shown inside the trigger at all times.
    let trigger_label = if props.values.is_empty() {
        props.placeholder
    } else {
        "Any"
    };

    html! {
        <div class="multi-select" aria-label={props.label}>
            // ── Invisible backdrop to catch outside clicks ─────────────────────
            if *open {
                <div class="multi-select__backdrop" onclick={on_close} />
            }

            // ── Selected pills row (rendered ABOVE the trigger) ────────────────
            if !props.selected.is_empty() {
                <div class="multi-select__pills">
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
                                    <svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 10 10" width="8" height="8" aria-hidden="true">
                                        <path d="M1 1l8 8M9 1l-8 8" stroke="currentColor" stroke-width="1.5" stroke-linecap="round"/>
                                    </svg>
                                </button>
                            </span>
                        }
                    })}
                </div>
            }

            // ── Trigger — constant height, just placeholder + chevron ──────────
            <div
                class={trigger_class}
                onclick={on_trigger}
                onkeydown={on_keydown}
                tabindex="0"
                role="combobox"
                aria-expanded={open.to_string()}
                aria-haspopup="listbox"
            >
                <span class="multi-select__placeholder">{ trigger_label }</span>
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
                                <span class="multi-select__option-check" aria-hidden="true">
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
