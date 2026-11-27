//! The agent inventory. Agents Sidefleet runs are discovered: one per org, machine and
//! agent kind, created when the machine registers or claims a run, with no owner until
//! someone claims it. Custom agents register through the API and get an agent key.

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use sidefleet_core::{Agent, RunEvent};
use sqlx::{postgres::PgRow, PgPool, Postgres, Row, Transaction};
use uuid::Uuid;

use super::activity::{self, UserRef};
use super::audit::{self, Actor, Investigate, NewRecord};
use crate::auth::sha256_hex;

/// Agent keys start with this (personal API tokens use `sfa_`).
pub const AGENT_KEY_PREFIX: &str = "sfk_";

#[derive(Debug, Clone)]
pub struct AgentRow {
    pub id: Uuid,
    pub org_id: Uuid,
    pub name: String,
    pub description: String,
    pub owner_id: Option<Uuid>,
    pub owner_login: Option<String>,
    pub team: String,
    pub kind: String,
    pub provider: String,
    pub model: String,
    pub runtime: String,
    pub worker_id: Option<String>,
    pub host: String,
    pub environment: String,
    pub risk: String,
    pub source: String,
    pub reported_status: Option<String>,
    pub key_prefix: Option<String>,
    pub key_live: bool,
    pub suspended_at: Option<DateTime<Utc>>,
    pub suspended_by: Option<String>,
    pub suspend_reason: Option<String>,
    pub created_at: DateTime<Utc>,
    pub last_active_at: Option<DateTime<Utc>>,
    pub pending: i64,
    pub has_running: bool,
}

#[derive(Debug, Clone, Serialize)]
pub struct Suspension {
    pub at: DateTime<Utc>,
    pub by: String,
    pub reason: String,
}

#[derive(Debug, Clone, Serialize)]
pub struct AgentView {
    pub id: Uuid,
    pub name: String,
    pub description: String,
    pub kind: String,
    pub provider: String,
    pub model: String,
    pub runtime: String,
    pub host: String,
    pub worker_id: Option<String>,
    pub environment: String,
    pub team: String,
    pub owner: Option<UserRef>,
    pub risk: String,
    pub source: String,
    pub status: String,
    pub pending_approvals: i64,
    pub credentials: Vec<String>,
    pub can_cancel: bool,
    pub suspension: Option<Suspension>,
    pub created_at: DateTime<Utc>,
    pub last_active_at: Option<DateTime<Utc>>,
}

impl AgentRow {
    pub fn suspended(&self) -> bool {
        self.suspended_at.is_some()
    }

    pub fn status(&self) -> &'static str {
        if self.suspended() {
            "suspended"
        } else if self.pending > 0 {
            "waiting"
        } else if self.has_running {
            "running"
        } else {
            "idle"
        }
    }

    /// Whether Sidefleet runs its process, so a suspension can stop its task.
    pub fn can_cancel(&self) -> bool {
        self.source == "discovered" && self.worker_id.is_some()
    }

    pub fn credentials(&self) -> Vec<String> {
        match (&self.key_prefix, self.key_live) {
            (Some(p), true) => vec![format!("SIDEFLEET_AGENT_KEY ({p}…)")],
            _ => vec![],
        }
    }

    /// `name (id)`, for audit records.
    pub fn label(&self) -> String {
        format!("{} ({})", self.name, self.id)
    }

    /// `riley, Platform` or `Unassigned`.
    pub fn owner_label(&self) -> String {
        match (&self.owner_login, self.team.is_empty()) {
            (Some(l), true) => l.clone(),
            (Some(l), false) => format!("{l}, {}", self.team),
            (None, _) => "Unassigned".into(),
        }
    }

    pub fn view(&self) -> AgentView {
        AgentView {
            id: self.id,
            name: self.name.clone(),
            description: self.description.clone(),
            kind: self.kind.clone(),
            provider: self.provider.clone(),
            model: self.model.clone(),
            runtime: self.runtime.clone(),
            host: self.host.clone(),
            worker_id: self.worker_id.clone(),
            environment: self.environment.clone(),
            team: self.team.clone(),
            owner: self.owner_id.map(|id| UserRef { id, login: self.owner_login.clone().unwrap_or_default() }),
            risk: self.risk.clone(),
            source: self.source.clone(),
            status: self.status().to_string(),
            pending_approvals: self.pending,
            credentials: self.credentials(),
            can_cancel: self.can_cancel(),
            suspension: self.suspended_at.map(|at| Suspension {
                at,
                by: self.suspended_by.clone().unwrap_or_default(),
                reason: self.suspend_reason.clone().unwrap_or_default(),
            }),
            created_at: self.created_at,
            last_active_at: self.last_active_at,
        }
    }
}

