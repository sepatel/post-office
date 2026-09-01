use std::collections::{HashMap, HashSet};
use std::sync::{Arc, Mutex};

use tokio::sync::{OwnedSemaphorePermit, Semaphore};

#[derive(Clone, Default)]
pub struct InferenceRuntime {
    gates: Arc<Mutex<HashMap<String, Gate>>>,
    reasoning_passthrough: Arc<Mutex<HashSet<String>>>,
}

struct Gate {
    capacity: usize,
    semaphore: Arc<Semaphore>,
}

impl InferenceRuntime {
    pub async fn acquire(&self, endpoint: &str, capacity: usize) -> OwnedSemaphorePermit {
        let capacity = capacity.max(1);
        let semaphore = {
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
        };
        semaphore
            .acquire_owned()
            .await
            .expect("inference semaphore is never closed")
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
