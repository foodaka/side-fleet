//! Wakes long-polling worker claims. A claim that asks to wait is held until a
//! run may have become claimable in its org: one was queued (a new task, a
//! reply, a fork, a restore, a run moved off another machine), or one finished,
//! which frees the thread's next message and a slot of the org's parallel limit.

use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};

use sidefleet_core::RunStatus;
use sqlx::Row;
use tokio::sync::{broadcast, watch};
use uuid::Uuid;

use crate::AppState;

#[derive(Clone, Default)]
pub struct ClaimSignal {
    inner: Arc<Inner>,
}

#[derive(Default)]
struct Inner {
    orgs: Mutex<HashMap<Uuid, watch::Sender<()>>>,
    closed: AtomicBool,
}

impl ClaimSignal {
    /// A receiver that sees every signal for `org` from now on.
    pub fn subscribe(&self, org: Uuid) -> watch::Receiver<()> {
        let mut orgs = self.inner.orgs.lock().unwrap();
        orgs.retain(|_, tx| tx.receiver_count() > 0);
        orgs.entry(org).or_insert_with(|| watch::channel(()).0).subscribe()
    }

    /// A run of `org` may have become claimable: waiting claims look again.
    pub fn signal(&self, org: Uuid) {
        if let Some(tx) = self.inner.orgs.lock().unwrap().get(&org) {
            tx.send_replace(());
        }
    }

    fn signal_all(&self) {
        for tx in self.inner.orgs.lock().unwrap().values() {
            tx.send_replace(());
        }
    }

    /// The server is shutting down: waiting claims answer now, new ones don't wait.
    pub fn close(&self) {
        self.inner.closed.store(true, Ordering::Relaxed);
        self.signal_all();
    }

    pub fn is_closed(&self) -> bool {
        self.inner.closed.load(Ordering::Relaxed)
    }
}

/// Follows run events and signals the run's org when one is queued or finishes.
/// Spawned by [`AppState::new`].
pub async fn watch(state: AppState) {
    let mut rx = state.events.subscribe_all();
    loop {
        match rx.recv().await {
            Ok(t) if t.status.is_some_and(|s| s == RunStatus::Queued || s.is_terminal()) => {
                match sqlx::query("SELECT org_id FROM runs WHERE id = $1").bind(t.run_id).fetch_optional(&state.pool).await {
                    Ok(Some(r)) => state.claims.signal(r.get("org_id")),
                    Ok(None) => {}
                    Err(e) => {
                        tracing::warn!(error = %e, "claim wake lookup failed; waking every org");
                        state.claims.signal_all();
                    }
                }
            }
            Ok(_) => {}
            Err(broadcast::error::RecvError::Lagged(n)) => {
                tracing::warn!(missed = n, "claim waker fell behind run events; waking every org");
                state.claims.signal_all();
            }
            Err(broadcast::error::RecvError::Closed) => return,
        }
    }
}
