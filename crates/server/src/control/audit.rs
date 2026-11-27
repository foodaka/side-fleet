//! The audit log: append-only and tamper-evident. Each record's `hash` covers its contents
//! and the `hash` of the record before it in the same org, so editing or deleting one
//! breaks the chain from there on (and the database refuses both anyway).
//!
//! Appends to one org are serialised with a transaction-level advisory lock. Take it last
//! in a transaction (after any run or approval locks), so two transactions can't wait on
//! each other.

use chrono::{DateTime, SecondsFormat, Utc};
use serde::Serialize;
use serde_json::{json, Value};
use sha2::{Digest, Sha256};
use sidefleet_core::policy::canonical;
use sqlx::{postgres::PgRow, PgPool, Postgres, Row, Transaction};
use uuid::Uuid;

use crate::auth::Caller;

/// `prev_hash` of an org's first record.
pub const GENESIS: &str = "0000000000000000000000000000000000000000000000000000000000000000";

/// Who did something, as the audit log names them.
#[derive(Debug, Clone)]
pub struct Actor {
    pub name: String,
    /// `admin` (org owner), `member`, `agent`, `api_key` or `system`.
    pub kind: &'static str,
}

impl Actor {
    pub fn system() -> Actor {
        Actor { name: "Sidefleet".into(), kind: "system" }
    }
    pub fn agent(name: &str) -> Actor {
        Actor { name: name.into(), kind: "agent" }
    }
    pub fn caller(c: &Caller) -> Actor {
        let kind = if c.token.is_some() {
            "api_key"
        } else if c.is_owner() {
            "admin"
        } else {
            "member"
        };
        Actor { name: c.user.as_ref().map_or_else(|| "developer key".to_string(), |u| u.login.clone()), kind }
    }
}

/// The questions an investigation asks of a record; every field is a short sentence.
#[derive(Debug, Clone, Default, Serialize)]
pub struct Investigate {
    pub what: String,
    pub agent: String,
    pub owner: String,
    pub permissions: String,
    pub policy: String,
    pub approval: String,
    pub decided_by: String,
    pub outcome: String,
}

pub struct NewRecord {
    pub actor: Actor,
    pub event: &'static str,
    pub agent_id: Option<Uuid>,
    pub target: String,
    pub detail: String,
    pub data: Investigate,
}

#[derive(Debug, Clone, Serialize)]
pub struct Record {
    pub id: Uuid,
    pub seq: i64,
    pub at: DateTime<Utc>,
    pub actor: String,
    pub actor_kind: String,
    pub event: String,
    pub agent_id: Option<Uuid>,
    pub target: String,
    pub detail: String,
    pub data: Value,
    pub prev_hash: String,
    pub hash: String,
}

const COLS: &str = "seq, id, org_id, at, actor, actor_kind, event, agent_id, target, detail, data, prev_hash, hash";

fn row_to_record(r: &PgRow) -> Record {
    Record {
        id: r.get("id"),
        seq: r.get("seq"),
        at: r.get("at"),
        actor: r.get("actor"),
        actor_kind: r.get("actor_kind"),
        event: r.get("event"),
        agent_id: r.get("agent_id"),
        target: r.get("target"),
        detail: r.get("detail"),
        data: r.get("data"),
        prev_hash: r.get("prev_hash"),
        hash: r.get("hash"),
    }
}

/// The hash of a record: SHA-256 over its fields as canonical JSON, `prev_hash` included.
#[allow(clippy::too_many_arguments)]
pub fn record_hash(
    org: Uuid,
    id: Uuid,
    at: DateTime<Utc>,
    actor: &str,
    actor_kind: &str,
    event: &str,
    agent_id: Option<Uuid>,
    target: &str,
    detail: &str,
    data: &Value,
    prev_hash: &str,
) -> String {
    let body = json!({
        "org_id": org, "id": id, "at": at.to_rfc3339_opts(SecondsFormat::Micros, true),
        "actor": actor, "actor_kind": actor_kind, "event": event, "agent_id": agent_id,
        "target": target, "detail": detail, "data": data, "prev_hash": prev_hash,
    });
    let d = Sha256::digest(canonical(&body).as_bytes());
    d.iter().map(|b| format!("{b:02x}")).collect()
}

fn clip(s: &str, n: usize) -> String {
    sidefleet_core::redact(s).chars().take(n).collect()
}

