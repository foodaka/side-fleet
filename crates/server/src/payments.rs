//! Agent budgets: a run's agent pays for APIs over MPP, from an access key the
//! worker makes for that run. The worker offers the key's address ([`offer_key`]),
//! the user's root key signs an authorization for it off this server
//! (`sidefleet wallet authorize`, [`authorize`]), and the worker reports each
//! payment ([`record`]). Nothing here can spend: the control plane only ever sees
//! public keys, signed authorizations and receipts.
//!
//! Every change happens under the run's row lock and is appended to the run's
//! events (`budget_key_ready`, `budget_authorized`, `payment_made`,
//! `payment_refused`), so open pages see it live. A key waiting for authorization
//! is also a pending approval (`sidefleet_budget`), so it shows in the bell and
//! sends a push like any other; denying it starts the agent without payments.

use serde_json::{json, Value};
use sidefleet_core::{ApprovalRequest, ApprovalStatus, PayKeyInfo, PayKeyOffer, PayKeyStatus, PayPolicy, PaymentReport, RunEvent, RunStatus, BUDGET_TOOL_NAME};
use sqlx::PgPool;
use uuid::Uuid;

use crate::db::{append_event_locked, lock_run};

pub enum Outcome<T> {
    Done(T),
    NotFound,
    NotOwner,
    /// The run isn't running, has no budget, or is in the wrong state for this step.
    Conflict(&'static str),
}

fn is_address(s: &str) -> bool {
    s.len() == 42 && s.starts_with("0x") && s[2..].bytes().all(|b| b.is_ascii_hexdigit())
}

/// The worker made a key for `run_id` and asks the user to authorize it.
pub async fn offer_key(pool: &PgPool, org: Uuid, run_id: Uuid, worker_id: &str, offer: &PayKeyOffer) -> sqlx::Result<Outcome<Vec<RunEvent>>> {
    if !is_address(&offer.address) || !is_address(&offer.token) {
        return Ok(Outcome::Conflict("the key and token must be 0x addresses"));
    }
    let mut tx = pool.begin().await?;
    let Some(run) = lock_run(&mut tx, Some(org), run_id).await? else {
        return Ok(Outcome::NotFound);
    };
    if run.worker_id.as_deref() != Some(worker_id) {
        return Ok(Outcome::NotOwner);
    }
    if run.status != RunStatus::Running || run.stop_requested {
        return Ok(Outcome::Conflict("the run is not running"));
    }
    let Some(budget) = run.budget_micros else {
        return Ok(Outcome::Conflict("this run has no budget"));
    };
    sqlx::query("UPDATE runs SET pay_key = $2, pay_authorization = NULL WHERE id = $1")
        .bind(run_id)
        .bind(json!(offer))
        .execute(&mut *tx)
        .await?;
    let ev = append_event_locked(
        &mut tx,
        run_id,
        "budget_key_ready",
        &json!({
            "address": offer.address,
            "chain_id": offer.chain_id,
            "token": offer.token,
            "expires_at": offer.expires_at,
            "budget_micros": budget,
            "ask_above_micros": run.ask_above_micros,
        }),
    )
    .await?;
    let ask = ApprovalRequest {
        tool_name: BUDGET_TOOL_NAME.into(),
        input: json!({"budget": usd(budget), "budget_micros": budget, "ask_above_micros": run.ask_above_micros, "address": offer.address, "expires_at": offer.expires_at}),
        tool_use_id: None,
        timeout_secs: offer.timeout_secs,
    };
    let (_, asked) = crate::approvals::insert_locked(&mut tx, org, run_id, &ask).await?;
    tx.commit().await?;
    Ok(Outcome::Done(vec![ev, asked]))
}

/// `$5.00` for 5_000_000 micros.
fn usd(micros: i64) -> String {
    format!("${:.2}", micros as f64 / 1_000_000.0)
}

/// The authorization for `run_id`'s key, once the user has given it, or that they won't.
pub async fn status(pool: &PgPool, org: Uuid, run_id: Uuid) -> sqlx::Result<Option<PayKeyStatus>> {
    let row: Option<(Option<Value>,)> = sqlx::query_as("SELECT pay_authorization FROM runs WHERE id = $1 AND org_id = $2")
        .bind(run_id)
        .bind(org)
        .fetch_optional(pool)
        .await?;
    let Some((auth,)) = row else { return Ok(None) };
    // Pending approvals past their deadline expire when polled, as the worker's own do.
    let pending: Option<Uuid> = sqlx::query_scalar("SELECT id FROM approvals WHERE run_id = $1 AND tool_name = $2 AND status = 'pending'")
        .bind(run_id)
        .bind(BUDGET_TOOL_NAME)
        .fetch_optional(pool)
        .await?;
    if let Some(id) = pending {
        crate::approvals::poll(pool, org, run_id, id, chrono::Utc::now()).await?;
    }
    let last: Option<String> = sqlx::query_scalar("SELECT status FROM approvals WHERE run_id = $1 AND tool_name = $2 ORDER BY created_at DESC LIMIT 1")
        .bind(run_id)
        .bind(BUDGET_TOOL_NAME)
        .fetch_optional(pool)
        .await?;
    let declined = auth.is_none() && last.as_deref().and_then(ApprovalStatus::parse).is_some_and(|s| s != ApprovalStatus::Pending && s != ApprovalStatus::Approved);
    Ok(Some(PayKeyStatus {
        wallet: auth.as_ref().and_then(|a| a.get("wallet")).and_then(Value::as_str).map(str::to_string),
        authorization: auth.and_then(|mut a| a.get_mut("authorization").map(Value::take)),
        declined,
    }))
}

/// A run's budget and the key waiting for authorization.
pub async fn info(pool: &PgPool, org: Uuid, run_id: Uuid) -> sqlx::Result<Option<PayKeyInfo>> {
    Ok(crate::db::get_run(pool, org, run_id).await?.map(|run| PayKeyInfo {
        run_id: run.id,
        task: run.task,
        budget_micros: run.budget_micros,
        ask_above_micros: run.ask_above_micros,
        spent_micros: run.spent_micros,
        key: run.pay_key.and_then(|k| serde_json::from_value(k).ok()),
        authorized: run.pay_authorization.is_some(),
    }))
}

/// The user signed an authorization for the key on offer. The worker checks it
/// against its key before using it; here it only has to name that key.
pub async fn authorize(
    pool: &PgPool,
    org: Uuid,
    run_id: Uuid,
    wallet: &str,
    authorization: &Value,
    key_id: Option<&str>,
    by: &str,
) -> sqlx::Result<Outcome<Vec<RunEvent>>> {
    if !is_address(wallet) {
        return Ok(Outcome::Conflict("the wallet must be a 0x address"));
    }
    let mut tx = pool.begin().await?;
    let Some(run) = lock_run(&mut tx, Some(org), run_id).await? else {
        return Ok(Outcome::NotFound);
    };
    if run.status != RunStatus::Running || run.stop_requested {
        return Ok(Outcome::Conflict("the run is not running"));
    }
    let Some(key) = run.pay_key.and_then(|k| serde_json::from_value::<PayKeyOffer>(k).ok()) else {
        return Ok(Outcome::Conflict("no key is waiting for authorization on this run"));
    };
    let names_key = authorization
        .get("keyId")
        .and_then(Value::as_str)
        .or(key_id)
        .is_some_and(|id| id.eq_ignore_ascii_case(&key.address));
    if !names_key {
        return Ok(Outcome::Conflict("the authorization is for a different key"));
    }
    sqlx::query("UPDATE runs SET pay_authorization = $2 WHERE id = $1")
        .bind(run_id)
        .bind(json!({"wallet": wallet, "authorization": authorization}))
        .execute(&mut *tx)
        .await?;
    let ev = append_event_locked(
        &mut tx,
        run_id,
        "budget_authorized",
        &json!({"wallet": wallet, "address": key.address, "expires_at": key.expires_at, "budget_micros": run.budget_micros, "by": by}),
    )
    .await?;
    let mut events = vec![ev];
    let pending: Option<Uuid> = sqlx::query_scalar("SELECT id FROM approvals WHERE run_id = $1 AND tool_name = $2 AND status = 'pending'")
        .bind(run_id)
        .bind(BUDGET_TOOL_NAME)
        .fetch_optional(&mut *tx)
        .await?;
    if let Some(id) = pending {
        if let Some(current) = crate::approvals::lock_approval(&mut tx, org, run_id, id).await? {
            let (_, decided) = crate::approvals::transition(&mut tx, &current, ApprovalStatus::Approved, None, Some(by)).await?;
            events.push(decided);
        }
    }
    tx.commit().await?;
    Ok(Outcome::Done(events))
}

/// The worker reports a payment of `run_id`'s agent: a paid one adds to what the run has spent.
pub async fn record(pool: &PgPool, org: Uuid, run_id: Uuid, worker_id: &str, report: &PaymentReport) -> sqlx::Result<Outcome<(i64, RunEvent)>> {
    let paid = match report.outcome.as_str() {
        "paid" => true,
        "refused" | "failed" => false,
        _ => return Ok(Outcome::Conflict("outcome must be paid, refused or failed")),
    };
    if report.amount_micros < 0 {
        return Ok(Outcome::Conflict("amount must not be negative"));
    }
    let mut tx = pool.begin().await?;
    let Some(run) = lock_run(&mut tx, Some(org), run_id).await? else {
        return Ok(Outcome::NotFound);
    };
    if run.worker_id.as_deref() != Some(worker_id) {
        return Ok(Outcome::NotOwner);
    }
    let spent = if paid {
        sqlx::query_scalar::<_, i64>("UPDATE runs SET spent_micros = spent_micros + $2 WHERE id = $1 RETURNING spent_micros")
            .bind(run_id)
            .bind(report.amount_micros)
            .fetch_one(&mut *tx)
            .await?
    } else {
        run.spent_micros
    };
    let clip = |s: &Option<String>, n: usize| s.as_ref().map(|s| sidefleet_core::redact(&s.chars().take(n).collect::<String>()));
    let payload = json!({
        "amount_micros": report.amount_micros,
        "url": sidefleet_core::redact(&report.url.chars().take(2000).collect::<String>()),
        "host": report.host.chars().take(253).collect::<String>(),
        "reason": clip(&report.reason, 500),
        "detail": clip(&report.detail, 1000),
        "receipt": report.receipt,
        "outcome": report.outcome,
        "spent_micros": spent,
        "budget_micros": run.budget_micros,
    });
    let ev = append_event_locked(&mut tx, run_id, if paid { "payment_made" } else { "payment_refused" }, &payload).await?;
    if let (true, Some(recipient)) = (paid, report.recipient.as_deref()) {
        pin_recipient(&mut tx, org, run.created_by, &report.host, recipient).await?;
    }
    tx.commit().await?;
    Ok(Outcome::Done((spent, ev)))
}

// ---- approved services ----

/// Your approved services (`user` None: the developer key's, per org). Nothing approved yet
/// means the default: every service asks first.
pub async fn policy(pool: &PgPool, org: Uuid, user: Option<Uuid>) -> sqlx::Result<PayPolicy> {
    let row: Option<Value> = sqlx::query_scalar("SELECT policy FROM pay_policies WHERE org_id = $1 AND user_id IS NOT DISTINCT FROM $2")
        .bind(org)
        .bind(user)
        .fetch_optional(pool)
        .await?;
    Ok(row.and_then(|v| serde_json::from_value(v).ok()).unwrap_or_default())
}

pub async fn set_policy(pool: &PgPool, org: Uuid, user: Option<Uuid>, policy: &PayPolicy) -> sqlx::Result<()> {
    sqlx::query(
        "INSERT INTO pay_policies (org_id, user_id, policy) VALUES ($1, $2, $3)
         ON CONFLICT (org_id, COALESCE(user_id, '00000000-0000-0000-0000-000000000000')) DO UPDATE SET policy = $3, updated_at = now()",
    )
    .bind(org)
    .bind(user)
    .bind(json!(policy))
    .execute(pool)
    .await?;
    Ok(())
}

/// The first payment to an approved service records where it went, so a later one that
/// asks to be paid somewhere else asks the user first.
async fn pin_recipient(tx: &mut sqlx::Transaction<'_, sqlx::Postgres>, org: Uuid, user: Option<Uuid>, host: &str, recipient: &str) -> sqlx::Result<()> {
    let row: Option<Value> = sqlx::query_scalar("SELECT policy FROM pay_policies WHERE org_id = $1 AND user_id IS NOT DISTINCT FROM $2 FOR UPDATE")
        .bind(org)
        .bind(user)
        .fetch_optional(&mut **tx)
        .await?;
    let Some(mut policy) = row.and_then(|v| serde_json::from_value::<PayPolicy>(v).ok()) else { return Ok(()) };
    let Some(s) = policy.approved.iter_mut().find(|s| s.host.eq_ignore_ascii_case(host) && s.recipient.is_none()) else { return Ok(()) };
    s.recipient = Some(recipient.to_string());
    sqlx::query("UPDATE pay_policies SET policy = $3, updated_at = now() WHERE org_id = $1 AND user_id IS NOT DISTINCT FROM $2")
        .bind(org)
        .bind(user)
        .bind(json!(policy))
        .execute(&mut **tx)
        .await?;
    Ok(())
}

/// The public MPP services catalog, for picking approved services. The page can't fetch
/// it itself (no CORS), so it is fetched here and kept for an hour.
pub async fn catalog() -> Result<Value, String> {
    use std::sync::Mutex;
    use std::time::{Duration, Instant};
    static CACHE: Mutex<Option<(Instant, Value)>> = Mutex::new(None);
    if let Some((at, v)) = CACHE.lock().unwrap_or_else(|e| e.into_inner()).as_ref() {
        if at.elapsed() < Duration::from_secs(3600) {
            return Ok(v.clone());
        }
    }
    let http = reqwest::Client::builder().timeout(Duration::from_secs(15)).build().map_err(|e| e.to_string())?;
    let full: Value = http.get("https://mpp.dev/api/services").send().await.and_then(|r| r.error_for_status()).map_err(|e| e.to_string())?.json().await.map_err(|e| e.to_string())?;
    // Only what the picker shows.
    let services: Vec<Value> = full["services"]
        .as_array()
        .map(|a| {
            a.iter()
                .filter(|s| s["status"] != "maintenance")
                .map(|s| {
                    let host = s["serviceUrl"].as_str().and_then(|u| reqwest::Url::parse(u).ok()).and_then(|u| u.host_str().map(str::to_string));
                    json!({"id": s["id"], "name": s["name"], "host": host, "description": s["description"], "categories": s["categories"], "url": s["url"]})
                })
                .filter(|s| !s["host"].is_null())
                .collect()
        })
        .unwrap_or_default();
    let v = json!({"services": services});
    *CACHE.lock().unwrap_or_else(|e| e.into_inner()) = Some((Instant::now(), v.clone()));
    Ok(v)
}
