//! Port of `dash.js/src/dash/controllers/RepresentationController.js` (stub).
//!
//! Manages representation updates and quality tracking.

use crate::dash::vo::representation::Representation;

/// Manages the active representation for a media type.
pub struct RepresentationController {
    media_type: String,
    current_representation: Option<Representation>,
    representations: Vec<Representation>,
}

impl RepresentationController {
    pub fn new(media_type: &str) -> Self {
        Self {
            media_type: media_type.to_string(),
            current_representation: None,
            representations: Vec::new(),
        }
    }

    /// Get the media type.
    pub fn get_media_type(&self) -> &str {
        &self.media_type
    }

    /// Update the list of available representations from media info.
    pub fn update_data(&mut self, representations: Vec<Representation>) {
        self.representations = representations;
        if self.current_representation.is_none() && !self.representations.is_empty() {
            self.current_representation = Some(self.representations[0].clone());
        }
    }

    /// Get the current active representation.
    pub fn get_current_representation(&self) -> Option<&Representation> {
        self.current_representation.as_ref()
    }

    /// Set the current representation by quality index.
    pub fn set_representation_by_quality(&mut self, quality: usize) -> Option<&Representation> {
        if quality < self.representations.len() {
            self.current_representation = Some(self.representations[quality].clone());
            self.current_representation.as_ref()
        } else {
            None
        }
    }

    /// Get all available representations.
    pub fn get_representations(&self) -> &[Representation] {
        &self.representations
    }

    /// Get a representation by quality index.
    pub fn get_representation_for_quality(&self, quality: usize) -> Option<&Representation> {
        self.representations.get(quality)
    }

    /// Get the quality index for the current representation.
    pub fn get_quality_for_representation(&self, rep_id: &str) -> Option<usize> {
        self.representations
            .iter()
            .position(|r| r.id.as_deref() == Some(rep_id))
    }

    /// Reset state.
    pub fn reset(&mut self) {
        self.current_representation = None;
        self.representations.clear();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn make_representations() -> Vec<Representation> {
        vec![
            Representation {
                id: Some("low".to_string()),
                bandwidth: Some(500000),
                ..Representation::default()
            },
            Representation {
                id: Some("mid".to_string()),
                bandwidth: Some(1000000),
                ..Representation::default()
            },
            Representation {
                id: Some("high".to_string()),
                bandwidth: Some(2000000),
                ..Representation::default()
            },
        ]
    }

    #[test]
    fn test_update_data() {
        let mut ctrl = RepresentationController::new("video");
        ctrl.update_data(make_representations());
        assert_eq!(ctrl.get_representations().len(), 3);
        assert_eq!(
            ctrl.get_current_representation().unwrap().id.as_deref(),
            Some("low")
        );
    }

    #[test]
    fn test_set_quality() {
        let mut ctrl = RepresentationController::new("video");
        ctrl.update_data(make_representations());

        let rep = ctrl.set_representation_by_quality(2).unwrap();
        assert_eq!(rep.id.as_deref(), Some("high"));
        assert_eq!(rep.bandwidth, Some(2000000));
    }

    #[test]
    fn test_get_quality_for_representation() {
        let mut ctrl = RepresentationController::new("video");
        ctrl.update_data(make_representations());

        assert_eq!(ctrl.get_quality_for_representation("mid"), Some(1));
        assert_eq!(ctrl.get_quality_for_representation("unknown"), None);
    }

    #[test]
    fn test_reset() {
        let mut ctrl = RepresentationController::new("video");
        ctrl.update_data(make_representations());
        ctrl.reset();
        assert!(ctrl.get_current_representation().is_none());
        assert!(ctrl.get_representations().is_empty());
    }
}