/// Append a record to `org`'s chain inside `tx`.
pub async fn append(tx: &mut Transaction<'_, Postgres>, org: Uuid, rec: NewRecord) -> sqlx::Result<Record> {
    sqlx::query("SELECT pg_advisory_xact_lock(hashtextextended('sidefleet-audit:' || $1::text, 0))").bind(org).execute(&mut **tx).await?;
    let prev: Option<String> = sqlx::query_scalar("SELECT hash FROM audit_log WHERE org_id = $1 ORDER BY seq DESC LIMIT 1")
        .bind(org)
        .fetch_optional(&mut **tx)
        .await?;
    let prev = prev.unwrap_or_else(|| GENESIS.to_string());
    let now = Utc::now();
    // Postgres keeps microseconds; hash what it will give back.
    let at = DateTime::from_timestamp_micros(now.timestamp_micros()).unwrap_or(now);
    let id = Uuid::new_v4();
    let target = clip(&rec.target, 300);
    let detail = clip(&rec.detail, 1000);
    let data = sidefleet_core::redact::redact_json(&serde_json::to_value(&rec.data).unwrap_or(json!({})));
    let hash = record_hash(org, id, at, &rec.actor.name, rec.actor.kind, rec.event, rec.agent_id, &target, &detail, &data, &prev);
    let row = sqlx::query(&format!(
        "INSERT INTO audit_log (id, org_id, at, actor, actor_kind, event, agent_id, target, detail, data, prev_hash, hash)
         VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10, $11, $12) RETURNING {COLS}"
    ))
    .bind(id)
    .bind(org)
    .bind(at)
    .bind(&rec.actor.name)
    .bind(rec.actor.kind)
    .bind(rec.event)
    .bind(rec.agent_id)
    .bind(&target)
    .bind(&detail)
    .bind(&data)
    .bind(&prev)
    .bind(&hash)
    .fetch_one(&mut **tx)
    .await?;
    Ok(row_to_record(&row))
}

/// Append one record in its own transaction.
pub async fn append_now(pool: &PgPool, org: Uuid, rec: NewRecord) -> sqlx::Result<Record> {
    let mut tx = pool.begin().await?;
    let r = append(&mut tx, org, rec).await?;
    tx.commit().await?;
    Ok(r)
}

#[derive(Debug, Default, Clone, serde::Deserialize)]
pub struct Filter {
    pub q: Option<String>,
    pub event: Option<String>,
    pub agent: Option<Uuid>,
    pub since: Option<DateTime<Utc>>,
    pub until: Option<DateTime<Utc>>,
    pub limit: Option<i64>,
    pub before: Option<i64>,
}

const WHERE: &str = "org_id = $1
   AND ($2::text IS NULL OR actor ILIKE $2 OR event ILIKE $2 OR target ILIKE $2 OR detail ILIKE $2)
   AND ($3::text IS NULL OR event = $3)
   AND ($4::uuid IS NULL OR agent_id = $4)
   AND ($5::timestamptz IS NULL OR at >= $5)
   AND ($6::timestamptz IS NULL OR at < $6)";

fn like(q: &Option<String>) -> Option<String> {
    q.as_deref().map(str::trim).filter(|q| !q.is_empty()).map(|q| format!("%{}%", q.replace('\\', "\\\\").replace('%', "\\%").replace('_', "\\_")))
}

/// Records under `f`, newest first, and how many there are in all.
pub async fn list(pool: &PgPool, org: Uuid, f: &Filter) -> sqlx::Result<(Vec<Record>, i64)> {
    let limit = f.limit.unwrap_or(100).clamp(1, 500);
    let q = like(&f.q);
    let event = f.event.clone().filter(|e| !e.is_empty());
    let rows = sqlx::query(&format!("SELECT {COLS} FROM audit_log WHERE {WHERE} AND ($7::bigint IS NULL OR seq < $7) ORDER BY seq DESC LIMIT $8"))
        .bind(org)
        .bind(&q)
        .bind(&event)
        .bind(f.agent)
        .bind(f.since)
        .bind(f.until)
        .bind(f.before)
        .bind(limit)
        .fetch_all(pool)
        .await?;
    let total: i64 = sqlx::query_scalar(&format!("SELECT count(*) FROM audit_log WHERE {WHERE}"))
        .bind(org)
        .bind(&q)
        .bind(&event)
        .bind(f.agent)
        .bind(f.since)
        .bind(f.until)
        .fetch_one(pool)
        .await?;
    Ok((rows.iter().map(row_to_record).collect(), total))
}