const SELECT: &str = "SELECT a.*, u.login AS owner_login,
        (SELECT count(*) FROM approvals p WHERE p.agent_id = a.id AND p.status = 'pending' AND p.expires_at > now()) AS pending,
        -- A registered agent counts as running for 10 minutes after it reports task_started.
        CASE WHEN a.source = 'discovered' THEN EXISTS (
                SELECT 1 FROM runs r WHERE r.org_id = a.org_id AND r.worker_id = a.worker_id AND r.agent = a.kind AND r.status = 'running')
             ELSE a.reported_status = 'running' AND a.last_active_at > now() - make_interval(mins => 10) END AS has_running
   FROM agents a LEFT JOIN users u ON u.id = a.owner_id";

fn row_to_agent(r: &PgRow) -> AgentRow {
    let key_hash: Option<String> = r.get("key_hash");
    let revoked: Option<DateTime<Utc>> = r.get("key_revoked_at");
    AgentRow {
        id: r.get("id"),
        org_id: r.get("org_id"),
        name: r.get("name"),
        description: r.get("description"),
        owner_id: r.get("owner_id"),
        owner_login: r.get("owner_login"),
        team: r.get("team"),
        kind: r.get("kind"),
        provider: r.get("provider"),
        model: r.get("model"),
        runtime: r.get("runtime"),
        worker_id: r.get("worker_id"),
        host: r.get("host"),
        environment: r.get("environment"),
        risk: r.get("risk"),
        source: r.get("source"),
        reported_status: r.get("reported_status"),
        key_prefix: r.get("key_prefix"),
        key_live: key_hash.is_some() && revoked.is_none(),
        suspended_at: r.get("suspended_at"),
        suspended_by: r.get("suspended_by"),
        suspend_reason: r.get("suspend_reason"),
        created_at: r.get("created_at"),
        last_active_at: r.get("last_active_at"),
        pending: r.get("pending"),
        has_running: r.get::<Option<bool>, _>("has_running").unwrap_or(false),
    }
}

pub async fn list(pool: &PgPool, org: Uuid) -> sqlx::Result<Vec<AgentRow>> {
    let rows = sqlx::query(&format!("{SELECT} WHERE a.org_id = $1 ORDER BY a.last_active_at DESC NULLS LAST, a.created_at DESC"))
        .bind(org)
        .fetch_all(pool)
        .await?;
    Ok(rows.iter().map(row_to_agent).collect())
}

pub async fn get<'e, E: sqlx::PgExecutor<'e>>(ex: E, org: Uuid, id: Uuid) -> sqlx::Result<Option<AgentRow>> {
    let row = sqlx::query(&format!("{SELECT} WHERE a.org_id = $1 AND a.id = $2")).bind(org).bind(id).fetch_optional(ex).await?;
    Ok(row.as_ref().map(row_to_agent))
}

/// The agent a machine runs `kind` as, in `org`.
pub async fn discovered(pool: &PgPool, org: Uuid, worker_id: &str, kind: Agent) -> sqlx::Result<Option<AgentRow>> {
    let row = sqlx::query(&format!("{SELECT} WHERE a.org_id = $1 AND a.worker_id = $2 AND a.kind = $3 AND a.source = 'discovered'"))
        .bind(org)
        .bind(worker_id)
        .bind(kind.as_str())
        .fetch_optional(pool)
        .await?;
    Ok(row.as_ref().map(row_to_agent))
}

pub fn provider(kind: Agent) -> &'static str {
    match kind {
        Agent::Claude => "Anthropic",
        Agent::Codex => "OpenAI",
        Agent::Opencode => "Varies by model",
    }
}

