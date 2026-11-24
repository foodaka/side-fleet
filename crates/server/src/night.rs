//! The night shift's end. A night-shift task (`stop_at` set, see `CreateRunRequest`) is only
//! taken by a machine with nothing else running, between its start time and `stop_at`. This
//! sweep, every [`SWEEP_EVERY`]:
//!
//! - stops night tasks still running when their night is over (their branch is kept, and a
//!   reply continues them);
//! - moves night tasks no machine took to the same hours the next night;
//! - once a person's night tasks have all stopped, sends them one push that sums the night
//!   up ("Night shift: 6 tasks in side-fleet" / "4 ready to merge · 1 failed its checks"),
//!   in place of the push per task the inbox holds back while the night lasts.

use std::collections::BTreeMap;
use std::time::Duration;

use chrono::Utc;
use serde_json::json;
use sqlx::Row;
use uuid::Uuid;

use crate::{db, push, tenancy::DEFAULT_ORG, AppState};

pub const SWEEP_EVERY: Duration = Duration::from_secs(30);
/// How long a stopped task gets to wind down before its night is summed up without it.
const STOP_GRACE_SECS: i64 = 180;

pub async fn watch(state: AppState) {
    let mut tick = tokio::time::interval_at(tokio::time::Instant::now() + SWEEP_EVERY, SWEEP_EVERY);
    tick.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
    loop {
        tick.tick().await;
        if let Err(e) = sweep(&state).await {
            tracing::warn!(error = %e, "night shift sweep failed");
        }
    }
}

/// One person's night, summed up.
#[derive(Debug, Clone, PartialEq)]
pub struct Report {
    pub org: Uuid,
    /// Who started the tasks; None for the developer key.
    pub created_by: Option<Uuid>,
    pub title: String,
    pub label: String,
}

/// How each of a night's tasks ended.
#[derive(Debug, Default, Clone, PartialEq)]
pub struct Tally {
    pub ready: usize,
    pub checks_failed: usize,
    pub done: usize,
    pub failed: usize,
    pub stopped: usize,
    pub moved: usize,
    pub repos: Vec<String>,
}

impl Tally {
    fn total(&self) -> usize {
        self.ready + self.checks_failed + self.done + self.failed + self.stopped + self.moved
    }

    /// "Night shift: 6 tasks in side-fleet", and what became of them.
    pub fn text(&self) -> (String, String) {
        let n = self.total();
        let tasks = if n == 1 { "1 task".to_string() } else { format!("{n} tasks") };
        let mut repos = self.repos.clone();
        repos.sort();
        repos.dedup();
        let title = match repos.as_slice() {
            [one] => format!("Night shift: {tasks} in {}", repo_name(one)),
            _ => format!("Night shift: {tasks}"),
        };
        let parts: Vec<String> = [
            (self.ready, "ready to merge", "ready to merge"),
            (self.done, "done", "done"),
            (self.checks_failed, "failed its checks", "failed their checks"),
            (self.failed, "failed", "failed"),
            (self.stopped, "ran out of time", "ran out of time"),
            (self.moved, "moved to tomorrow night", "moved to tomorrow night"),
        ]
        .into_iter()
        .filter(|(c, _, _)| *c > 0)
        .map(|(c, one, many)| format!("{c} {}", if c == 1 { one } else { many }))
        .collect();
        (title, parts.join(" · "))
    }
}

/// `github.com/acme/web` → `web`.
fn repo_name(repo: &str) -> &str {
    repo.trim_end_matches('/').rsplit('/').next().unwrap_or(repo)
}

