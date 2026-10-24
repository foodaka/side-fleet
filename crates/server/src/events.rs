//! In-process fan-out of run events to SSE subscribers. Persistence is in
//! Postgres; this only carries "something new was appended" to live listeners.

use std::collections::HashMap;
use std::sync::{Arc, Mutex};

use sidefleet_core::RunEvent;
use tokio::sync::broadcast;
use uuid::Uuid;

#[derive(Clone)]
pub struct EventBus {
    inner: Arc<Mutex<HashMap<Uuid, broadcast::Sender<RunEvent>>>>,
    /// Every published event's run and kind, for listeners across runs (the inbox).
    all: broadcast::Sender<Touched>,
}

/// A run got an event of this kind (the payload stays with the run's own subscribers).
#[derive(Debug, Clone)]
pub struct Touched {
    pub run_id: Uuid,
    pub kind: String,
}

impl Default for EventBus {
    fn default() -> Self {
        Self { inner: Arc::default(), all: broadcast::channel(1024).0 }
    }
}

impl EventBus {
    pub fn subscribe_all(&self) -> broadcast::Receiver<Touched> {
        self.all.subscribe()
    }

    pub fn subscribe(&self, run_id: Uuid) -> broadcast::Receiver<RunEvent> {
        let mut map = self.inner.lock().unwrap();
        map.entry(run_id)
            .or_insert_with(|| broadcast::channel(1024).0)
            .subscribe()
    }

    pub fn publish(&self, ev: RunEvent) {
        let _ = self.all.send(Touched { run_id: ev.run_id, kind: ev.kind.clone() });
        let terminal = ev.kind == "status"
            && ev
                .payload
                .get("status")
                .and_then(|s| s.as_str())
                .and_then(sidefleet_core::RunStatus::parse)
                .map(|s| s.is_terminal())
                .unwrap_or(false);
        let mut map = self.inner.lock().unwrap();
        if let Some(tx) = map.get(&ev.run_id) {
            // Send errors only mean there are no live subscribers right now.
            let _ = tx.send(ev.clone());
        }
        if terminal {
            // Subscribers already hold their receivers; dropping the sender
            // lets their streams end once they drain.
            map.remove(&ev.run_id);
        }
    }
}
