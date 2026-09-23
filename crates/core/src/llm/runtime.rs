use std::collections::{HashMap, HashSet};
use std::future::Future;
use std::sync::{Arc, Mutex};

use tokio::sync::{watch, Notify};

tokio::task_local! {
    static RESERVED_LANE: ReservedLane;
}

#[derive(Clone)]
pub struct InferenceRuntime {
    gates: Arc<Mutex<HashMap<String, Arc<ModelGate>>>>,
    reasoning_passthrough: Arc<Mutex<HashSet<String>>>,
    cancellation: watch::Sender<u64>,
    retry_wake: Arc<Notify>,
}

struct ModelGate {
    state: Mutex<GateState>,
    available: Notify,
}

struct GateState {
    capacity: usize,
    active: usize,
}

pub struct InferencePermit {
    gate: Option<Arc<ModelGate>>,
}

struct ReservedLane {
    key: String,
    _permit: InferencePermit,
}

impl Drop for InferencePermit {
    fn drop(&mut self) {
        let Some(gate) = self.gate.take() else {
            return;
        };
        gate.state.lock().unwrap().active -= 1;
        gate.available.notify_one();
    }
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

impl InferenceRuntime {
    pub fn model_key(endpoint: &str, model: &str) -> String {
        format!(
            "{}\u{0}{}",
            endpoint.trim().trim_end_matches('/').to_ascii_lowercase(),
            model.trim()
        )
    }

    fn gate(&self, endpoint: &str, model: &str, capacity: usize) -> Arc<ModelGate> {
        let key = Self::model_key(endpoint, model);
        let mut gates = self.gates.lock().unwrap();
        Arc::clone(gates.entry(key).or_insert_with(|| {
            Arc::new(ModelGate {
                state: Mutex::new(GateState {
                    capacity: capacity.max(1),
                    active: 0,
                }),
                available: Notify::new(),
            })
        }))
    }

    pub fn set_capacity(&self, endpoint: &str, model: &str, capacity: usize) {
        let gate = self.gate(endpoint, model, capacity);
        gate.state.lock().unwrap().capacity = capacity.max(1);
        gate.available.notify_waiters();
    }

    pub fn try_acquire(
        &self,
        endpoint: &str,
        model: &str,
        capacity: usize,
    ) -> Option<InferencePermit> {
        let gate = self.gate(endpoint, model, capacity);
        Self::try_acquire_gate(gate)
    }

    pub async fn acquire(&self, endpoint: &str, model: &str, capacity: usize) -> InferencePermit {
        let key = Self::model_key(endpoint, model);
        if RESERVED_LANE
            .try_with(|reservation| reservation.key == key)
            .unwrap_or(false)
        {
            return InferencePermit { gate: None };
        }

        let gate = self.gate(endpoint, model, capacity);
        loop {
            if let Some(permit) = Self::try_acquire_gate(Arc::clone(&gate)) {
                return permit;
            }
            let notified = gate.available.notified();
            if let Some(permit) = Self::try_acquire_gate(Arc::clone(&gate)) {
                return permit;
            }
            notified.await;
        }
    }

    pub async fn with_reservation<T>(
        &self,
        endpoint: &str,
        model: &str,
        permit: InferencePermit,
        future: impl Future<Output = T>,
    ) -> T {
        RESERVED_LANE
            .scope(
                ReservedLane {
                    key: Self::model_key(endpoint, model),
                    _permit: permit,
                },
                future,
            )
            .await
    }

    pub fn available_permits(&self, endpoint: &str, model: &str, capacity: usize) -> usize {
        let gate = self.gate(endpoint, model, capacity);
        let state = gate.state.lock().unwrap();
        state.capacity.saturating_sub(state.active)
    }

    fn try_acquire_gate(gate: Arc<ModelGate>) -> Option<InferencePermit> {
        let mut state = gate.state.lock().unwrap();
        if state.active >= state.capacity {
            return None;
        }
        state.active += 1;
        drop(state);
        Some(InferencePermit { gate: Some(gate) })
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
            .contains(&Self::model_key(endpoint, model))
    }

    pub fn enable_reasoning_passthrough(&self, endpoint: &str, model: &str) {
        self.reasoning_passthrough
            .lock()
            .unwrap()
            .insert(Self::model_key(endpoint, model));
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

    #[test]
    fn lower_capacity_blocks_new_requests_until_active_requests_finish() {
        let runtime = InferenceRuntime::default();
        let first = runtime.try_acquire("http://local/v1", "model", 2).unwrap();
        let second = runtime.try_acquire("http://local/v1", "model", 2).unwrap();

        runtime.set_capacity("http://local/v1", "model", 1);
        assert!(runtime.try_acquire("http://local/v1", "model", 1).is_none());

        drop(first);
        assert!(runtime.try_acquire("http://local/v1", "model", 1).is_none());

        drop(second);
        assert!(runtime.try_acquire("http://local/v1", "model", 1).is_some());
    }

    #[test]
    fn reservation_is_reused_by_the_matching_model_request() {
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap();
        runtime.block_on(async {
            let inference = InferenceRuntime::default();
            let permit = inference
                .try_acquire("http://local/v1", "model", 1)
                .unwrap();

            inference
                .with_reservation("http://local/v1", "model", permit, async {
                    let borrowed = inference.acquire("http://local/v1", "model", 1).await;
                    assert_eq!(
                        inference.available_permits("http://local/v1", "model", 1),
                        0
                    );
                    drop(borrowed);
                })
                .await;

            assert_eq!(
                inference.available_permits("http://local/v1", "model", 1),
                1
            );
        });
    }
}