/// Make sure `org` has an agent for `kind` on machine `worker_id`; returns its id. A new
/// one is recorded in the audit log. `model` (when not empty) becomes its model.
pub async fn ensure_discovered(pool: &PgPool, org: Uuid, worker_id: &str, kind: Agent, model: &str) -> sqlx::Result<Uuid> {
    let host: String = sqlx::query_scalar("SELECT display_name FROM workers WHERE org_id = $1 AND id = $2")
        .bind(org)
        .bind(worker_id)
        .fetch_optional(pool)
        .await?
        .unwrap_or_else(|| worker_id.to_string());
    if let Some(id) = sqlx::query_scalar::<_, Uuid>(
        "UPDATE agents SET host = $4, model = CASE WHEN $5 <> '' THEN $5 ELSE model END
          WHERE org_id = $1 AND worker_id = $2 AND kind = $3 AND source = 'discovered' RETURNING id",
    )
    .bind(org)
    .bind(worker_id)
    .bind(kind.as_str())
    .bind(&host)
    .bind(model)
    .fetch_optional(pool)
    .await?
    {
        return Ok(id);
    }
    let mut tx = pool.begin().await?;
    let name = format!("{} on {host}", kind.display_name());
    let row = sqlx::query(
        "INSERT INTO agents (id, org_id, name, description, kind, provider, model, runtime, worker_id, host, source)
         VALUES ($1, $2, $3, $4, $5, $6, $7, 'Sidefleet worker', $8, $9, 'discovered')
         ON CONFLICT (org_id, worker_id, kind) WHERE source = 'discovered' DO UPDATE SET host = EXCLUDED.host
         RETURNING id, (xmax = 0) AS inserted",
    )
    .bind(Uuid::new_v4())
    .bind(org)
    .bind(&name)
    .bind(format!("Discovered from {host}. Nobody has claimed it yet."))
    .bind(kind.as_str())
    .bind(provider(kind))
    .bind(model)
    .bind(worker_id)
    .bind(&host)
    .fetch_one(&mut *tx)
    .await?;
    let id: Uuid = row.get("id");
    if row.get::<bool, _>("inserted") {
        audit::append(
            &mut tx,
            org,
            NewRecord {
                actor: Actor::system(),
                event: "agent.registered",
                agent_id: Some(id),
                target: name.clone(),
                detail: format!("Discovered: {} on machine {host}", kind.display_name()),
                data: Investigate {
                    what: "Sidefleet found a new agent on a connected machine and added it to the inventory.".into(),
                    agent: format!("{name} ({id})"),
                    owner: "Unassigned".into(),
                    permissions: "Organisation policies".into(),
                    policy: "Assigned: organisation policies".into(),
                    approval: "Not applicable".into(),
                    decided_by: "Sidefleet".into(),
                    outcome: "Registered; its actions are unattributed until someone claims it".into(),
                },
            },
        )
        .await?;
    }
    tx.commit().await?;
    Ok(id)
}

/// A machine registered: add an agent for each agent kind it has, in each org it serves.
pub async fn discover_machine(pool: &PgPool, orgs: &[Uuid], worker_id: &str, caps: &sidefleet_core::Capabilities) -> sqlx::Result<()> {
    let mut kinds = Vec::new();
    if caps.agents.claude.is_some() {
        kinds.push(Agent::Claude);
    }
    if caps.agents.codex.is_some() {
        kinds.push(Agent::Codex);
    }
    if caps.agents.opencode.is_some() {
        kinds.push(Agent::Opencode);
    }
    for org in orgs {
        for k in &kinds {
            ensure_discovered(pool, *org, worker_id, *k, "").await?;
        }
    }
    Ok(())
}

/// The agent kinds suspended on machine `worker_id` in `org`: it gets no runs for them.
pub async fn suspended_kinds(pool: &PgPool, org: Uuid, worker_id: &str) -> sqlx::Result<Vec<String>> {
    sqlx::query_scalar("SELECT kind FROM agents WHERE org_id = $1 AND worker_id = $2 AND source = 'discovered' AND suspended_at IS NOT NULL")
        .bind(org)
        .bind(worker_id)
        .fetch_all(pool)
        .await
}

pub async fn touch(pool: &PgPool, agent: Uuid) -> sqlx::Result<()> {
    sqlx::query("UPDATE agents SET last_active_at = now() WHERE id = $1").bind(agent).execute(pool).await?;
    Ok(())
}

// ---- registration and changes ----

#[derive(Debug, Clone, Default, Deserialize)]
pub struct AgentInput {
    pub name: Option<String>,
    pub description: Option<String>,
    pub kind: Option<String>,
    pub provider: Option<String>,
    pub model: Option<String>,
    pub runtime: Option<String>,
    pub host: Option<String>,
    pub environment: Option<String>,
    pub team: Option<String>,
    pub risk: Option<String>,
    pub owner_id: Option<Uuid>,
}

fn cut(s: &Option<String>, n: usize) -> Option<String> {
    s.as_ref().map(|v| v.trim().chars().take(n).collect())
}

pub fn check_risk(r: &str) -> Result<(), String> {
    if matches!(r, "low" | "medium" | "high" | "unknown") {
        Ok(())
    } else {
        Err("risk must be low, medium, high or unknown".into())
    }
}

pub fn new_key() -> String {
    format!("{AGENT_KEY_PREFIX}{}", crate::tenancy::random_hex(24))
}

