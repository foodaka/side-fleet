//! The execution gateway: every tool call a governed agent makes is checked here before
//! it runs. Sidefleet's worker asks for Claude Code's calls (a `PreToolUse` hook), custom
//! agents ask with their agent key. Each decision is recorded; a denied or held call is
//! also written to the audit log.

use serde::Deserialize;
use serde_json::{json, Value};
use sidefleet_core::policy::{action_digest, classify, evaluate, Action, Decision};
use sidefleet_core::{redact_tool_input, ApprovalRequest, Run, RunEvent};
use sqlx::PgPool;
use uuid::Uuid;

use super::activity::{self, NewEvent};
use super::agents::AgentRow;
use super::audit::{self, Actor, Investigate, NewRecord};
use super::policies::{self, Policy};
use crate::approvals::{self, Hold, RequestOutcome};

#[derive(Debug, Clone, Default, Deserialize)]
pub struct CheckRequest {
    /// The agent's own tool (`Bash`), or an action tool (`secrets.read`).
    #[serde(default)]
    pub tool_name: Option<String>,
    #[serde(default)]
    pub input: Option<Value>,
    /// The action form: `tool` (`shell.exec`) and `action` (its target).
    #[serde(default)]
    pub tool: Option<String>,
    #[serde(default)]
    pub action: Option<String>,
    #[serde(default)]
    pub tool_use_id: Option<String>,
    /// The task's worktree, to tell writes inside it from writes elsewhere.
    #[serde(default)]
    pub workdir: Option<String>,
    #[serde(default)]
    pub context: Option<Value>,
    #[serde(default)]
    pub timeout_secs: Option<u64>,
    #[serde(default)]
    pub agent_id: Option<Uuid>,
}

impl CheckRequest {
    /// The tool name and input to classify and bind an approval to.
    pub fn call(&self) -> Option<(String, Value)> {
        if let Some(t) = self.tool_name.as_deref().map(str::trim).filter(|t| !t.is_empty()) {
            return Some((t.chars().take(200).collect(), self.input.clone().unwrap_or(json!({}))));
        }
        let tool = self.tool.as_deref().map(str::trim).filter(|t| !t.is_empty())?;
        Some((tool.chars().take(200).collect(), json!(self.action.clone().unwrap_or_default())))
    }
}

/// What the engine decided, before anything acts on it.
pub struct Evaluated {
    pub decision: Decision,
    pub policy: Option<Policy>,
    pub overridden: Vec<Policy>,
    pub actions: Vec<Action>,
    /// The action that decided.
    pub action: Action,
    pub reason: String,
    pub elapsed_ms: f64,
}

/// Decide a call for `agent` (or for any agent, under org policies only).
pub async fn evaluate_call(pool: &PgPool, org: Uuid, agent: Option<&AgentRow>, tool_name: &str, input: &Value, workdir: Option<&str>) -> sqlx::Result<Evaluated> {
    let started = std::time::Instant::now();
    let actions = classify(tool_name, input, workdir);
    if let Some(a) = agent.filter(|a| a.suspended()) {
        return Ok(Evaluated {
            decision: Decision::Deny,
            policy: None,
            overridden: vec![],
            action: actions[0].clone(),
            actions,
            reason: format!("Sidefleet refused this call: the agent is suspended (agent suspended). {}", a.suspend_reason.clone().unwrap_or_default()).trim().to_string(),
            elapsed_ms: started.elapsed().as_secs_f64() * 1000.0,
        });
    }
    let applicable = policies::for_agent(pool, org, agent).await?;
    let rules: Vec<_> = applicable.iter().map(Policy::rule).collect();
    let e = evaluate(&rules, &actions);
    let policy = e.rule.and_then(|id| applicable.iter().find(|p| p.id == id).cloned());
    let overridden = e.matched.iter().filter(|id| Some(**id) != e.rule).filter_map(|id| applicable.iter().find(|p| p.id == *id).cloned()).collect();
    let reason = match (&policy, e.decision) {
        (Some(p), Decision::Deny) => format!("Denied by the Sidefleet policy \"{}\". {}", p.name, p.description).trim().to_string(),
        (None, _) => "No Sidefleet policy allows this action, so it is denied.".to_string(),
        (Some(p), Decision::RequireApproval) => format!("The Sidefleet policy \"{}\" needs someone to approve this action.", p.name),
        (Some(p), Decision::Allow) => format!("Allowed by the Sidefleet policy \"{}\".", p.name),
    };
    let action = actions.get(e.action).cloned().unwrap_or_else(|| actions[0].clone());
    Ok(Evaluated { decision: e.decision, policy, overridden, actions, action, reason, elapsed_ms: started.elapsed().as_secs_f64() * 1000.0 })
}

pub fn policy_ref(p: &Policy) -> Value {
    json!({"id": p.id, "name": p.name, "priority": p.priority, "scope": p.scope})
}

/// The answer the dry-run evaluator gives.
pub fn evaluated_json(e: &Evaluated) -> Value {
    json!({
        "decision": e.decision.as_str(),
        "policy": e.policy.as_ref().map(policy_ref),
        "overridden": e.overridden.iter().map(|p| json!({"id": p.id, "name": p.name, "priority": p.priority, "decision": p.decision})).collect::<Vec<_>>(),
        "actions": e.actions,
        "reason": e.reason,
        "elapsed_ms": (e.elapsed_ms * 100.0).round() / 100.0,
    })
}

