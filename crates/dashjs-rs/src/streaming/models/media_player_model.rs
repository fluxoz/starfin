//! Port of `dash.js/src/streaming/models/MediaPlayerModel.js`.
use crate::core::settings::Settings;
#[derive(Clone, Debug, Default)]
pub struct MediaPlayerModel { pub settings: Settings }
impl MediaPlayerModel { pub fn new(settings: Settings) -> Self { Self { settings } } }