/// Register a custom agent. Returns it and its key, shown once.
pub async fn register(pool: &PgPool, org: Uuid, input: &AgentInput, owner: Option<Uuid>, actor: &Actor) -> Result<(AgentRow, String), RegisterError> {
    let name = cut(&input.name, 120).unwrap_or_default();
    if name.is_empty() {
        return Err(RegisterError::Bad("an agent needs a name".into()));
    }
    let kind = cut(&input.kind, 20).unwrap_or_else(|| "custom".into());
    if !matches!(kind.as_str(), "claude" | "codex" | "opencode" | "custom") {
        return Err(RegisterError::Bad("kind must be claude, codex, opencode or custom".into()));
    }
    let risk = cut(&input.risk, 20).unwrap_or_else(|| "unknown".into());
    check_risk(&risk).map_err(RegisterError::Bad)?;
    let owner = input.owner_id.or(owner);
    if let Some(o) = owner {
        let member: bool = sqlx::query_scalar("SELECT EXISTS (SELECT 1 FROM org_members WHERE org_id = $1 AND user_id = $2)").bind(org).bind(o).fetch_one(pool).await?;
        if !member {
            return Err(RegisterError::Bad("the owner must be a member of this org".into()));
        }
    }
    let key = new_key();
    let id = Uuid::new_v4();
    let mut tx = pool.begin().await?;
    sqlx::query(
        "INSERT INTO agents (id, org_id, name, description, owner_id, team, kind, provider, model, runtime, host, environment, risk, source, key_hash, key_prefix)
         VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10, $11, $12, $13, 'registered', $14, $15)",
    )
    .bind(id)
    .bind(org)
    .bind(&name)
    .bind(cut(&input.description, 1000).unwrap_or_default())
    .bind(owner)
    .bind(cut(&input.team, 80).unwrap_or_default())
    .bind(&kind)
    .bind(cut(&input.provider, 80).unwrap_or_default())
    .bind(cut(&input.model, 120).unwrap_or_default())
    .bind(cut(&input.runtime, 80).filter(|r| !r.is_empty()).unwrap_or_else(|| "External".into()))
    .bind(cut(&input.host, 120).unwrap_or_default())
    .bind(cut(&input.environment, 120).unwrap_or_default())
    .bind(&risk)
    .bind(sha256_hex(&key))
    .bind(&key[..AGENT_KEY_PREFIX.len() + 4])
    .execute(&mut *tx)
    .await?;
    let agent = get(&mut *tx, org, id).await?.ok_or(sqlx::Error::RowNotFound)?;
    audit::append(
        &mut tx,
        org,
        NewRecord {
            actor: actor.clone(),
            event: "agent.registered",
            agent_id: Some(id),
            target: name.clone(),
            detail: format!("{} agent on {}", agent.runtime, if agent.host.is_empty() { "an unnamed host" } else { &agent.host }),
            data: Investigate {
                what: "A new agent was registered through the API.".into(),
                agent: agent.label(),
                owner: agent.owner_label(),
                permissions: "Organisation policies".into(),
                policy: "Assigned: organisation policies".into(),
                approval: "Not applicable".into(),
                decided_by: actor.name.clone(),
                outcome: "Registered and active; an agent key was issued".into(),
            },
        },
    )
    .await?;
    tx.commit().await?;
    Ok((agent, key))
}

pub enum RegisterError {
    Bad(String),
    Db(sqlx::Error),
}

impl From<sqlx::Error> for RegisterError {
    fn from(e: sqlx::Error) -> Self {
        RegisterError::Db(e)
    }
}

