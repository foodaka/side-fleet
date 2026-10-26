//! Whether a task (thread) is waiting on its developer, worked out from the
//! state of its latest run and the events that run already records. Pure, so
//! every transition is unit-tested here; [`crate::inbox`] gathers the facts.

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use sidefleet_core::RunStatus;

/// Why a task is in the inbox. New reasons go at the end; the page shows
/// unknown ones with their `label`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Reason {
    /// The agent finished; the thread waits for a reply.
    Finished,
    /// The run failed.
    Failed,
    /// The machine's account hit its usage limit: failed, or queued until another machine can take it.
    UsageLimit,
    /// The machine running it dropped: the run failed, or it was moved and waits for another machine.
    MachineLost,
    /// The agent asked for permission to use a tool and waits for an answer
    /// (see [`crate::inbox::pending_approvals`]).
    ApprovalNeeded,
}

impl Reason {
    /// Short text for the page and for push notifications.
    pub fn label(self) -> &'static str {
        match self {
            Reason::Finished => "Finished, waiting for your reply",
            Reason::Failed => "Failed",
            Reason::UsageLimit => "Paused on a usage limit",
            Reason::MachineLost => "Its machine dropped",
            Reason::ApprovalNeeded => "Needs your approval",
        }
    }
}

/// What the inbox knows about a thread's latest run (by creation time).
#[derive(Debug, Clone)]
pub struct ThreadFacts {
    pub status: RunStatus,
    /// `agent` or `restore` (see `Run::kind`).
    pub kind: String,
    /// The run replaces one whose machine dropped.
    pub moved: bool,
    /// `reason` of the run's latest `status` event, if any.
    pub status_reason: Option<String>,
    /// When the run's latest `status` event was recorded.
    pub status_at: DateTime<Utc>,
    /// The run hit a usage limit since it last started: when the limit resets.
    pub usage_limit_until: Option<DateTime<Utc>>,
    /// A tool approval waiting on the developer: since when, and what for.
    pub approval: Option<(DateTime<Utc>, String)>,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct Attention {
    pub reason: Reason,
    /// When the task started waiting. It stays in the inbox until the developer
    /// has seen it after this time.
    pub since: DateTime<Utc>,
    /// Extra structured detail (never agent output): when a limit resets, that the
    /// task moved, or which tool call waits for approval (redacted, one line).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub detail: Option<String>,
}

/// Status reasons the control plane writes when a machine stops responding.
const LOST_REASONS: &[&str] = &["worker lost", "worker restarted"];

pub fn compute(f: &ThreadFacts) -> Option<Attention> {
    let at = |reason, detail: Option<String>| Some(Attention { reason, since: f.status_at, detail });
    if let Some((since, what)) = &f.approval {
        return Some(Attention { reason: Reason::ApprovalNeeded, since: *since, detail: Some(what.clone()).filter(|s| !s.is_empty()) });
    }
    let limit = f.usage_limit_until.map(|u| format!("resets {}", u.to_rfc3339_opts(chrono::SecondsFormat::Secs, true)));
    match f.status {
        // Working, or a reply waiting its turn: nothing to do.
        RunStatus::Running => None,
        RunStatus::Queued if f.usage_limit_until.is_some() => at(Reason::UsageLimit, limit),
        RunStatus::Queued if f.moved => at(Reason::MachineLost, Some("moved; waiting for another machine".into())),
        RunStatus::Queued => None,
        // The developer stopped it themselves.
        RunStatus::Stopped => None,
        // Putting files back is quick and asked for; finishing it needs no reply.
        RunStatus::Succeeded if f.kind == "restore" => None,
        RunStatus::Succeeded => at(Reason::Finished, None),
        RunStatus::Failed if f.usage_limit_until.is_some() => at(Reason::UsageLimit, limit),
        RunStatus::Failed if f.status_reason.as_deref().is_some_and(|r| LOST_REASONS.contains(&r)) => at(Reason::MachineLost, None),
        RunStatus::Failed => at(Reason::Failed, None),
    }
}

/// Shown when the developer has not seen the task since it started waiting.
/// `seen` is the later of their last look at this task and their last "clear all".
pub fn unseen(a: &Attention, seen: Option<DateTime<Utc>>) -> bool {
    seen.is_none_or(|s| a.since > s)
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::Duration;

    fn facts(status: RunStatus) -> ThreadFacts {
        ThreadFacts {
            status,
            kind: "agent".into(),
            moved: false,
            status_reason: None,
            status_at: Utc::now(),
            usage_limit_until: None,
            approval: None,
        }
    }

    fn reason(f: &ThreadFacts) -> Option<Reason> {
        compute(f).map(|a| a.reason)
    }

    #[test]
    fn a_task_at_work_needs_nothing() {
        assert_eq!(reason(&facts(RunStatus::Queued)), None);
        assert_eq!(reason(&facts(RunStatus::Running)), None);
    }

    #[test]
    fn finishing_waits_for_a_reply() {
        let f = facts(RunStatus::Succeeded);
        let a = compute(&f).unwrap();
        assert_eq!(a.reason, Reason::Finished);
        assert_eq!(a.since, f.status_at);
    }

    #[test]
    fn replying_takes_it_out_again() {
        // A reply is a new latest run: queued, then running, then finished again.
        let mut f = facts(RunStatus::Succeeded);
        assert!(compute(&f).is_some());
        f.status = RunStatus::Queued;
        assert_eq!(reason(&f), None);
        f.status = RunStatus::Running;
        assert_eq!(reason(&f), None);
        f.status = RunStatus::Succeeded;
        f.status_at += Duration::minutes(3);
        assert_eq!(reason(&f), Some(Reason::Finished));
    }

    #[test]
    fn stopping_it_yourself_is_not_news() {
        assert_eq!(reason(&facts(RunStatus::Stopped)), None);
    }

    #[test]
    fn a_finished_restore_needs_no_reply() {
        let mut f = facts(RunStatus::Succeeded);
        f.kind = "restore".into();
        assert_eq!(reason(&f), None);
        f.status = RunStatus::Failed;
        assert_eq!(reason(&f), Some(Reason::Failed));
    }

    #[test]
    fn failures() {
        assert_eq!(reason(&facts(RunStatus::Failed)), Some(Reason::Failed));
        let mut f = facts(RunStatus::Failed);
        f.status_reason = Some("something else".into());
        assert_eq!(reason(&f), Some(Reason::Failed));
    }

    #[test]
    fn usage_limits_pause_the_task() {
        let until = Utc::now() + Duration::hours(2);
        // Nowhere else to go: the run failed on the limit.
        let mut f = facts(RunStatus::Failed);
        f.usage_limit_until = Some(until);
        let a = compute(&f).unwrap();
        assert_eq!(a.reason, Reason::UsageLimit);
        assert!(a.detail.unwrap().starts_with("resets "));
        // Queued again for another machine: still paused until one takes it.
        f.status = RunStatus::Queued;
        assert_eq!(reason(&f), Some(Reason::UsageLimit));
        // Another machine took it (a new start clears the limit fact).
        f.status = RunStatus::Running;
        f.usage_limit_until = None;
        assert_eq!(reason(&f), None);
    }

    #[test]
    fn a_dropped_machine() {
        for why in LOST_REASONS {
            let mut f = facts(RunStatus::Failed);
            f.status_reason = Some(why.to_string());
            assert_eq!(reason(&f), Some(Reason::MachineLost));
        }
        // Moved: the replacement waits in the queue, then runs elsewhere.
        let mut f = facts(RunStatus::Queued);
        f.moved = true;
        assert_eq!(reason(&f), Some(Reason::MachineLost));
        f.status = RunStatus::Running;
        assert_eq!(reason(&f), None);
        f.status = RunStatus::Succeeded;
        assert_eq!(reason(&f), Some(Reason::Finished));
    }

    #[test]
    fn approvals_come_first_even_while_running() {
        let asked = Utc::now() - Duration::seconds(30);
        let mut f = facts(RunStatus::Running);
        f.approval = Some((asked, "Bash: rm -rf build".into()));
        let a = compute(&f).unwrap();
        assert_eq!(a.reason, Reason::ApprovalNeeded);
        assert_eq!(a.since, asked);
        f.status = RunStatus::Failed;
        assert_eq!(reason(&f), Some(Reason::ApprovalNeeded));
    }

    #[test]
    fn seen_after_it_started_waiting_hides_it() {
        let a = compute(&facts(RunStatus::Succeeded)).unwrap();
        assert!(unseen(&a, None));
        assert!(unseen(&a, Some(a.since - Duration::seconds(1))));
        assert!(!unseen(&a, Some(a.since)));
        assert!(!unseen(&a, Some(a.since + Duration::seconds(1))));
    }

    #[test]
    fn reasons_serialise_as_snake_case() {
        let all = [Reason::Finished, Reason::Failed, Reason::UsageLimit, Reason::MachineLost, Reason::ApprovalNeeded];
        let names: Vec<String> = all.iter().map(|r| serde_json::to_value(r).unwrap().as_str().unwrap().to_string()).collect();
        assert_eq!(names, ["finished", "failed", "usage_limit", "machine_lost", "approval_needed"]);
    }
}
