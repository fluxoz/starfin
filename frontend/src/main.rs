mod app;
mod api;
mod models;
mod components;


fn main() {
    yew::Renderer::<app::App>::new().render();
}
