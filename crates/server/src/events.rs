//! In-process fan-out of run events to SSE subscribers. Persistence is in
//! Postgres; this only carries "something new was appended" to live listeners,
//! plus the reply text an agent is writing, which is never stored.

use std::collections::HashMap;
use std::sync::{Arc, Mutex};

use sidefleet_core::{RunEvent, RunStatus};
use tokio::sync::broadcast;
use uuid::Uuid;

#[derive(Clone)]
pub struct EventBus {
    inner: Arc<Mutex<HashMap<Uuid, broadcast::Sender<Update>>>>,
    /// Every published event's run and kind, for listeners across runs (the inbox).
    all: broadcast::Sender<Touched>,
}

/// A run got an event of this kind (the payload stays with the run's own subscribers).
#[derive(Debug, Clone)]
pub struct Touched {
    pub run_id: Uuid,
    pub kind: String,
    /// The status a `status` event moved the run to.
    pub status: Option<RunStatus>,
}

/// What a run's subscribers get, in the order the worker sent it.
#[derive(Debug, Clone)]
pub enum Update {
    /// A stored event.
    Event(RunEvent),
    /// The text block the agent is writing so far (redacted; empty clears it).
    /// Superseded by the stored `assistant_text` that follows.
    Live(String),
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

    pub fn subscribe(&self, run_id: Uuid) -> broadcast::Receiver<Update> {
        let mut map = self.inner.lock().unwrap();
        map.entry(run_id)
            .or_insert_with(|| broadcast::channel(1024).0)
            .subscribe()
    }

    pub fn publish(&self, ev: RunEvent) {
        let status = (ev.kind == "status")
            .then(|| ev.payload.get("status").and_then(|s| s.as_str()).and_then(RunStatus::parse))
            .flatten();
        let _ = self.all.send(Touched { run_id: ev.run_id, kind: ev.kind.clone(), status });
        let terminal = status.is_some_and(|s| s.is_terminal());
        let mut map = self.inner.lock().unwrap();
        if let Some(tx) = map.get(&ev.run_id) {
            // Send errors only mean there are no live subscribers right now.
            let _ = tx.send(Update::Event(ev.clone()));
        }
        if terminal {
            // Subscribers already hold their receivers; dropping the sender
            // lets their streams end once they drain.
            map.remove(&ev.run_id);
        }
    }

    /// Whether anyone follows this run right now.
    pub fn watched(&self, run_id: Uuid) -> bool {
        self.inner.lock().unwrap().get(&run_id).is_some_and(|tx| tx.receiver_count() > 0)
    }

    /// Show the text the agent is writing to the run's current subscribers.
    pub fn publish_live(&self, run_id: Uuid, text: String) {
        if let Some(tx) = self.inner.lock().unwrap().get(&run_id) {
            let _ = tx.send(Update::Live(text));
        }
    }
}
