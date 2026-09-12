use std::sync::Arc;

#[derive(Debug, Clone)]
pub enum Event {
    Progress {
        downloaded: u64,
        total: u64,
        element: String,
    },
    Check {
        checked: usize,
        total: usize,
        element: String,
    },
    Speed(f64),
    Estimated(f64),
    Extract(String),
    Patch(String),
    Error(String),
}

pub type EventHandler = Arc<dyn Fn(Event) + Send + Sync>;

pub fn noop() -> EventHandler {
    Arc::new(|_| {})
}
