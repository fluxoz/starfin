use wasm_bindgen_futures::spawn_local;
use web_sys::HtmlInputElement;
use yew::prelude::*;

use crate::api;

/// Props for the password modal.
#[derive(Properties, PartialEq)]
pub struct PasswordModalProps {
    /// Whether the password has already been set on the server.
    pub password_set: bool,
    /// Called after successful authentication.
    pub on_authenticated: Callback<()>,
}

#[function_component(PasswordModal)]
pub fn password_modal(props: &PasswordModalProps) -> Html {
    let password = use_state(|| String::new());
    let confirm = use_state(|| String::new());
    let error = use_state(|| Option::<String>::None);
    let submitting = use_state(|| false);

    let on_password_input = {
        let password = password.clone();
        Callback::from(move |e: InputEvent| {
            if let Some(input) = e.target_dyn_into::<HtmlInputElement>() {
                password.set(input.value());
            }
        })
    };

    let on_confirm_input = {
        let confirm = confirm.clone();
        Callback::from(move |e: InputEvent| {
            if let Some(input) = e.target_dyn_into::<HtmlInputElement>() {
                confirm.set(input.value());
            }
        })
    };

    let password_set = props.password_set;
    let on_authenticated = props.on_authenticated.clone();

    let on_submit = {
        let password = password.clone();
        let confirm = confirm.clone();
        let error = error.clone();
        let submitting = submitting.clone();

        Callback::from(move |e: SubmitEvent| {
            e.prevent_default();
            let pw = (*password).clone();
            let cf = (*confirm).clone();
            let error = error.clone();
            let submitting = submitting.clone();
            let on_authenticated = on_authenticated.clone();

            if pw.is_empty() {
                error.set(Some("Password cannot be empty".into()));
                return;
            }

            submitting.set(true);
            error.set(None);

            spawn_local(async move {
                let result = if password_set {
                    api::login(&pw).await
                } else {
                    api::set_password(&pw, &cf).await
                };

                match result {
                    Ok(()) => on_authenticated.emit(()),
                    Err(e) => {
                        error.set(Some(e));
                        submitting.set(false);
                    }
                }
            });
        })
    };

    let title = if props.password_set {
        "ENTER PASSWORD"
    } else {
        "SET PASSWORD"
    };

    let subtitle = if props.password_set {
        "Enter your password to access the library."
    } else {
        "Choose a password to protect your library."
    };

    html! {
        <div class="pw-backdrop">
            <div class="pw-modal">
                <div class="pw-modal__logo">{ "STARFIN" }</div>
                <div class="pw-modal__title">{ title }</div>
                <div class="pw-modal__subtitle">{ subtitle }</div>
                <form class="pw-modal__form" onsubmit={on_submit}>
                    <input
                        type="password"
                        class="pw-modal__input"
                        placeholder="Password"
                        value={(*password).clone()}
                        oninput={on_password_input}
                        autofocus=true
                    />
                    if !props.password_set {
                        <input
                            type="password"
                            class="pw-modal__input"
                            placeholder="Confirm password"
                            value={(*confirm).clone()}
                            oninput={on_confirm_input}
                        />
                    }
                    if let Some(err) = &*error {
                        <div class="pw-modal__error">{ err }</div>
                    }
                    <button
                        type="submit"
                        class="pw-modal__btn"
                        disabled={*submitting}
                    >
                        { if *submitting { "PLEASE WAIT…" } else if props.password_set { "UNLOCK" } else { "SET PASSWORD" } }
                    </button>
                </form>
            </div>
        </div>
    }
}