/// Apply `input` to an agent. Returns the new row; audits what changed.
pub async fn update(pool: &PgPool, org: Uuid, current: &AgentRow, input: &AgentInput, actor: &Actor) -> Result<AgentRow, RegisterError> {
    if let Some(r) = &input.risk {
        check_risk(r.trim()).map_err(RegisterError::Bad)?;
    }
    if let Some(o) = input.owner_id {
        let member: bool = sqlx::query_scalar("SELECT EXISTS (SELECT 1 FROM org_members WHERE org_id = $1 AND user_id = $2)").bind(org).bind(o).fetch_one(pool).await?;
        if !member {
            return Err(RegisterError::Bad("the owner must be a member of this org".into()));
        }
    }
    let name = cut(&input.name, 120).filter(|n| !n.is_empty());
    let team = cut(&input.team, 80);
    let mut tx = pool.begin().await?;
    sqlx::query(
        "UPDATE agents SET name = COALESCE($3, name), description = COALESCE($4, description), team = COALESCE($5, team),
                environment = COALESCE($6, environment), risk = COALESCE($7, risk), owner_id = COALESCE($8, owner_id),
                model = COALESCE($9, model), provider = COALESCE($10, provider), host = COALESCE($11, host)
          WHERE org_id = $1 AND id = $2",
    )
    .bind(org)
    .bind(current.id)
    .bind(&name)
    .bind(cut(&input.description, 1000))
    .bind(&team)
    .bind(cut(&input.environment, 120))
    .bind(cut(&input.risk, 20))
    .bind(input.owner_id)
    .bind(cut(&input.model, 120).filter(|_| current.source == "registered"))
    .bind(cut(&input.provider, 80).filter(|_| current.source == "registered"))
    .bind(cut(&input.host, 120).filter(|_| current.source == "registered"))
    .execute(&mut *tx)
    .await?;
    let after = get(&mut *tx, org, current.id).await?.ok_or(sqlx::Error::RowNotFound)?;
    let mut changed = Vec::new();
    if after.name != current.name {
        changed.push(format!("renamed from {}", current.name));
    }
    if after.owner_id != current.owner_id {
        changed.push(format!("owner {}", after.owner_login.clone().unwrap_or_default()));
    }
    if after.risk != current.risk {
        changed.push(format!("risk {} → {}", current.risk, after.risk));
    }
    if after.environment != current.environment {
        changed.push(format!("environment {}", after.environment));
    }
    if after.description != current.description || after.model != current.model || after.provider != current.provider || after.host != current.host {
        changed.push("details".into());
    }
    let team_changed = after.team != current.team;
    if !changed.is_empty() {
        let owner_changed = after.owner_id != current.owner_id;
        audit::append(
            &mut tx,
            org,
            NewRecord {
                actor: actor.clone(),
                event: if owner_changed { "agent.owner_assigned" } else { "agent.updated" },
                agent_id: Some(current.id),
                target: after.name.clone(),
                detail: changed.join(", "),
                data: Investigate {
                    what: if owner_changed { "The agent got a new owner.".into() } else { "The agent's details changed.".into() },
                    agent: after.label(),
                    owner: after.owner_label(),
                    permissions: "Unchanged".into(),
                    policy: "None (administrative action)".into(),
                    approval: "Not applicable".into(),
                    decided_by: actor.name.clone(),
                    outcome: if owner_changed { "Its actions are attributed to the new owner".into() } else { "Saved".into() },
                },
            },
        )
        .await?;
    }
    if team_changed {
        let to = if after.team.is_empty() { "no team".to_string() } else { format!("team {}", after.team) };
        audit::append(
            &mut tx,
            org,
            NewRecord {
                actor: actor.clone(),
                event: "permission.changed",
                agent_id: Some(current.id),
                target: after.name.clone(),
                detail: format!("Moved to {to}"),
                data: Investigate {
                    what: "The agent moved team, which changes the team policies that apply to it.".into(),
                    agent: after.label(),
                    owner: after.owner_label(),
                    permissions: format!("Organisation policies plus those of {to}"),
                    policy: "Team policies re-evaluated".into(),
                    approval: "Not applicable".into(),
                    decided_by: actor.name.clone(),
                    outcome: "Live from the next tool call".into(),
                },
            },
        )
        .await?;
    }
    tx.commit().await?;
    Ok(after)
}

/// Issue a new key for a registered agent, revoking the old one.
pub async fn reissue_key(pool: &PgPool, org: Uuid, agent: &AgentRow, actor: &Actor) -> sqlx::Result<String> {
    let key = new_key();
    let mut tx = pool.begin().await?;
    sqlx::query("UPDATE agents SET key_hash = $3, key_prefix = $4, key_revoked_at = NULL WHERE org_id = $1 AND id = $2")
        .bind(org)
        .bind(agent.id)
        .bind(sha256_hex(&key))
        .bind(&key[..AGENT_KEY_PREFIX.len() + 4])
        .execute(&mut *tx)
        .await?;
    audit::append(
        &mut tx,
        org,
        NewRecord {
            actor: actor.clone(),
            event: "credential.issued",
            agent_id: Some(agent.id),
            target: agent.name.clone(),
            detail: format!("New SIDEFLEET_AGENT_KEY ({}…)", &key[..AGENT_KEY_PREFIX.len() + 4]),
            data: Investigate {
                what: "A new agent key was issued; the previous one stops working.".into(),
                agent: agent.label(),
                owner: agent.owner_label(),
                permissions: "Unchanged".into(),
                policy: "None (administrative action)".into(),
                approval: "Not applicable".into(),
                decided_by: actor.name.clone(),
                outcome: "The agent can reach the gateway with the new key".into(),
            },
        },
    )
    .await?;
    tx.commit().await?;
    Ok(key)
}

