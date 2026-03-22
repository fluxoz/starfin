//! Port of `dash.js/src/streaming/models/MediaPlayerModel.js`.
use crate::core::settings::Settings;
#[derive(Clone, Debug, Default)]
pub struct MediaPlayerModel { pub settings: Settings }
impl MediaPlayerModel { pub fn new(settings: Settings) -> Self { Self { settings } } }

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn new_with_default_settings() {
        let model = MediaPlayerModel::new(Settings::default());
        // buffer settings should be accessible
        let _buf = model.settings.streaming.buffer.initial_buffer_level;
    }

    #[test]
    fn new_preserves_custom_log_level() {
        let mut settings = Settings::default();
        settings.debug.log_level = 4; // DEBUG level
        let model = MediaPlayerModel::new(settings);
        assert_eq!(model.settings.debug.log_level, 4);
    }

    #[test]
    fn default_model() {
        let model = MediaPlayerModel::default();
        // Default settings should be applied — auto_switch_bitrate.video is true
        assert!(model.settings.streaming.abr.auto_switch_bitrate.video);
    }

    #[test]
    fn clone_model() {
        let model = MediaPlayerModel::new(Settings::default());
        let model2 = model.clone();
        assert_eq!(
            model.settings.streaming.abr.auto_switch_bitrate.video,
            model2.settings.streaming.abr.auto_switch_bitrate.video,
        );
    }
}
