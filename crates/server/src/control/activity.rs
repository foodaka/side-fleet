//! Agent activity: tasks starting and ending, tool calls and the decisions about them,
//! approvals and suspensions, for the Activity page and the dashboard. Everything goes
//! through redaction before it is stored.

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use sqlx::{postgres::PgRow, PgExecutor, PgPool, Row};
use uuid::Uuid;

/// An event to record. `policy` is the policy that decided, if any.
#[derive(Debug, Clone, Default)]
pub struct NewEvent {
    pub agent_id: Option<Uuid>,
    pub run_id: Option<Uuid>,
    pub kind: &'static str,
    pub tool: String,
    pub action: String,
    pub context: Value,
    pub outcome: &'static str,
    pub policy: Option<(Uuid, String)>,
    pub decision: Option<&'static str>,
    pub metadata: Value,
}

pub async fn record<'e, E: PgExecutor<'e>>(ex: E, org: Uuid, ev: NewEvent) -> sqlx::Result<()> {
    let action: String = sidefleet_core::redact(&ev.action).chars().take(2000).collect();
    let empty = json!({});
    let context = sidefleet_core::redact::redact_json(if ev.context.is_null() { &empty } else { &ev.context });
    let metadata = sidefleet_core::redact::redact_json(if ev.metadata.is_null() { &empty } else { &ev.metadata });
    sqlx::query(
        "INSERT INTO agent_events (id, org_id, agent_id, run_id, type, tool, action, context, outcome, policy_id, policy_name, decision, metadata)
         VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10, $11, $12, $13)",
    )
    .bind(Uuid::new_v4())
    .bind(org)
    .bind(ev.agent_id)
    .bind(ev.run_id)
    .bind(ev.kind)
    .bind(ev.tool.chars().take(200).collect::<String>())
    .bind(action)
    .bind(context)
    .bind(ev.outcome)
    .bind(ev.policy.as_ref().map(|p| p.0))
    .bind(ev.policy.as_ref().map(|p| p.1.clone()))
    .bind(ev.decision)
    .bind(metadata)
    .execute(ex)
    .await?;
    Ok(())
}

/// Record without failing the caller: activity is a view, not the source of truth.
pub async fn record_quietly(pool: &PgPool, org: Uuid, ev: NewEvent) {
    if let Err(e) = record(pool, org, ev).await {
        tracing::warn!(error = %e, "recording agent activity failed");
    }
}

#[derive(Debug, Clone, Serialize)]
pub struct Ref {
    pub id: Uuid,
    pub name: String,
}

#[derive(Debug, Clone, Serialize)]
pub struct UserRef {
    pub id: Uuid,
    pub login: String,
}

#[derive(Debug, Clone, Serialize)]
pub struct Event {
    pub id: Uuid,
    pub at: DateTime<Utc>,
    pub agent: Option<Ref>,
    pub owner: Option<UserRef>,
    pub team: String,
    pub run_id: Option<Uuid>,
    #[serde(rename = "type")]
    pub kind: String,
    pub tool: String,
    pub action: String,
    pub context: Value,
    pub outcome: String,
    pub policy: Option<Ref>,
    pub decision: Option<String>,
    pub metadata: Value,
}

fn row_to_event(r: &PgRow) -> Event {
    let agent_id: Option<Uuid> = r.get("agent_id");
    let owner_id: Option<Uuid> = r.get("owner_id");
    let policy_id: Option<Uuid> = r.get("policy_id");
    Event {
        id: r.get("id"),
        at: r.get("at"),
        agent: agent_id.map(|id| Ref { id, name: r.get::<Option<String>, _>("agent_name").unwrap_or_default() }),
        owner: owner_id.map(|id| UserRef { id, login: r.get::<Option<String>, _>("owner_login").unwrap_or_default() }),
        team: r.get::<Option<String>, _>("team").unwrap_or_default(),
        run_id: r.get("run_id"),
        kind: r.get("type"),
        tool: r.get("tool"),
        action: r.get("action"),
        context: r.get("context"),
        outcome: r.get("outcome"),
        policy: policy_id.map(|id| Ref { id, name: r.get::<Option<String>, _>("policy_name").unwrap_or_default() }),
        decision: r.get("decision"),
        metadata: r.get("metadata"),
    }
}

