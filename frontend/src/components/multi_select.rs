/// A custom multi-select dropdown component.
///
/// # Layout
/// ```text
/// [tag-a ×] [tag-b ×]          ← removable pill row (only when selections exist)
/// ┌─ Any ─────────────────── ▾ ┐  ← trigger — constant height
/// └─────────────────────────────┘
///   ○ option-a
///   ● option-b  (checked)
///   ○ option-c
/// ```
///
/// Selected values appear as pill buttons **above** the trigger.
/// Clicking the trigger opens a dropdown list. Clicking anywhere outside the
/// component (or pressing Escape) closes it via a window-level click listener.
/// All internal click handlers call `stop_propagation()` so the window
/// listener does not fire while the user interacts with the dropdown.
use wasm_bindgen::prelude::*;
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

    // Holds the active window click listener so it can be removed on cleanup.
    let listener: std::rc::Rc<std::cell::RefCell<Option<Closure<dyn Fn(web_sys::MouseEvent)>>>> =
        use_mut_ref(|| None);

    // Add a window-level click listener when the dropdown is open so that
    // clicking anywhere outside the component closes it.  All internal click
    // handlers call stop_propagation() to prevent this listener from firing
    // when the user interacts with the dropdown itself.
    {
        let open_hdl = open.clone();
        let listener = listener.clone();

        use_effect_with(*open, move |&is_open| {
            let win = web_sys::window().expect("window not available");

            // Remove any previously-registered listener first.
            if let Some(ref cb) = *listener.borrow() {
                let _ = win.remove_event_listener_with_callback(
                    "click",
                    cb.as_ref().unchecked_ref(),
                );
            }
            *listener.borrow_mut() = None;

            if is_open {
                let open_close = open_hdl.clone();
                let cb = Closure::wrap(Box::new(move |_e: web_sys::MouseEvent| {
                    open_close.set(false);
                }) as Box<dyn Fn(web_sys::MouseEvent)>);

                let _ = win
                    .add_event_listener_with_callback("click", cb.as_ref().unchecked_ref());
                *listener.borrow_mut() = Some(cb);
            }

            // Cleanup: remove the listener when deps change or component unmounts.
            let listener_cleanup = listener.clone();
            move || {
                if let Some(ref cb) = *listener_cleanup.borrow() {
                    if let Some(win) = web_sys::window() {
                        let _ = win.remove_event_listener_with_callback(
                            "click",
                            cb.as_ref().unchecked_ref(),
                        );
                    }
                }
                *listener_cleanup.borrow_mut() = None;
            }
        });
    }

    // Toggle dropdown open/closed when the trigger is clicked.
    let on_trigger = {
        let open = open.clone();
        Callback::from(move |e: MouseEvent| {
            e.stop_propagation(); // prevents window listener from closing dropdown
            open.set(!*open);
        })
    };

    // Toggle an individual option in the dropdown list.
    let make_toggle = |value: String| {
        let cb = props.onchange.clone();
        let current = props.selected.clone();
        Callback::from(move |e: MouseEvent| {
            e.stop_propagation(); // prevents window listener from closing dropdown
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
            e.stop_propagation(); // prevents window listener from closing dropdown
            let next: Vec<String> =
                current.iter().filter(|v| v.as_str() != value).cloned().collect();
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

    // Stop propagation on the container so clicks inside never reach the
    // window listener.  This is a belt-and-suspenders guard on top of each
    // individual handler also calling stop_propagation().
    let on_container_click = Callback::from(|e: MouseEvent| {
        e.stop_propagation();
    });

    let trigger_label = if props.values.is_empty() {
        props.placeholder
    } else {
        "Any"
    };

    html! {
        <div class="multi-select" aria-label={props.label} onclick={on_container_click}>

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
                                    <svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 10 10"
                                        width="8" height="8" aria-hidden="true">
                                        <path d="M1 1l8 8M9 1l-8 8" stroke="currentColor"
                                            stroke-width="1.5" stroke-linecap="round"/>
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
                <svg class="multi-select__chevron" aria-hidden="true"
                    xmlns="http://www.w3.org/2000/svg" viewBox="0 0 10 6"
                    width="10" height="6" fill="none">
                    <path d="M1 1l4 4 4-4" stroke="currentColor"
                        stroke-width="1.5" stroke-linecap="round" stroke-linejoin="round"/>
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
