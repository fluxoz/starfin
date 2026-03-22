//! Port of dash.js `BaseURLController`.
//!
//! Manages a prioritised, weighted list of base URLs and resolves relative
//! paths against the currently selected base URL.

/// A single base URL entry from the MPD.
#[derive(Clone, Debug)]
pub struct BaseUrl {
    pub url: String,
    pub service_location: Option<String>,
    pub priority: u32,
    pub weight: u32,
}

/// Selects among available base URLs using priority / weight and resolves
/// relative paths.
#[derive(Clone, Debug, Default)]
pub struct BaseUrlController {
    base_urls: Vec<BaseUrl>,
    selected_index: usize,
}

impl BaseUrlController {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn add_url(&mut self, url: BaseUrl) {
        self.base_urls.push(url);
    }

    /// Selects the best URL: lowest priority first, then highest weight among
    /// candidates sharing that priority. Updates `selected_index`.
    pub fn select_url(&mut self) -> Option<&BaseUrl> {
        if self.base_urls.is_empty() {
            return None;
        }
        let min_priority = self.base_urls.iter().map(|u| u.priority).min().unwrap();
        let best_idx = self
            .base_urls
            .iter()
            .enumerate()
            .filter(|(_, u)| u.priority == min_priority)
            .max_by_key(|(_, u)| u.weight)
            .map(|(i, _)| i)
            .unwrap();
        self.selected_index = best_idx;
        Some(&self.base_urls[self.selected_index])
    }

    pub fn get_selected_url(&self) -> Option<&BaseUrl> {
        self.base_urls.get(self.selected_index)
    }

    pub fn get_all_urls(&self) -> &[BaseUrl] {
        &self.base_urls
    }

    /// Combines the selected base URL with a relative path.
    pub fn resolve(&self, relative: &str) -> Option<String> {
        self.base_urls.get(self.selected_index).map(|base| {
            if base.url.ends_with('/') {
                format!("{}{}", base.url, relative)
            } else {
                format!("{}/{}", base.url, relative)
            }
        })
    }

    pub fn reset(&mut self) {
        self.base_urls.clear();
        self.selected_index = 0;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn priority_selection() {
        let mut ctrl = BaseUrlController::new();
        ctrl.add_url(BaseUrl {
            url: "http://high.example.com/".to_string(),
            service_location: None,
            priority: 10,
            weight: 1,
        });
        ctrl.add_url(BaseUrl {
            url: "http://low.example.com/".to_string(),
            service_location: None,
            priority: 1,
            weight: 1,
        });

        let selected = ctrl.select_url().unwrap();
        assert_eq!(selected.url, "http://low.example.com/");
    }

    #[test]
    fn weight_selection_among_same_priority() {
        let mut ctrl = BaseUrlController::new();
        ctrl.add_url(BaseUrl {
            url: "http://light.example.com/".to_string(),
            service_location: None,
            priority: 1,
            weight: 10,
        });
        ctrl.add_url(BaseUrl {
            url: "http://heavy.example.com/".to_string(),
            service_location: None,
            priority: 1,
            weight: 90,
        });

        let selected = ctrl.select_url().unwrap();
        assert_eq!(selected.url, "http://heavy.example.com/");
    }

    #[test]
    fn resolve_paths() {
        let mut ctrl = BaseUrlController::new();
        ctrl.add_url(BaseUrl {
            url: "http://cdn.example.com/content".to_string(),
            service_location: None,
            priority: 1,
            weight: 1,
        });
        ctrl.select_url();

        assert_eq!(
            ctrl.resolve("segment_0.m4s").unwrap(),
            "http://cdn.example.com/content/segment_0.m4s"
        );
    }

    #[test]
    fn resolve_with_trailing_slash() {
        let mut ctrl = BaseUrlController::new();
        ctrl.add_url(BaseUrl {
            url: "http://cdn.example.com/content/".to_string(),
            service_location: None,
            priority: 1,
            weight: 1,
        });
        ctrl.select_url();

        assert_eq!(
            ctrl.resolve("segment_0.m4s").unwrap(),
            "http://cdn.example.com/content/segment_0.m4s"
        );
    }

    #[test]
    fn reset_clears() {
        let mut ctrl = BaseUrlController::new();
        ctrl.add_url(BaseUrl {
            url: "http://example.com/".to_string(),
            service_location: None,
            priority: 1,
            weight: 1,
        });
        ctrl.reset();
        assert!(ctrl.get_all_urls().is_empty());
        assert!(ctrl.get_selected_url().is_none());
    }
}
