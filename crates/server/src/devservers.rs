//! Dev servers: servers a task's agent left running on its machine. The machine reports
//! the full set with every heartbeat (see `sidefleet_worker::devservers`); the page lists a
//! thread's and asks to stop one, which reaches the machine with its next heartbeat.

use chrono::{DateTime, Utc};
use serde::Serialize;
use serde_json::json;
use sidefleet_core::{DevServer, DevServerStop};
use sqlx::{PgPool, Row};
use uuid::Uuid;

/// The most a machine may report at once.
const MAX_REPORTED: usize = 100;
/// A stop request is passed on with each heartbeat for this long, then shows as not stopping.
const STOP_WINDOW: &str = "2 minutes";

/// One of a thread's dev servers, for the page.
#[derive(Debug, Clone, Serialize)]
pub struct DevServerView {
    pub pid: i32,
    pub ports: Vec<i32>,
    pub command: String,
    pub started_at: Option<DateTime<Utc>>,
    pub worker_id: String,
    /// Someone asked to stop it and the machine hasn't reported it gone yet.
    pub stopping: bool,
}

/// Replace what `worker_id` last reported for threads in `orgs`. A server only counts
/// in a worktree thread of those orgs that ran on this machine.
pub async fn report(pool: &PgPool, orgs: &[Uuid], worker_id: &str, servers: &[DevServer]) -> sqlx::Result<()> {
    let servers = json!(servers.iter().take(MAX_REPORTED).collect::<Vec<_>>());
    let mut tx = pool.begin().await?;
    sqlx::query(
        "DELETE FROM dev_servers d USING runs r
          WHERE d.thread_id = r.id AND r.org_id = ANY($3) AND d.worker_id = $2
            AND NOT EXISTS (SELECT 1 FROM jsonb_to_recordset($1) AS x(thread uuid, pid int) WHERE x.thread = d.thread_id AND x.pid = d.pid)",
    )
    .bind(&servers)
    .bind(worker_id)
    .bind(orgs)
    .execute(&mut *tx)
    .await?;
    sqlx::query(
        "INSERT INTO dev_servers (thread_id, pid, worker_id, ports, command, started_at)
         SELECT DISTINCT ON (x.thread, x.pid) x.thread, x.pid, $2, x.ports, left(x.command, 300), x.started_at
           FROM jsonb_to_recordset($1) AS x(thread uuid, pid int, ports int[], command text, started_at timestamptz)
           JOIN runs r ON r.id = x.thread AND r.parent_id IS NULL AND r.org_id = ANY($3) AND r.isolation = 'worktree'
          WHERE x.ports IS NOT NULL AND x.command IS NOT NULL
            AND EXISTS (SELECT 1 FROM runs l WHERE (l.id = r.id OR l.parent_id = r.id) AND l.worker_id = $2)
         ON CONFLICT (thread_id, pid) DO UPDATE SET
            worker_id = EXCLUDED.worker_id, ports = EXCLUDED.ports, command = EXCLUDED.command, seen_at = now(),
            -- The same pid in a later process is another server: forget the stop.
            stop_requested_at = CASE WHEN dev_servers.started_at IS NOT DISTINCT FROM EXCLUDED.started_at THEN dev_servers.stop_requested_at END,
            started_at = EXCLUDED.started_at",
    )
    .bind(&servers)
    .bind(worker_id)
    .bind(orgs)
    .execute(&mut *tx)
    .await?;
    tx.commit().await
}

/// Dev servers on `worker_id` that someone asked to stop.
pub async fn stops_for(pool: &PgPool, orgs: &[Uuid], worker_id: &str) -> sqlx::Result<Vec<DevServerStop>> {
    let rows = sqlx::query(&format!(
        "SELECT d.thread_id, d.pid, d.started_at FROM dev_servers d JOIN runs r ON r.id = d.thread_id
          WHERE d.worker_id = $1 AND r.org_id = ANY($2) AND d.stop_requested_at > now() - interval '{STOP_WINDOW}'"
    ))
    .bind(worker_id)
    .bind(orgs)
    .fetch_all(pool)
    .await?;
    Ok(rows
        .iter()
        .map(|r| DevServerStop { thread: r.get("thread_id"), pid: r.get::<i32, _>("pid") as u32, started_at: r.get("started_at") })
        .collect())
}

/// The dev servers of thread `root` (of `org`) on machines that are online.
pub async fn list(pool: &PgPool, org: Uuid, root: Uuid) -> sqlx::Result<Vec<DevServerView>> {
    let rows = sqlx::query(&format!(
        "SELECT d.pid, d.ports, d.command, d.started_at, d.worker_id,
                COALESCE(d.stop_requested_at > now() - interval '{STOP_WINDOW}', false) AS stopping
           FROM dev_servers d JOIN runs r ON r.id = d.thread_id AND r.org_id = $2
           JOIN workers w ON w.id = d.worker_id AND w.org_id = $2
          WHERE d.thread_id = $1 AND w.last_heartbeat_at > now() - interval '45 seconds'
          ORDER BY d.ports[1], d.pid"
    ))
    .bind(root)
    .bind(org)
    .fetch_all(pool)
    .await?;
    Ok(rows
        .iter()
        .map(|r| DevServerView {
            pid: r.get("pid"),
            ports: r.get("ports"),
            command: r.get("command"),
            started_at: r.get("started_at"),
            worker_id: r.get("worker_id"),
            stopping: r.get("stopping"),
        })
        .collect())
}

/// Ask for thread `root`'s server `pid` to be stopped. False when there is no such server.
pub async fn request_stop(pool: &PgPool, root: Uuid, pid: u32) -> sqlx::Result<bool> {
    let res = sqlx::query("UPDATE dev_servers SET stop_requested_at = now() WHERE thread_id = $1 AND pid = $2")
        .bind(root)
        .bind(pid as i32)
        .execute(pool)
        .await?;
    Ok(res.rows_affected() == 1)
}