#[derive(Debug, Default, Clone, Deserialize)]
pub struct Filter {
    pub agent: Option<Uuid>,
    pub owner: Option<Uuid>,
    pub team: Option<String>,
    #[serde(rename = "type")]
    pub kind: Option<String>,
    pub outcome: Option<String>,
    pub since: Option<DateTime<Utc>>,
    pub until: Option<DateTime<Utc>>,
    pub limit: Option<i64>,
    pub before: Option<Uuid>,
}

const FROM: &str = "FROM agent_events e LEFT JOIN agents a ON a.id = e.agent_id LEFT JOIN users u ON u.id = a.owner_id";
const WHERE: &str = "e.org_id = $1
   AND ($2::uuid IS NULL OR e.agent_id = $2)
   AND ($3::uuid IS NULL OR a.owner_id = $3)
   AND ($4::text IS NULL OR lower(a.team) = lower($4))
   AND ($5::text IS NULL OR e.type = $5)
   AND ($6::timestamptz IS NULL OR e.at >= $6)
   AND ($7::timestamptz IS NULL OR e.at < $7)";

fn nonempty(s: &Option<String>) -> Option<String> {
    s.as_deref().map(str::trim).filter(|s| !s.is_empty()).map(str::to_string)
}

/// Events under `f`, newest first, and the count per outcome without the outcome filter.
pub async fn list(pool: &PgPool, org: Uuid, f: &Filter) -> sqlx::Result<(Vec<Event>, Value)> {
    let limit = f.limit.unwrap_or(100).clamp(1, 500);
    let before: Option<DateTime<Utc>> = match f.before {
        Some(id) => sqlx::query_scalar("SELECT at FROM agent_events WHERE id = $1 AND org_id = $2").bind(id).bind(org).fetch_optional(pool).await?,
        None => None,
    };
    let rows = sqlx::query(&format!(
        "SELECT e.*, a.name AS agent_name, a.owner_id, a.team, u.login AS owner_login {FROM}
          WHERE {WHERE} AND ($8::text IS NULL OR e.outcome = $8) AND ($9::timestamptz IS NULL OR e.at < $9)
          ORDER BY e.at DESC LIMIT $10"
    ))
    .bind(org)
    .bind(f.agent)
    .bind(f.owner)
    .bind(nonempty(&f.team))
    .bind(nonempty(&f.kind))
    .bind(f.since)
    .bind(f.until)
    .bind(nonempty(&f.outcome))
    .bind(before)
    .bind(limit)
    .fetch_all(pool)
    .await?;
    let counts = sqlx::query(&format!("SELECT e.outcome, count(*) AS n {FROM} WHERE {WHERE} GROUP BY e.outcome"))
        .bind(org)
        .bind(f.agent)
        .bind(f.owner)
        .bind(nonempty(&f.team))
        .bind(nonempty(&f.kind))
        .bind(f.since)
        .bind(f.until)
        .fetch_all(pool)
        .await?;
    let mut c = serde_json::Map::new();
    for r in &counts {
        c.insert(r.get::<String, _>("outcome"), json!(r.get::<i64, _>("n")));
    }
    Ok((rows.iter().map(row_to_event).collect(), Value::Object(c)))
}