/// `{task, repo, branch}` of a run, for approvals and activity.
pub fn run_context(run: &Run) -> Value {
    let root = run.parent_id.unwrap_or(run.id);
    let branch = (run.isolation == "worktree").then(|| format!("sidefleet/{}", &root.simple().to_string()[..8]));
    json!({"task": run.task.lines().next().unwrap_or_default().chars().take(200).collect::<String>(), "repo": run.repo, "branch": branch})
}

pub struct Checked {
    pub body: Value,
    pub events: Vec<RunEvent>,
}

/// Check one call for real: decide, record, and for `require_approval` create the approval
/// (on `run` when there is one). `worker_id` is the machine running `run`.
pub async fn check(pool: &PgPool, org: Uuid, agent: &AgentRow, run: Option<(&Run, &str)>, req: &CheckRequest) -> sqlx::Result<Option<Checked>> {
    let Some((tool_name, raw)) = req.call() else { return Ok(None) };
    let input = redact_tool_input(&raw, approvals::MAX_INPUT_STRING_BYTES);
    let digest = action_digest(&tool_name, &input);
    let e = evaluate_call(pool, org, Some(agent), &tool_name, &input, req.workdir.as_deref()).await?;
    let context = match (run, &req.context) {
        (Some((r, _)), _) => run_context(r),
        (None, Some(c)) if c.is_object() => c.clone(),
        _ => json!({}),
    };
    let command = approvals::command_text(&tool_name, &input);
    // The activity row shows the tool beside it: an action named by a custom agent is just its target.
    let shown = match &input {
        Value::String(target) => target.clone(),
        _ => command.clone(),
    };
    let policy_pair = e.policy.as_ref().map(|p| (p.id, p.name.clone()));
    let mut events = Vec::new();
    let mut decision = e.decision;
    let mut reason = e.reason.clone();
    let mut approval = Value::Null;
    let metadata = json!({"elapsed_ms": e.elapsed_ms, "digest": digest, "tool_name": tool_name, "tool_use_id": req.tool_use_id,
                          "actions": e.actions, "overridden": e.overridden.iter().map(|p| p.name.clone()).collect::<Vec<_>>()});
    match e.decision {
        Decision::Allow | Decision::Deny => {
            let mut tx = pool.begin().await?;
            activity::record(
                &mut *tx,
                org,
                NewEvent {
                    agent_id: Some(agent.id),
                    run_id: run.map(|(r, _)| r.id),
                    kind: "tool_call",
                    tool: e.action.tool.clone(),
                    action: shown.clone(),
                    context: context.clone(),
                    outcome: if e.decision == Decision::Allow { "allowed" } else { "denied" },
                    policy: policy_pair.clone(),
                    decision: Some(e.decision.as_str()),
                    metadata: metadata.clone(),
                },
            )
            .await?;
            if e.decision == Decision::Deny {
                audit::append(
                    &mut tx,
                    org,
                    NewRecord {
                        actor: Actor::agent(&agent.name),
                        event: "policy.denied",
                        agent_id: Some(agent.id),
                        target: agent.name.clone(),
                        detail: command.chars().take(200).collect(),
                        data: Investigate {
                            what: format!("The agent tried {} ({}). The gateway refused it.", e.action.tool, tool_name),
                            agent: agent.label(),
                            owner: agent.owner_label(),
                            permissions: if agent.suspended() {
                                "Every call denied: agent suspended".into()
                            } else {
                                format!("{}: denied", e.policy.as_ref().map_or("No matching policy", |p| p.category.as_str()))
                            },
                            policy: e.policy.as_ref().map_or_else(
                                || if agent.suspended() { "None (agent suspended)".into() } else { "None matched: default deny".into() },
                                |p| format!("{} ({})", p.name, p.scope_label()),
                            ),
                            approval: "Not applicable, denied outright".into(),
                            decided_by: "Policy engine".into(),
                            outcome: "Blocked before execution".into(),
                        },
                    },
                )
                .await?;
            }
            tx.commit().await?;
        }
        Decision::RequireApproval => {
            let p = e.policy.as_ref();
            let hold = Hold {
                kind: "policy",
                agent_id: Some(agent.id),
                policy_id: p.map(|p| p.id),
                policy_name: p.map(|p| p.name.clone()),
                policy_scope: p.map(|p| p.scope_label()),
                digest: Some(digest.clone()),
                action: Some(e.action.tool.clone()),
                context: context.clone(),
            };
            match run {
                Some((r, worker_id)) => {
                    let ask = ApprovalRequest { tool_name: tool_name.clone(), input: input.clone(), tool_use_id: req.tool_use_id.clone(), timeout_secs: req.timeout_secs };
                    match approvals::request(pool, org, r.id, worker_id, &ask, &hold).await? {
                        RequestOutcome::Created(a, ev) => {
                            events.push(ev);
                            approval = serde_json::to_value(&*a).unwrap_or(Value::Null);
                        }
                        _ => {
                            decision = Decision::Deny;
                            reason = "The run is not running, so nothing can be approved.".into();
                        }
                    }
                }
                None => {
                    let a = approvals::request_detached(pool, org, &tool_name, &input, req.timeout_secs, &hold).await?;
                    approval = serde_json::to_value(&a).unwrap_or(Value::Null);
                }
            }
        }
    }
    super::agents::touch(pool, agent.id).await?;
    Ok(Some(Checked {
        body: json!({
            "decision": decision.as_str(),
            "policy": e.policy.as_ref().map(policy_ref),
            "reason": reason,
            "digest": digest,
            "actions": e.actions,
            "approval": approval,
        }),
        events,
    }))
}
