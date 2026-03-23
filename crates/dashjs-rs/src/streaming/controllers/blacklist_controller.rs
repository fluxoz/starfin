//! Port of `dash.js/src/streaming/controllers/BlacklistController.js`.
//!
//! Maintains a list of blacklisted entries (e.g. URLs or service-location
//! strings) and allows callers to query membership.

/// Manages a list of blacklisted string entries.
#[derive(Clone, Debug, Default)]
pub struct BlacklistController {
    blacklist: Vec<String>,
}

impl BlacklistController {
    pub fn new() -> Self {
        Self::default()
    }

    /// Returns `true` when `query` is present in the blacklist.
    pub fn contains(&self, query: &str) -> bool {
        if self.blacklist.is_empty() || query.is_empty() {
            return false;
        }
        self.blacklist.iter().any(|e| e == query)
    }

    /// Adds `entry` to the blacklist if it is not already present.
    pub fn add(&mut self, entry: String) {
        if !self.blacklist.iter().any(|e| e == &entry) {
            self.blacklist.push(entry);
        }
    }

    /// Removes `entry` from the blacklist if present.
    pub fn remove(&mut self, entry: &str) {
        self.blacklist.retain(|e| e != entry);
    }

    pub fn reset(&mut self) {
        self.blacklist.clear();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn add_and_contains() {
        let mut ctrl = BlacklistController::new();
        assert!(!ctrl.contains("http://example.com"));
        ctrl.add("http://example.com".to_string());
        assert!(ctrl.contains("http://example.com"));
    }

    #[test]
    fn no_duplicates() {
        let mut ctrl = BlacklistController::new();
        ctrl.add("a".to_string());
        ctrl.add("a".to_string());
        assert_eq!(ctrl.blacklist.len(), 1);
    }

    #[test]
    fn remove_entry() {
        let mut ctrl = BlacklistController::new();
        ctrl.add("a".to_string());
        ctrl.remove("a");
        assert!(!ctrl.contains("a"));
    }

    #[test]
    fn reset_clears() {
        let mut ctrl = BlacklistController::new();
        ctrl.add("a".to_string());
        ctrl.reset();
        assert!(!ctrl.contains("a"));
    }
}
