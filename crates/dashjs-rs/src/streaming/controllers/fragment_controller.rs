//! Port of dash.js `FragmentController`.
//!
//! Manages the lifecycle of individual fragment (segment) requests: queuing,
//! execution, completion and failure tracking.

/// Current state of the fragment controller pipeline.
#[derive(Clone, Debug, PartialEq)]
pub enum FragmentState {
    Idle,
    Loading,
    Completed,
    Failed,
}

impl Default for FragmentState {
    fn default() -> Self {
        Self::Idle
    }
}

/// A single fragment (segment) request.
#[derive(Clone, Debug)]
pub struct FragmentRequest {
    pub url: String,
    pub media_type: String,
    pub quality: usize,
    pub index: u64,
    pub start_time: f64,
    pub duration: f64,
    pub representation_id: String,
}

/// Controls fragment request queuing and completion tracking.
#[derive(Clone, Debug)]
pub struct FragmentController {
    state: FragmentState,
    pending_requests: Vec<FragmentRequest>,
    executed_requests: Vec<FragmentRequest>,
    max_pending: usize,
}

impl Default for FragmentController {
    fn default() -> Self {
        Self {
            state: FragmentState::Idle,
            pending_requests: Vec::new(),
            executed_requests: Vec::new(),
            max_pending: 1,
        }
    }
}

impl FragmentController {
    pub fn new() -> Self {
        Self::default()
    }

    /// Enqueues a request if the pending queue is not full.
    pub fn process_request(&mut self, request: FragmentRequest) -> bool {
        if self.pending_requests.len() >= self.max_pending {
            return false;
        }
        self.pending_requests.push(request);
        self.state = FragmentState::Loading;
        true
    }

    /// Returns a reference to the next pending request and transitions to
    /// `Loading`.
    pub fn execute_current(&mut self) -> Option<&FragmentRequest> {
        if self.pending_requests.is_empty() {
            return None;
        }
        self.state = FragmentState::Loading;
        self.pending_requests.first()
    }

    /// Marks the request with the given `index` as completed, moving it from
    /// pending to executed.
    pub fn on_request_completed(&mut self, index: u64) {
        if let Some(pos) = self.pending_requests.iter().position(|r| r.index == index) {
            let req = self.pending_requests.remove(pos);
            self.executed_requests.push(req);
        }
        if self.pending_requests.is_empty() {
            self.state = FragmentState::Completed;
        }
    }

    pub fn on_request_failed(&mut self, index: u64) {
        self.pending_requests.retain(|r| r.index != index);
        if self.pending_requests.is_empty() {
            self.state = FragmentState::Failed;
        }
    }

    pub fn get_state(&self) -> &FragmentState {
        &self.state
    }

    pub fn get_pending_requests(&self) -> &[FragmentRequest] {
        &self.pending_requests
    }

    pub fn get_executed_requests(&self) -> &[FragmentRequest] {
        &self.executed_requests
    }

    pub fn has_pending_requests(&self) -> bool {
        !self.pending_requests.is_empty()
    }

    pub fn clear_pending(&mut self) {
        self.pending_requests.clear();
        if self.state == FragmentState::Loading {
            self.state = FragmentState::Idle;
        }
    }

    pub fn reset(&mut self) {
        self.state = FragmentState::Idle;
        self.pending_requests.clear();
        self.executed_requests.clear();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn make_request(index: u64) -> FragmentRequest {
        FragmentRequest {
            url: format!("http://example.com/seg_{}.m4s", index),
            media_type: "video".to_string(),
            quality: 0,
            index,
            start_time: index as f64 * 4.0,
            duration: 4.0,
            representation_id: "v0".to_string(),
        }
    }

    #[test]
    fn process_and_execute() {
        let mut ctrl = FragmentController::new();
        assert!(ctrl.process_request(make_request(0)));
        assert_eq!(*ctrl.get_state(), FragmentState::Loading);
        assert!(ctrl.has_pending_requests());

        let req = ctrl.execute_current().unwrap();
        assert_eq!(req.index, 0);
    }

    #[test]
    fn max_pending_enforced() {
        let mut ctrl = FragmentController::new();
        assert!(ctrl.process_request(make_request(0)));
        assert!(!ctrl.process_request(make_request(1)));
    }

    #[test]
    fn completion_tracking() {
        let mut ctrl = FragmentController::new();
        ctrl.process_request(make_request(0));
        ctrl.on_request_completed(0);

        assert_eq!(*ctrl.get_state(), FragmentState::Completed);
        assert!(!ctrl.has_pending_requests());
        assert_eq!(ctrl.get_executed_requests().len(), 1);
    }

    #[test]
    fn failure_tracking() {
        let mut ctrl = FragmentController::new();
        ctrl.process_request(make_request(0));
        ctrl.on_request_failed(0);

        assert_eq!(*ctrl.get_state(), FragmentState::Failed);
        assert!(!ctrl.has_pending_requests());
        assert!(ctrl.get_executed_requests().is_empty());
    }

    #[test]
    fn reset_clears_all() {
        let mut ctrl = FragmentController::new();
        ctrl.process_request(make_request(0));
        ctrl.on_request_completed(0);
        ctrl.reset();

        assert_eq!(*ctrl.get_state(), FragmentState::Idle);
        assert!(ctrl.get_pending_requests().is_empty());
        assert!(ctrl.get_executed_requests().is_empty());
    }
}