/// The agent behind an agent key, if the key is live.
pub async fn by_key(pool: &PgPool, key: &str) -> sqlx::Result<Option<AgentRow>> {
    let row = sqlx::query(&format!("{SELECT} WHERE a.key_hash = $1 AND a.key_revoked_at IS NULL"))
        .bind(sha256_hex(key))
        .fetch_optional(pool)
        .await?;
    Ok(row.as_ref().map(row_to_agent))
}

/// Registered agents report what they're doing through the gateway.
pub async fn report_status(pool: &PgPool, agent: Uuid, status: Option<&str>) -> sqlx::Result<()> {
    sqlx::query("UPDATE agents SET last_active_at = now(), reported_status = COALESCE($2, reported_status) WHERE id = $1")
        .bind(agent)
        .bind(status)
        .execute(pool)
        .await?;
    Ok(())
}

// ---- suspension ----

#[derive(Debug, Clone, Serialize)]
pub struct Incident {
    pub id: Uuid,
    pub suspended_at: DateTime<Utc>,
    pub suspended_by: String,
    pub reason: String,
    pub revoked: Vec<String>,
    pub cancelled_runs: i32,
    pub cancelled_approvals: i32,
    /// Why a running process was not stopped, when it wasn't.
    pub not_stopped: Option<String>,
    pub reactivated_at: Option<DateTime<Utc>>,
    pub reactivated_by: Option<String>,
}

fn row_to_incident(r: &PgRow) -> Incident {
    Incident {
        id: r.get("id"),
        suspended_at: r.get("suspended_at"),
        suspended_by: r.get("suspended_by"),
        reason: r.get("reason"),
        revoked: r.get("revoked"),
        cancelled_runs: r.get("cancelled_runs"),
        cancelled_approvals: r.get("cancelled_approvals"),
        not_stopped: r.get("not_stopped"),
        reactivated_at: r.get("reactivated_at"),
        reactivated_by: r.get("reactivated_by"),
    }
}

pub async fn incidents(pool: &PgPool, org: Uuid, agent: Uuid) -> sqlx::Result<Vec<Incident>> {
    let rows = sqlx::query("SELECT * FROM agent_incidents WHERE org_id = $1 AND agent_id = $2 ORDER BY suspended_at DESC")
        .bind(org)
        .bind(agent)
        .fetch_all(pool)
        .await?;
    Ok(rows.iter().map(row_to_incident).collect())
}

#[derive(Debug, Clone, Deserialize)]
pub struct SuspendRequest {
    #[serde(default)]
    pub reason: String,
    #[serde(default = "yes")]
    pub revoke_credentials: bool,
    #[serde(default = "yes")]
    pub cancel_running: bool,
}

fn yes() -> bool {
    true
}

pub enum SuspendOutcome {
    Suspended(Box<AgentRow>, Incident, Vec<RunEvent>),
    NotFound,
    AlreadySuspended,
}

