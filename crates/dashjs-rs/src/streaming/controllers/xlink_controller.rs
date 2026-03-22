//! Port of `dash.js/src/streaming/controllers/XlinkController.js`.
//!
//! Resolves xlink:href attributes in MPD Period and AdaptationSet elements.

/// Resolve type constants.
pub const RESOLVE_TYPE_ONLOAD: &str = "onLoad";
pub const RESOLVE_TYPE_ONACTUATE: &str = "onActuate";
pub const RESOLVE_TO_ZERO: &str = "urn:mpeg:dash:resolve-to-zero:2013";

/// An xlink element pending resolution.
#[derive(Clone, Debug)]
pub struct XlinkElement {
    pub url: String,
    pub element_type: String,
    pub index: usize,
    pub resolve_type: String,
    pub resolved: bool,
    pub resolved_content: Option<String>,
}

impl XlinkElement {
    pub fn new(url: String, element_type: String, index: usize, resolve_type: String) -> Self {
        Self { url, element_type, index, resolve_type, resolved: false, resolved_content: None }
    }
}

/// Controls xlink resolution for an MPD manifest.
#[derive(Clone, Debug, Default)]
pub struct XlinkController {
    pending: Vec<XlinkElement>,
}

impl XlinkController {
    pub fn new() -> Self {
        Self::default()
    }

    /// Queues an element for xlink resolution.
    pub fn queue_element(&mut self, element: XlinkElement) {
        if element.url == RESOLVE_TO_ZERO {
            return;
        }
        self.pending.push(element);
    }

    /// Marks an element as resolved with optional content.
    pub fn mark_resolved(&mut self, index: usize, content: Option<String>) {
        if let Some(el) = self.pending.get_mut(index) {
            el.resolved = true;
            el.resolved_content = content;
        }
    }

    /// Returns `true` when all queued elements have been resolved.
    pub fn is_resolving_finished(&self) -> bool {
        self.pending.iter().all(|e| e.resolved)
    }

    /// Returns pending elements that still need to be fetched.
    pub fn get_pending(&self) -> &[XlinkElement] {
        &self.pending
    }

    pub fn reset(&mut self) {
        self.pending.clear();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn resolve_to_zero_not_queued() {
        let mut ctrl = XlinkController::new();
        ctrl.queue_element(XlinkElement::new(
            RESOLVE_TO_ZERO.to_string(), "Period".to_string(), 0, RESOLVE_TYPE_ONLOAD.to_string(),
        ));
        assert!(ctrl.get_pending().is_empty());
    }

    #[test]
    fn resolving_finished_when_all_resolved() {
        let mut ctrl = XlinkController::new();
        ctrl.queue_element(XlinkElement::new(
            "http://example.com/period.xml".to_string(), "Period".to_string(), 0, RESOLVE_TYPE_ONLOAD.to_string(),
        ));
        assert!(!ctrl.is_resolving_finished());
        ctrl.mark_resolved(0, Some("<Period/>".to_string()));
        assert!(ctrl.is_resolving_finished());
    }
}