/// The dashboard's numbers.
pub async fn summary(pool: &PgPool, org: Uuid) -> sqlx::Result<Value> {
    let a = sqlx::query(
        "SELECT count(*) AS total,
                count(DISTINCT NULLIF(team, '')) FILTER (WHERE owner_id IS NOT NULL) AS teams,
                count(*) FILTER (WHERE owner_id IS NULL) AS unowned,
                count(*) FILTER (WHERE suspended_at IS NOT NULL) AS suspended
           FROM agents WHERE org_id = $1",
    )
    .bind(org)
    .fetch_one(pool)
    .await?;
    let statuses = super::agents::list(pool, org).await?;
    let count = |s: &str| statuses.iter().filter(|x| x.status() == s).count();
    let p = sqlx::query("SELECT count(*) AS n, min(expires_at) AS soonest FROM approvals WHERE org_id = $1 AND status = 'pending' AND expires_at > now()")
        .bind(org)
        .fetch_one(pool)
        .await?;
    let hours = sqlx::query(
        "SELECT h.hour,
                count(e.id) FILTER (WHERE e.decision = 'allow') AS allowed,
                count(e.id) FILTER (WHERE e.decision = 'require_approval') AS approval,
                count(e.id) FILTER (WHERE e.decision = 'deny') AS denied
           FROM generate_series(date_trunc('hour', now()) - interval '23 hours', date_trunc('hour', now()), interval '1 hour') AS h(hour)
           LEFT JOIN agent_events e ON e.org_id = $1 AND e.decision IS NOT NULL
                AND e.type IN ('tool_call', 'permission_request') AND date_trunc('hour', e.at) = h.hour
          GROUP BY h.hour ORDER BY h.hour",
    )
    .bind(org)
    .fetch_all(pool)
    .await?;
    let hours: Vec<Value> = hours
        .iter()
        .map(|r| {
            json!({"hour": r.get::<DateTime<Utc>, _>("hour"), "allowed": r.get::<i64, _>("allowed"),
                   "approval": r.get::<i64, _>("approval"), "denied": r.get::<i64, _>("denied")})
        })
        .collect();
    let sum = |k: &str| hours.iter().map(|h| h[k].as_i64().unwrap_or(0)).sum::<i64>();
    let (allowed, approval, denied) = (sum("allowed"), sum("approval"), sum("denied"));
    let g = sqlx::query(
        "SELECT count(*) AS total,
                count(*) FILTER (WHERE a.owner_id IS NULL) AS unattributed,
                count(*) FILTER (WHERE a.owner_id IS NOT NULL AND (e.policy_id IS NULL OR pol.builtin = 'default_deny')) AS no_policy,
                count(*) FILTER (WHERE a.owner_id IS NOT NULL AND e.policy_id IS NOT NULL AND pol.builtin IS DISTINCT FROM 'default_deny') AS governed,
                percentile_cont(0.95) WITHIN GROUP (ORDER BY (e.metadata->>'elapsed_ms')::float8)
                    FILTER (WHERE e.at > now() - interval '24 hours') AS p95
           FROM agent_events e LEFT JOIN agents a ON a.id = e.agent_id LEFT JOIN policies pol ON pol.id = e.policy_id
          WHERE e.org_id = $1 AND e.type IN ('tool_call', 'permission_request') AND e.at > now() - interval '7 days'",
    )
    .bind(org)
    .fetch_one(pool)
    .await?;
    Ok(json!({
        "agents": {
            "total": a.get::<i64, _>("total"), "teams": a.get::<i64, _>("teams"), "unowned": a.get::<i64, _>("unowned"),
            "running": count("running"), "waiting": count("waiting"), "suspended": a.get::<i64, _>("suspended"),
        },
        "approvals": {"pending": p.get::<i64, _>("n"), "soonest_expiry": p.get::<Option<DateTime<Utc>>, _>("soonest")},
        "decisions_24h": {"total": allowed + approval + denied, "allowed": allowed, "approval": approval, "denied": denied, "hours": hours},
        "governed_7d": {
            "total": g.get::<i64, _>("total"), "governed": g.get::<i64, _>("governed"),
            "no_policy": g.get::<i64, _>("no_policy"), "unattributed": g.get::<i64, _>("unattributed"),
        },
        "engine": {"p95_ms": g.get::<Option<f64>, _>("p95")},
    }))
}