/// Suspend an agent: block it at the gateway, revoke its key, cancel its pending
/// approvals and stop its running tasks where Sidefleet runs them. Returns the run events
/// to publish.
pub async fn suspend(pool: &PgPool, org: Uuid, id: Uuid, req: &SuspendRequest, actor: &Actor) -> sqlx::Result<SuspendOutcome> {
    let reason: String = {
        let r = req.reason.trim();
        if r.is_empty() { "No reason given.".to_string() } else { r.chars().take(1000).collect() }
    };
    // 1. Block it, revoke its key and open the incident, in one go.
    let mut tx = pool.begin().await?;
    let locked: Option<Option<DateTime<Utc>>> = sqlx::query_scalar("SELECT suspended_at FROM agents WHERE org_id = $1 AND id = $2 FOR UPDATE")
        .bind(org)
        .bind(id)
        .fetch_optional(&mut *tx)
        .await?;
    match locked {
        None => return Ok(SuspendOutcome::NotFound),
        Some(Some(_)) => return Ok(SuspendOutcome::AlreadySuspended),
        Some(None) => {}
    }
    let before = get(&mut *tx, org, id).await?.ok_or(sqlx::Error::RowNotFound)?;
    let revoked = if req.revoke_credentials { before.credentials() } else { vec![] };
    sqlx::query(
        "UPDATE agents SET suspended_at = now(), suspended_by = $3, suspend_reason = $4,
                key_revoked_at = CASE WHEN $5 AND key_hash IS NOT NULL THEN now() ELSE key_revoked_at END
          WHERE org_id = $1 AND id = $2",
    )
    .bind(org)
    .bind(id)
    .bind(&actor.name)
    .bind(&reason)
    .bind(req.revoke_credentials)
    .execute(&mut *tx)
    .await?;
    let incident_id = Uuid::new_v4();
    sqlx::query("INSERT INTO agent_incidents (id, org_id, agent_id, suspended_by, reason, revoked) VALUES ($1, $2, $3, $4, $5, $6)")
        .bind(incident_id)
        .bind(org)
        .bind(id)
        .bind(&actor.name)
        .bind(&reason)
        .bind(&revoked)
        .execute(&mut *tx)
        .await?;
    activity::record(
        &mut *tx,
        org,
        activity::NewEvent {
            agent_id: Some(id),
            kind: "agent_suspended",
            tool: "admin".into(),
            action: format!("{}: {reason}", actor.name),
            outcome: "suspended",
            ..Default::default()
        },
    )
    .await?;
    audit::append(
        &mut tx,
        org,
        NewRecord {
            actor: actor.clone(),
            event: "agent.suspended",
            agent_id: Some(id),
            target: before.name.clone(),
            detail: reason.clone(),
            data: Investigate {
                what: format!("{} suspended the agent.", actor.name),
                agent: before.label(),
                owner: before.owner_label(),
                permissions: "Every call now denied: agent suspended".into(),
                policy: "None (administrative action)".into(),
                approval: "Not applicable".into(),
                decided_by: actor.name.clone(),
                outcome: "New tasks and tool calls blocked at the gateway".into(),
            },
        },
    )
    .await?;
    if !revoked.is_empty() {
        audit::append(
            &mut tx,
            org,
            NewRecord {
                actor: actor.clone(),
                event: "credential.revoked",
                agent_id: Some(id),
                target: before.name.clone(),
                detail: format!("{} revoked", revoked.join(", ")),
                data: Investigate {
                    what: "Sidefleet-managed credentials were revoked as part of a suspension.".into(),
                    agent: before.label(),
                    owner: before.owner_label(),
                    permissions: "All access withdrawn".into(),
                    policy: "None (administrative action)".into(),
                    approval: "Not applicable".into(),
                    decided_by: actor.name.clone(),
                    outcome: "Invalid from now; reissue to use again".into(),
                },
            },
        )
        .await?;
    }
    tx.commit().await?;

    // 2. Its pending approvals end, each under its own run's lock.
    let mut events = Vec::new();
    let cancelled = crate::approvals::cancel_for_agent(pool, org, id, "The agent was suspended.").await?;
    let cancelled_approvals = cancelled.0;
    events.extend(cancelled.1);

    // 3. Stop what it is running, where Sidefleet runs it.
    let mut cancelled_runs = 0;
    let mut not_stopped = None;
    let running: Vec<Uuid> = match (&before.worker_id, before.source.as_str()) {
        (Some(w), "discovered") => {
            sqlx::query_scalar("SELECT id FROM runs WHERE org_id = $1 AND worker_id = $2 AND agent = $3 AND status = 'running'")
                .bind(org)
                .bind(w)
                .bind(&before.kind)
                .fetch_all(pool)
                .await?
        }
        _ => vec![],
    };
    if !before.can_cancel() {
        if before.has_running {
            not_stopped = Some(format!(
                "Not possible: {} runs outside a Sidefleet worker. Its next call through the gateway is denied, but the process keeps running until someone stops it{}.",
                before.name,
                if before.host.is_empty() { String::new() } else { format!(" on {}", before.host) }
            ));
        }
    } else if !req.cancel_running {
        if !running.is_empty() {
            not_stopped = Some(format!("Left running by choice: {} task(s). Their next tool call is denied.", running.len()));
        }
    } else {
        for run_id in &running {
            if let Some((_, ev)) = crate::db::request_stop(pool, org, *run_id).await? {
                cancelled_runs += 1;
                events.extend(ev);
                audit::append_now(
                    pool,
                    org,
                    NewRecord {
                        actor: actor.clone(),
                        event: "task.cancelled",
                        agent_id: Some(id),
                        target: before.name.clone(),
                        detail: format!("Stop sent to the worker on {} for run {run_id}", before.host),
                        data: Investigate {
                            what: "The running task was cancelled through the Sidefleet worker.".into(),
                            agent: before.label(),
                            owner: before.owner_label(),
                            permissions: "Not applicable".into(),
                            policy: "None (administrative action)".into(),
                            approval: "Not applicable".into(),
                            decided_by: actor.name.clone(),
                            outcome: "The worker stops the agent: SIGTERM, then SIGKILL".into(),
                        },
                    },
                )
                .await?;
            }
        }
    }
    let row = sqlx::query("UPDATE agent_incidents SET cancelled_runs = $2, cancelled_approvals = $3, not_stopped = $4 WHERE id = $1 RETURNING *")
        .bind(incident_id)
        .bind(cancelled_runs)
        .bind(cancelled_approvals)
        .bind(&not_stopped)
        .fetch_one(pool)
        .await?;
    let after = get(pool, org, id).await?.ok_or(sqlx::Error::RowNotFound)?;
    Ok(SuspendOutcome::Suspended(Box::new(after), row_to_incident(&row), events))
}

