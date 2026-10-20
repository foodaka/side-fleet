//! In-process fan-out of run events to SSE subscribers. Persistence is in
//! Postgres; this only carries "something new was appended" to live listeners.

use std::collections::HashMap;
use std::sync::{Arc, Mutex};

use ac_core::RunEvent;
use tokio::sync::broadcast;
use uuid::Uuid;

#[derive(Clone, Default)]
pub struct EventBus {
    inner: Arc<Mutex<HashMap<Uuid, broadcast::Sender<RunEvent>>>>,
}

impl EventBus {
    pub fn subscribe(&self, run_id: Uuid) -> broadcast::Receiver<RunEvent> {
        let mut map = self.inner.lock().unwrap();
        map.entry(run_id)
            .or_insert_with(|| broadcast::channel(1024).0)
            .subscribe()
    }

    pub fn publish(&self, ev: RunEvent) {
        let terminal = ev.kind == "status"
            && ev
                .payload
                .get("status")
                .and_then(|s| s.as_str())
                .and_then(ac_core::RunStatus::parse)
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
