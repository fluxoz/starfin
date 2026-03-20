mod app;
mod api;
mod models;
mod components;
use wasm_bindgen_console_logger::DEFAULT_LOGGER;
use log::info;


fn main() {
    console_error_panic_hook::set_once();
    log::set_logger(&DEFAULT_LOGGER).unwrap();
    log::set_max_level(log::LevelFilter::Info);
    info!("App started!");
    yew::Renderer::<app::App>::new().render();
}