pub enum ReactivateOutcome {
    Reactivated(Box<AgentRow>),
    NotFound,
    NotSuspended,
}

pub async fn reactivate(pool: &PgPool, org: Uuid, id: Uuid, actor: &Actor) -> sqlx::Result<ReactivateOutcome> {
    let mut tx = pool.begin().await?;
    let Some(before) = get(&mut *tx, org, id).await? else { return Ok(ReactivateOutcome::NotFound) };
    let n = sqlx::query("UPDATE agents SET suspended_at = NULL, suspended_by = NULL, suspend_reason = NULL WHERE org_id = $1 AND id = $2 AND suspended_at IS NOT NULL")
        .bind(org)
        .bind(id)
        .execute(&mut *tx)
        .await?
        .rows_affected();
    if n == 0 {
        return Ok(ReactivateOutcome::NotSuspended);
    }
    sqlx::query("UPDATE agent_incidents SET reactivated_at = now(), reactivated_by = $3 WHERE org_id = $1 AND agent_id = $2 AND reactivated_at IS NULL")
        .bind(org)
        .bind(id)
        .bind(&actor.name)
        .execute(&mut *tx)
        .await?;
    activity::record(
        &mut *tx,
        org,
        activity::NewEvent { agent_id: Some(id), kind: "agent_reactivated", tool: "admin".into(), action: actor.name.clone(), outcome: "reactivated", ..Default::default() },
    )
    .await?;
    let keys_note = if before.key_prefix.is_some() && !before.key_live { "; its revoked key must be reissued" } else { "" };
    audit::append(
        &mut tx,
        org,
        NewRecord {
            actor: actor.clone(),
            event: "agent.reactivated",
            agent_id: Some(id),
            target: before.name.clone(),
            detail: format!("Suspension lifted{keys_note}"),
            data: Investigate {
                what: format!("{} reactivated the agent.", actor.name),
                agent: before.label(),
                owner: before.owner_label(),
                permissions: "Back to its assigned policies".into(),
                policy: "None (administrative action)".into(),
                approval: "Not applicable".into(),
                decided_by: actor.name.clone(),
                outcome: format!("Can take new tasks{keys_note}"),
            },
        },
    )
    .await?;
    tx.commit().await?;
    let after = get(pool, org, id).await?.ok_or(sqlx::Error::RowNotFound)?;
    Ok(ReactivateOutcome::Reactivated(Box::new(after)))
}

/// The detail page's extra parts.
pub async fn detail(pool: &PgPool, org: Uuid, agent: &AgentRow) -> sqlx::Result<Value> {
    use sidefleet_core::policy::{classify, evaluate, PERMISSION_SAMPLES};
    let policies = super::policies::for_agent(pool, org, Some(agent)).await?;
    let rules: Vec<_> = policies.iter().map(|p| p.rule()).collect();
    let permissions: Vec<Value> = PERMISSION_SAMPLES
        .iter()
        .map(|(label, shown, tool, target)| {
            if agent.suspended() {
                return json!({"action": label, "tool": shown, "decision": "deny", "policy": null, "source": "Agent suspended"});
            }
            let e = evaluate(&rules, &classify(tool, &json!(target), None));
            let p = e.rule.and_then(|id| policies.iter().find(|p| p.id == id));
            json!({
                "action": label, "tool": shown, "decision": e.decision.as_str(),
                "policy": p.map(|p| json!({"id": p.id, "name": p.name})),
                "source": p.map_or_else(|| "No matching policy (denied)".to_string(), |p| p.name.clone()),
            })
        })
        .collect();
    let assigned: Vec<Value> = policies.iter().map(|p| json!({"id": p.id, "name": p.name, "priority": p.priority, "scope": p.scope, "decision": p.decision})).collect();
    Ok(json!({
        "agent": agent.view(),
        "permissions": permissions,
        "policies": assigned,
        "incidents": incidents(pool, org, agent.id).await?,
    }))
}

/// For audit records written where only the agent's id is at hand.
pub async fn labels(tx: &mut Transaction<'_, Postgres>, org: Uuid, id: Option<Uuid>) -> sqlx::Result<(String, String, String)> {
    let Some(id) = id else { return Ok(("Unknown agent".into(), "Unassigned".into(), String::new())) };
    Ok(match get(&mut **tx, org, id).await? {
        Some(a) => (a.label(), a.owner_label(), a.name),
        None => (id.to_string(), "Unassigned".into(), id.to_string()),
    })
}
