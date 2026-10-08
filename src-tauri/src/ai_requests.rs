use std::collections::HashMap;
use std::sync::Mutex;
use tokio::sync::oneshot;

/// Registry of in-flight native AI requests so the frontend can cancel them.
/// Each AI command registers a one-shot channel under a request id and selects
/// between the AI future and the cancel signal.
#[derive(Default)]
pub struct AiRequestRegistry {
    pending: Mutex<HashMap<String, oneshot::Sender<()>>>,
}

impl AiRequestRegistry {
    pub fn new() -> Self {
        Self {
            pending: Mutex::new(HashMap::new()),
        }
    }

    pub fn register(&self, request_id: String) -> oneshot::Receiver<()> {
        let (tx, rx) = oneshot::channel();
        if let Ok(mut pending) = self.pending.lock() {
            pending.insert(request_id, tx);
        }
        rx
    }

    pub fn unregister(&self, request_id: &str) {
        if let Ok(mut pending) = self.pending.lock() {
            pending.remove(request_id);
        }
    }

    pub fn cancel(&self, request_id: &str) -> bool {
        if let Ok(mut pending) = self.pending.lock() {
            if let Some(tx) = pending.remove(request_id) {
                let _ = tx.send(());
                return true;
            }
        }
        false
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn register_and_cancel_signals() {
        let registry = AiRequestRegistry::new();
        let mut rx = registry.register("abc".to_string());
        assert!(registry.cancel("abc"));
        assert_eq!(rx.try_recv().ok(), Some(()));
        assert!(!registry.cancel("abc"));
    }

    #[test]
    fn unregister_removes_pending() {
        let registry = AiRequestRegistry::new();
        let mut rx = registry.register("abc".to_string());
        registry.unregister("abc");
        assert!(!registry.cancel("abc"));
        assert!(rx.try_recv().is_err());
    }
}