/// Every record under `f`, oldest first, for an export.
pub async fn export(pool: &PgPool, org: Uuid, f: &Filter) -> sqlx::Result<Vec<Record>> {
    let rows = sqlx::query(&format!("SELECT {COLS} FROM audit_log WHERE {WHERE} ORDER BY seq LIMIT 100000"))
        .bind(org)
        .bind(like(&f.q))
        .bind(f.event.clone().filter(|e| !e.is_empty()))
        .bind(f.agent)
        .bind(f.since)
        .bind(f.until)
        .fetch_all(pool)
        .await?;
    Ok(rows.iter().map(row_to_record).collect())
}

#[derive(Debug, Clone, Serialize)]
pub struct Verification {
    pub intact: bool,
    pub records: i64,
    /// `seq` of the first record whose `prev_hash` or `hash` does not fit.
    pub broken_at: Option<i64>,
    pub head: Option<String>,
    pub verified_at: DateTime<Utc>,
}

/// Walk `org`'s whole chain from the start and recompute every hash.
pub async fn verify(pool: &PgPool, org: Uuid) -> sqlx::Result<Verification> {
    use futures::TryStreamExt;
    let sql = format!("SELECT {COLS} FROM audit_log WHERE org_id = $1 ORDER BY seq");
    let mut rows = sqlx::query(&sql).bind(org).fetch(pool);
    let mut prev = GENESIS.to_string();
    let mut count = 0i64;
    let mut broken_at = None;
    while let Some(row) = rows.try_next().await? {
        let r = row_to_record(&row);
        count += 1;
        if broken_at.is_some() {
            continue;
        }
        let expect = record_hash(org, r.id, r.at, &r.actor, &r.actor_kind, &r.event, r.agent_id, &r.target, &r.detail, &r.data, &r.prev_hash);
        if r.prev_hash != prev || r.hash != expect {
            broken_at = Some(r.seq);
        }
        prev = r.hash;
    }
    Ok(Verification { intact: broken_at.is_none(), records: count, broken_at, head: (count > 0).then_some(prev), verified_at: Utc::now() })
}

fn csv_field(s: &str) -> String {
    if s.contains([',', '"', '\n', '\r']) {
        format!("\"{}\"", s.replace('"', "\"\""))
    } else {
        s.to_string()
    }
}

pub fn to_csv(records: &[Record]) -> String {
    let mut out = String::from("seq,id,at,actor,actor_kind,event,agent_id,target,detail,what,owner,permissions,policy,approval,decided_by,outcome,prev_hash,hash\n");
    for r in records {
        let d = |k: &str| r.data.get(k).and_then(Value::as_str).unwrap_or_default().to_string();
        let fields = [
            r.seq.to_string(),
            r.id.to_string(),
            r.at.to_rfc3339_opts(SecondsFormat::Micros, true),
            r.actor.clone(),
            r.actor_kind.clone(),
            r.event.clone(),
            r.agent_id.map(|a| a.to_string()).unwrap_or_default(),
            r.target.clone(),
            r.detail.clone(),
            d("what"),
            d("owner"),
            d("permissions"),
            d("policy"),
            d("approval"),
            d("decided_by"),
            d("outcome"),
            r.prev_hash.clone(),
            r.hash.clone(),
        ];
        out.push_str(&fields.iter().map(|f| csv_field(f)).collect::<Vec<_>>().join(","));
        out.push('\n');
    }
    out
}

pub fn to_jsonl(records: &[Record]) -> String {
    records.iter().map(|r| serde_json::to_string(r).unwrap_or_default() + "\n").collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hashes_change_with_any_field() {
        let (org, id, at) = (Uuid::new_v4(), Uuid::new_v4(), Utc::now());
        let h = |detail: &str, prev: &str| record_hash(org, id, at, "riley", "admin", "agent.suspended", None, "x", detail, &json!({"what": "w"}), prev);
        assert_eq!(h("a", GENESIS), h("a", GENESIS));
        assert_ne!(h("a", GENESIS), h("b", GENESIS));
        assert_ne!(h("a", GENESIS), h("a", "ff"));
    }

    #[test]
    fn csv_quotes_what_needs_it() {
        assert_eq!(csv_field("plain"), "plain");
        assert_eq!(csv_field("a,b"), "\"a,b\"");
        assert_eq!(csv_field("say \"hi\""), "\"say \"\"hi\"\"\"");
    }
}
