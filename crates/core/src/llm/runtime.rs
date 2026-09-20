use std::collections::{HashMap, HashSet};
use std::sync::{Arc, Mutex};

use tokio::sync::{watch, Notify, OwnedSemaphorePermit, Semaphore};

#[derive(Clone)]
pub struct InferenceRuntime {
    gates: Arc<Mutex<HashMap<String, Gate>>>,
    reasoning_passthrough: Arc<Mutex<HashSet<String>>>,
    cancellation: watch::Sender<u64>,
    retry_wake: Arc<Notify>,
}

impl Default for InferenceRuntime {
    fn default() -> Self {
        let (cancellation, _) = watch::channel(0);
        Self {
            gates: Arc::new(Mutex::new(HashMap::new())),
            reasoning_passthrough: Arc::new(Mutex::new(HashSet::new())),
            cancellation,
            retry_wake: Arc::new(Notify::new()),
        }
    }
}

struct Gate {
    capacity: usize,
    semaphore: Arc<Semaphore>,
}

impl InferenceRuntime {
    fn semaphore(&self, endpoint: &str, capacity: usize) -> Arc<Semaphore> {
        let capacity = capacity.max(1);
        let mut gates = self.gates.lock().unwrap();
        let gate = gates.entry(endpoint.to_string()).or_insert_with(|| Gate {
            capacity,
            semaphore: Arc::new(Semaphore::new(capacity)),
        });
        if gate.capacity != capacity {
            *gate = Gate {
                capacity,
                semaphore: Arc::new(Semaphore::new(capacity)),
            };
        }
        Arc::clone(&gate.semaphore)
    }

    pub async fn acquire(&self, endpoint: &str, capacity: usize) -> OwnedSemaphorePermit {
        self.semaphore(endpoint, capacity)
            .acquire_owned()
            .await
            .expect("inference semaphore is never closed")
    }

    pub fn available_permits(&self, endpoint: &str, capacity: usize) -> usize {
        self.semaphore(endpoint, capacity).available_permits()
    }

    pub fn cancellation(&self) -> watch::Receiver<u64> {
        self.cancellation.subscribe()
    }

    pub fn cancel_all(&self) {
        self.cancellation.send_modify(|generation| {
            *generation = generation.wrapping_add(1);
        });
    }

    pub fn wake_retry_worker(&self) {
        self.retry_wake.notify_one();
    }

    pub async fn wait_for_retry_work(&self) {
        self.retry_wake.notified().await;
    }

    pub fn requires_reasoning_passthrough(&self, endpoint: &str, model: &str) -> bool {
        self.reasoning_passthrough
            .lock()
            .unwrap()
            .contains(&format!("{endpoint}\u{0}{model}"))
    }

    pub fn enable_reasoning_passthrough(&self, endpoint: &str, model: &str) {
        self.reasoning_passthrough
            .lock()
            .unwrap()
            .insert(format!("{endpoint}\u{0}{model}"));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cancellation_notifies_requests_that_are_already_running() {
        let runtime = InferenceRuntime::default();
        let cancellation = runtime.cancellation();

        runtime.cancel_all();

        assert!(cancellation.has_changed().unwrap());
    }
}