/// Stop, move and sum up whatever nights are over. Returns the reports it made (sent as a
/// push when the server has web push set up).
pub async fn sweep(state: &AppState) -> sqlx::Result<Vec<Report>> {
    let pool = &state.pool;

    // Still going when the night ended: stop it.
    let running = sqlx::query(
        "SELECT r.id, r.org_id FROM runs r JOIN runs root ON root.id = COALESCE(r.parent_id, r.id)
         WHERE root.stop_at <= now() AND root.night_reported_at IS NULL
           AND r.status = 'running' AND NOT r.stop_requested
           -- Replies sent after the night ended are the person's own, daytime work.
           AND (r.id = root.id OR r.created_at < root.stop_at)",
    )
    .fetch_all(pool)
    .await?;
    for r in running {
        let (id, org): (Uuid, Uuid) = (r.get("id"), r.get("org_id"));
        if let Some((_, Some(ev))) = db::request_stop(pool, org, id).await? {
            state.events.publish(ev);
        }
        tracing::info!(run_id = %id, "the night shift ended: stopping the task");
    }

    // Nights that are over, by person.
    let roots = sqlx::query(
        "SELECT root.id, root.org_id, root.created_by, root.status, root.repo,
                (root.stop_at < now() - make_interval(secs => $1)) AS overdue,
                EXISTS (SELECT 1 FROM runs l WHERE (l.id = root.id OR l.parent_id = root.id) AND l.status = 'running') AS busy
         FROM runs root
         WHERE root.parent_id IS NULL AND root.stop_at <= now() AND root.night_reported_at IS NULL",
    )
    .bind(STOP_GRACE_SECS as f64)
    .fetch_all(pool)
    .await?;
    let mut nights: BTreeMap<(Uuid, Option<Uuid>), Vec<sqlx::postgres::PgRow>> = BTreeMap::new();
    for r in roots {
        nights.entry((r.get("org_id"), r.get("created_by"))).or_default().push(r);
    }

    let mut reports = Vec::new();
    for ((org, created_by), roots) in nights {
        // Wait until every task of the night has stopped, or has had long enough to.
        if roots.iter().any(|r| r.get::<bool, _>("busy") && !r.get::<bool, _>("overdue")) {
            continue;
        }
        let queued: Vec<Uuid> = roots.iter().filter(|r| r.get::<&str, _>("status") == "queued").map(|r| r.get("id")).collect();
        let ended: Vec<Uuid> = roots.iter().filter(|r| r.get::<&str, _>("status") != "queued").map(|r| r.get("id")).collect();
        let mut tally = Tally::default();

        // No machine took it: the same hours, the next night (or the next one still ahead).
        let moved = sqlx::query(
            "UPDATE runs SET start_at = start_at + d.shift, stop_at = stop_at + d.shift
             FROM (SELECT (floor(extract(epoch FROM now() - stop_at) / 86400)::int + 1) * interval '1 day' AS shift, id FROM runs WHERE id = ANY($1)) d
             WHERE runs.id = d.id AND runs.status = 'queued' AND runs.stop_at <= now() AND runs.night_reported_at IS NULL
             RETURNING runs.repo",
        )
        .bind(&queued)
        .fetch_all(pool)
        .await?;
        tally.moved = moved.len();
        tally.repos.extend(moved.iter().map(|r| r.get::<String, _>("repo")));

        // Claimed for this report, so a second sweep (or server) never reports it again.
        let done = sqlx::query(
            "UPDATE runs root SET night_reported_at = now()
             WHERE root.id = ANY($1) AND root.night_reported_at IS NULL
             RETURNING root.repo,
               (SELECT l.status FROM runs l WHERE l.id = root.id OR l.parent_id = root.id ORDER BY l.created_at DESC LIMIT 1) AS latest,
               (SELECT l.result->'checks'->>'verdict' FROM runs l WHERE (l.id = root.id OR l.parent_id = root.id)
                  AND l.result->'checks' IS NOT NULL ORDER BY l.created_at DESC LIMIT 1) AS checks",
        )
        .bind(&ended)
        .fetch_all(pool)
        .await?;
        for r in &done {
            tally.repos.push(r.get("repo"));
            let latest: String = r.get("latest");
            let checks: Option<String> = r.get("checks");
            match (latest.as_str(), checks.as_deref()) {
                ("succeeded", Some("passed")) => tally.ready += 1,
                ("succeeded", Some("failed")) => tally.checks_failed += 1,
                ("succeeded", _) => tally.done += 1,
                ("stopped", _) => tally.stopped += 1,
                _ => tally.failed += 1,
            }
        }
        if tally.total() == 0 {
            continue;
        }
        let (title, label) = tally.text();
        tracing::info!(org = %org, title = %title, label = %label, "night shift summed up");
        let report = Report { org, created_by, title, label };
        send(state, &report).await;
        reports.push(report);
    }
    Ok(reports)
}

/// The push for one night, to the browsers of whoever started its tasks.
async fn send(state: &AppState, report: &Report) {
    let Some(vapid) = state.config.vapid.clone() else { return };
    // The developer key's own tasks: its viewer, in the default org only.
    let viewer = match report.created_by {
        Some(u) => u,
        None if report.org == DEFAULT_ORG => Uuid::nil(),
        None => return,
    };
    let subs = match push::for_viewer(&state.pool, viewer).await {
        Ok(s) => s,
        Err(e) => return tracing::warn!(error = %e, "night shift push recipients failed"),
    };
    let body = json!({
        "id": format!("night-{}", Utc::now().format("%Y-%m-%d")),
        "title": report.title,
        "label": report.label,
        "url": "/#/",
    })
    .to_string()
    .into_bytes();
    let http = state.inbox.http();
    for sub in subs {
        match push::send(&http, &vapid, &sub, &body).await {
            push::Delivery::Sent => tracing::info!("night shift push sent"),
            push::Delivery::Gone => {
                let _ = push::forget_endpoint(&state.pool, &sub.endpoint).await;
            }
            push::Delivery::Failed(e) => tracing::warn!(error = %e, "night shift push failed"),
        }
    }
}

/// Whether the inbox should hold back a push for `root`: its night is still going, and the
/// night's summary will cover it.
pub async fn holds_push(pool: &sqlx::PgPool, root: Uuid) -> sqlx::Result<bool> {
    let r = sqlx::query("SELECT stop_at IS NOT NULL AND night_reported_at IS NULL AS held FROM runs WHERE id = $1")
        .bind(root)
        .fetch_optional(pool)
        .await?;
    Ok(r.is_some_and(|r| r.get("held")))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sums_up_a_night_in_a_line() {
        let t = Tally { ready: 4, checks_failed: 1, stopped: 1, moved: 2, repos: vec!["github.com/acme/web".into(); 3], ..Default::default() };
        assert_eq!(t.text(), ("Night shift: 8 tasks in web".into(), "4 ready to merge · 1 failed its checks · 1 ran out of time · 2 moved to tomorrow night".into()));
        let t = Tally { done: 1, repos: vec!["a".into()], ..Default::default() };
        assert_eq!(t.text(), ("Night shift: 1 task in a".into(), "1 done".into()));
        let t = Tally { failed: 2, checks_failed: 2, repos: vec!["a".into(), "b".into()], ..Default::default() };
        assert_eq!(t.text(), ("Night shift: 4 tasks".into(), "2 failed their checks · 2 failed".into()));
    }
}
