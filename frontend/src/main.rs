mod app;
mod api;
mod models;
mod components;
use log::info;


fn main() {
    wasm_logger::init(wasm_logger::Config::default());
    yew::Renderer::<app::App>::new().render();
}
