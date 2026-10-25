//! Users, orgs, sessions, per-org worker tokens, machine enrollment and plan
//! limits: everything the hosted, multi-tenant control plane adds. A
//! single-tenant install never creates users; it works in [`DEFAULT_ORG`].

use chrono::{DateTime, Duration, Utc};
use rand::{Rng, RngCore};
use serde::Serialize;
use serde_json::Value;
use sidefleet_core::Capabilities;
use sqlx::{postgres::PgRow, PgPool, Row};
use uuid::Uuid;

use crate::auth::sha256_hex;

/// The org of a single-tenant install: the developer key and the static worker
/// tokens act in it, and every row from before orgs existed belongs to it.
pub const DEFAULT_ORG: Uuid = Uuid::from_u128(1);

pub const SESSION_DAYS: i64 = 30;
pub const ENROLLMENT_MINUTES: i64 = 10;
/// How long an approved enrollment waits for its `connect` to collect the token.
const COLLECT_GRACE_MINUTES: i64 = 10;
/// Pending enrollments allowed at once, per org and in total (the endpoint is public).
const MAX_PENDING_PER_ORG: i64 = 20;
const MAX_PENDING_TOTAL: i64 = 5_000;

pub fn random_hex(bytes: usize) -> String {
    let mut b = vec![0u8; bytes];
    rand::thread_rng().fill_bytes(&mut b);
    b.iter().map(|x| format!("{x:02x}")).collect()
}

// ---- plans ----

#[derive(Debug, Clone, Copy, Serialize, PartialEq, Eq)]
pub struct Limits {
    /// Registered machines.
    pub machines: i64,
    /// Runs queued or running at the same time.
    pub parallel_runs: i64,
}

/// Limits of an org on its plan. The default org (self-hosting) has none.
pub fn limits(org: Uuid, plan: &str) -> Option<Limits> {
    if org == DEFAULT_ORG {
        return None;
    }
    Some(match plan {
        "pro" => Limits { machines: 10, parallel_runs: 10 },
        _ => Limits { machines: 2, parallel_runs: 2 },
    })
}

pub async fn org_limits(pool: &PgPool, org: Uuid) -> sqlx::Result<Option<Limits>> {
    let plan: Option<String> = sqlx::query("SELECT plan FROM orgs WHERE id = $1").bind(org).fetch_optional(pool).await?.map(|r| r.get("plan"));
    Ok(limits(org, plan.as_deref().unwrap_or("free")))
}

// ---- users and orgs ----

#[derive(Debug, Clone, Serialize)]
pub struct User {
    pub id: Uuid,
    pub github_id: i64,
    pub login: String,
    pub email: Option<String>,
}

fn row_to_user(r: &PgRow) -> User {
    User { id: r.get("id"), github_id: r.get("github_id"), login: r.get("login"), email: r.get("email") }
}

#[derive(Debug, Clone, Serialize)]
pub struct OrgMembership {
    pub id: Uuid,
    pub name: String,
    pub slug: String,
    pub plan: String,
    pub role: String,
    pub limits: Option<Limits>,
}

fn row_to_membership(r: &PgRow) -> OrgMembership {
    let id: Uuid = r.get("id");
    let plan: String = r.get("plan");
    OrgMembership { id, name: r.get("name"), slug: r.get("slug"), limits: limits(id, &plan), plan, role: r.get("role") }
}

/// Lower-case letters, digits and dashes, from a GitHub login or an org name.
pub fn slugify(s: &str) -> String {
    let mut out = String::new();
    for c in s.chars().flat_map(char::to_lowercase) {
        if c.is_ascii_alphanumeric() {
            out.push(c);
        } else if !out.ends_with('-') && !out.is_empty() {
            out.push('-');
        }
    }
    let out = out.trim_matches('-').chars().take(40).collect::<String>();
    if out.is_empty() { "org".into() } else { out }
}

async fn create_org_tx(tx: &mut sqlx::Transaction<'_, sqlx::Postgres>, name: &str, owner: Uuid) -> sqlx::Result<Uuid> {
    let base = slugify(name);
    let taken = sqlx::query("SELECT 1 AS x FROM orgs WHERE slug = $1").bind(&base).fetch_optional(&mut **tx).await?.is_some();
    let slug = if taken || base == "default" { format!("{base}-{}", random_hex(2)) } else { base };
    let id = Uuid::new_v4();
    sqlx::query("INSERT INTO orgs (id, name, slug) VALUES ($1, $2, $3)").bind(id).bind(name).bind(&slug).execute(&mut **tx).await?;
    sqlx::query("INSERT INTO org_members (org_id, user_id, role) VALUES ($1, $2, 'owner')").bind(id).bind(owner).execute(&mut **tx).await?;
    Ok(id)
}

/// Create or refresh the user behind a GitHub account. A new user gets a
/// personal org named after their login. Returns the user and whether it is new.
pub async fn upsert_github_user(pool: &PgPool, github_id: i64, login: &str, email: Option<&str>) -> sqlx::Result<(User, bool)> {
    let mut tx = pool.begin().await?;
    let row = sqlx::query(
        "INSERT INTO users (id, github_id, login, email) VALUES ($1, $2, $3, $4)
         ON CONFLICT (github_id) DO UPDATE SET login = EXCLUDED.login, email = COALESCE(EXCLUDED.email, users.email)
         RETURNING id, github_id, login, email, (xmax = 0) AS inserted",
    )
    .bind(Uuid::new_v4())
    .bind(github_id)
    .bind(login)
    .bind(email)
    .fetch_one(&mut *tx)
    .await?;
    let user = row_to_user(&row);
    let inserted: bool = row.get("inserted");
    if inserted {
        create_org_tx(&mut tx, login, user.id).await?;
    }
    tx.commit().await?;
    Ok((user, inserted))
}

pub async fn create_org(pool: &PgPool, name: &str, owner: Uuid) -> sqlx::Result<Uuid> {
    let mut tx = pool.begin().await?;
    let id = create_org_tx(&mut tx, name, owner).await?;
    tx.commit().await?;
    Ok(id)
}

/// The caller's orgs: those they own first, then by age.
pub async fn memberships(pool: &PgPool, user: Uuid) -> sqlx::Result<Vec<OrgMembership>> {
    let rows = sqlx::query(
        "SELECT o.id, o.name, o.slug, o.plan, m.role FROM org_members m JOIN orgs o ON o.id = m.org_id
         WHERE m.user_id = $1 ORDER BY (m.role = 'owner') DESC, m.created_at, o.created_at",
    )
    .bind(user)
    .fetch_all(pool)
    .await?;
    Ok(rows.iter().map(row_to_membership).collect())
}

pub async fn default_org(pool: &PgPool) -> sqlx::Result<OrgMembership> {
    let row = sqlx::query("SELECT id, name, slug, plan, 'owner' AS role FROM orgs WHERE id = $1").bind(DEFAULT_ORG).fetch_one(pool).await?;
    Ok(row_to_membership(&row))
}

pub enum AddMember {
    Added(User),
    NoSuchUser,
}

/// Add a user who has signed in at least once, by GitHub login.
pub async fn add_member(pool: &PgPool, org: Uuid, login: &str, role: &str) -> sqlx::Result<AddMember> {
    let Some(row) = sqlx::query("SELECT id, github_id, login, email FROM users WHERE lower(login) = lower($1)").bind(login).fetch_optional(pool).await? else {
        return Ok(AddMember::NoSuchUser);
    };
    let user = row_to_user(&row);
    sqlx::query("INSERT INTO org_members (org_id, user_id, role) VALUES ($1, $2, $3) ON CONFLICT (org_id, user_id) DO UPDATE SET role = EXCLUDED.role")
        .bind(org)
        .bind(user.id)
        .bind(role)
        .execute(pool)
        .await?;
    Ok(AddMember::Added(user))
}

#[derive(Debug, Clone, Serialize)]
pub struct Member {
    pub login: String,
    pub role: String,
}

pub async fn list_members(pool: &PgPool, org: Uuid) -> sqlx::Result<Vec<Member>> {
    let rows = sqlx::query("SELECT u.login, m.role FROM org_members m JOIN users u ON u.id = m.user_id WHERE m.org_id = $1 ORDER BY m.created_at")
        .bind(org)
        .fetch_all(pool)
        .await?;
    Ok(rows.iter().map(|r| Member { login: r.get("login"), role: r.get("role") }).collect())
}

// ---- sessions ----

/// Start a session; returns the cookie value (only its hash is stored).
pub async fn create_session(pool: &PgPool, user: Uuid) -> sqlx::Result<String> {
    let value = random_hex(32);
    sqlx::query("INSERT INTO sessions (id, user_id, expires_at) VALUES ($1, $2, now() + make_interval(days => $3))")
        .bind(sha256_hex(&value))
        .bind(user)
        .bind(SESSION_DAYS as i32)
        .execute(pool)
        .await?;
    // Opportunistic clean-up keeps the table small without a background job.
    sqlx::query("DELETE FROM sessions WHERE expires_at < now() - interval '1 day'").execute(pool).await?;
    Ok(value)
}

pub async fn session_user(pool: &PgPool, cookie_value: &str) -> sqlx::Result<Option<User>> {
    let row = sqlx::query(
        "SELECT u.id, u.github_id, u.login, u.email FROM sessions s JOIN users u ON u.id = s.user_id
         WHERE s.id = $1 AND s.expires_at > now()",
    )
    .bind(sha256_hex(cookie_value))
    .fetch_optional(pool)
    .await?;
    Ok(row.as_ref().map(row_to_user))
}

pub async fn delete_session(pool: &PgPool, cookie_value: &str) -> sqlx::Result<()> {
    sqlx::query("DELETE FROM sessions WHERE id = $1").bind(sha256_hex(cookie_value)).execute(pool).await?;
    Ok(())
}

// ---- worker tokens ----

#[derive(Debug, Clone, Serialize)]
pub struct WorkerToken {
    pub id: Uuid,
    pub name: String,
    /// First characters of the value, to tell tokens apart; the rest is never shown again.
    pub prefix: String,
    pub created_at: DateTime<Utc>,
    pub revoked_at: Option<DateTime<Utc>>,
    pub last_used_at: Option<DateTime<Utc>>,
}

fn row_to_token(r: &PgRow) -> WorkerToken {
    WorkerToken {
        id: r.get("id"),
        name: r.get("name"),
        prefix: "sfw_".into(),
        created_at: r.get("created_at"),
        revoked_at: r.get("revoked_at"),
        last_used_at: r.get("last_used_at"),
    }
}

const TOKEN_COLS: &str = "id, name, created_at, revoked_at, last_used_at";

/// Mint a worker token for `org`. Returns its record and the value, which is not stored.
pub async fn create_token<'e, E: sqlx::PgExecutor<'e>>(ex: E, org: Uuid, name: &str) -> sqlx::Result<(WorkerToken, String)> {
    let value = format!("sfw_{}", random_hex(24));
    let row = sqlx::query(&format!("INSERT INTO worker_tokens (id, org_id, name, token_hash) VALUES ($1, $2, $3, $4) RETURNING {TOKEN_COLS}"))
        .bind(Uuid::new_v4())
        .bind(org)
        .bind(name)
        .bind(sha256_hex(&value))
        .fetch_one(ex)
        .await?;
    let mut t = row_to_token(&row);
    t.prefix = value[..8].to_string();
    Ok((t, value))
}

pub async fn list_tokens(pool: &PgPool, org: Uuid) -> sqlx::Result<Vec<WorkerToken>> {
    let rows = sqlx::query(&format!("SELECT {TOKEN_COLS} FROM worker_tokens WHERE org_id = $1 ORDER BY created_at DESC"))
        .bind(org)
        .fetch_all(pool)
        .await?;
    Ok(rows.iter().map(row_to_token).collect())
}

/// Revoke a token of `org`. Workers using it get 401 on their next call.
pub async fn revoke_token(pool: &PgPool, org: Uuid, id: Uuid) -> sqlx::Result<Option<WorkerToken>> {
    let row = sqlx::query(&format!(
        "UPDATE worker_tokens SET revoked_at = COALESCE(revoked_at, now()) WHERE id = $1 AND org_id = $2 RETURNING {TOKEN_COLS}"
    ))
    .bind(id)
    .bind(org)
    .fetch_optional(pool)
    .await?;
    Ok(row.as_ref().map(row_to_token))
}

/// The org a live (not revoked) token belongs to. Also records when it was last used.
pub async fn token_org(pool: &PgPool, token_hash: &str) -> sqlx::Result<Option<Uuid>> {
    let row = sqlx::query("SELECT id, org_id, last_used_at FROM worker_tokens WHERE token_hash = $1 AND revoked_at IS NULL")
        .bind(token_hash)
        .fetch_optional(pool)
        .await?;
    let Some(row) = row else { return Ok(None) };
    let last: Option<DateTime<Utc>> = row.get("last_used_at");
    if last.is_none_or(|t| Utc::now() - t > Duration::minutes(1)) {
        sqlx::query("UPDATE worker_tokens SET last_used_at = now() WHERE id = $1").bind(row.get::<Uuid, _>("id")).execute(pool).await?;
    }
    Ok(Some(row.get("org_id")))
}

// ---- API tokens ----

/// What an API token may do. Browser sessions and the developer key are not limited.
#[derive(Debug, Clone, Copy, Serialize, PartialEq, Eq, PartialOrd, Ord)]
#[serde(rename_all = "lowercase")]
pub enum Scope {
    /// Look: threads, their events and approvals, machines, repos.
    Read,
    /// Also start tasks, reply, stop, restore, fork, archive and open PRs; never with full access.
    Tasks,
    /// Also approve or deny tool calls and run with full access (`bypassPermissions`).
    Full,
}

impl Scope {
    pub fn parse(s: &str) -> Option<Scope> {
        match s {
            "read" => Some(Scope::Read),
            "tasks" => Some(Scope::Tasks),
            "full" => Some(Scope::Full),
            _ => None,
        }
    }
    pub fn as_str(self) -> &'static str {
        match self {
            Scope::Read => "read",
            Scope::Tasks => "tasks",
            Scope::Full => "full",
        }
    }
}

#[derive(Debug, Clone, Serialize)]
pub struct ApiToken {
    pub id: Uuid,
    pub name: String,
    pub scope: Scope,
    /// First characters of the value, to tell tokens apart; the rest is never shown again.
    pub prefix: String,
    pub created_at: DateTime<Utc>,
    pub revoked_at: Option<DateTime<Utc>>,
    pub last_used_at: Option<DateTime<Utc>>,
    /// Updates in chat, when its app is told how tasks go. The secret is never listed.
    pub updates: Option<ChatUpdates>,
}

#[derive(Debug, Clone, Serialize)]
pub struct ChatUpdates {
    /// `hermes`, `openclaw` or `webhook`: how updates are sent.
    pub kind: String,
    pub url: String,
    /// Every task in the org, not only the ones this connection started or replied to.
    pub all_tasks: bool,
    /// When an update last went out, and why it failed if it did.
    pub last_at: Option<DateTime<Utc>>,
    pub last_error: Option<String>,
}

fn row_to_api_token(r: &PgRow) -> ApiToken {
    let updates = match (r.get::<Option<String>, _>("notify_kind"), r.get::<Option<String>, _>("notify_url")) {
        (Some(kind), Some(url)) => Some(ChatUpdates {
            kind,
            url,
            all_tasks: r.get("notify_all"),
            last_at: r.get("notify_last_at"),
            last_error: r.get("notify_last_error"),
        }),
        _ => None,
    };
    ApiToken {
        id: r.get("id"),
        name: r.get("name"),
        scope: Scope::parse(r.get("scope")).unwrap_or(Scope::Read),
        prefix: "sfa_".into(),
        created_at: r.get("created_at"),
        revoked_at: r.get("revoked_at"),
        last_used_at: r.get("last_used_at"),
        updates,
    }
}

const API_TOKEN_COLS: &str =
    "id, name, scope, created_at, revoked_at, last_used_at, notify_kind, notify_url, notify_all, notify_last_at, notify_last_error";

/// Mint an API token for `user` (none: the developer key) in `org`. Returns its record and the value, which is not stored.
pub async fn create_api_token(pool: &PgPool, org: Uuid, user: Option<Uuid>, name: &str, scope: Scope) -> sqlx::Result<(ApiToken, String)> {
    let value = format!("sfa_{}", random_hex(24));
    let row = sqlx::query(&format!(
        "INSERT INTO api_tokens (id, org_id, user_id, name, scope, token_hash) VALUES ($1, $2, $3, $4, $5, $6) RETURNING {API_TOKEN_COLS}"
    ))
    .bind(Uuid::new_v4())
    .bind(org)
    .bind(user)
    .bind(name)
    .bind(scope.as_str())
    .bind(sha256_hex(&value))
    .fetch_one(pool)
    .await?;
    let mut t = row_to_api_token(&row);
    t.prefix = value[..8].to_string();
    Ok((t, value))
}

/// The caller's own tokens in `org`, newest first.
pub async fn list_api_tokens(pool: &PgPool, org: Uuid, user: Option<Uuid>) -> sqlx::Result<Vec<ApiToken>> {
    let rows = sqlx::query(&format!(
        "SELECT {API_TOKEN_COLS} FROM api_tokens WHERE org_id = $1 AND user_id IS NOT DISTINCT FROM $2 ORDER BY created_at DESC"
    ))
    .bind(org)
    .bind(user)
    .fetch_all(pool)
    .await?;
    Ok(rows.iter().map(row_to_api_token).collect())
}

/// Revoke one of the caller's tokens in `org`. It gets 401 on its next call.
pub async fn revoke_api_token(pool: &PgPool, org: Uuid, user: Option<Uuid>, id: Uuid) -> sqlx::Result<Option<ApiToken>> {
    let row = sqlx::query(&format!(
        "UPDATE api_tokens SET revoked_at = COALESCE(revoked_at, now())
         WHERE id = $1 AND org_id = $2 AND user_id IS NOT DISTINCT FROM $3 RETURNING {API_TOKEN_COLS}"
    ))
    .bind(id)
    .bind(org)
    .bind(user)
    .fetch_optional(pool)
    .await?;
    Ok(row.as_ref().map(row_to_api_token))
}

/// Turn on (or change) updates in chat for one of the caller's live tokens. Returns
/// the token and the secret its app checks updates with: kept across changes,
/// made the first time.
pub async fn set_chat_updates(
    pool: &PgPool,
    org: Uuid,
    user: Option<Uuid>,
    id: Uuid,
    kind: &str,
    url: &str,
    all_tasks: bool,
) -> sqlx::Result<Option<(ApiToken, String)>> {
    let row = sqlx::query(&format!(
        "UPDATE api_tokens SET notify_kind = $4, notify_url = $5, notify_all = $6,
             notify_secret = COALESCE(notify_secret, $7), notify_last_error = NULL
         WHERE id = $1 AND org_id = $2 AND user_id IS NOT DISTINCT FROM $3 AND revoked_at IS NULL
         RETURNING {API_TOKEN_COLS}, notify_secret"
    ))
    .bind(id)
    .bind(org)
    .bind(user)
    .bind(kind)
    .bind(url)
    .bind(all_tasks)
    .bind(random_hex(24))
    .fetch_optional(pool)
    .await?;
    Ok(row.map(|r| (row_to_api_token(&r), r.get("notify_secret"))))
}

/// Turn updates in chat off. The secret goes too, so turning them on again makes a new one.
pub async fn clear_chat_updates(pool: &PgPool, org: Uuid, user: Option<Uuid>, id: Uuid) -> sqlx::Result<Option<ApiToken>> {
    let row = sqlx::query(&format!(
        "UPDATE api_tokens SET notify_kind = NULL, notify_url = NULL, notify_secret = NULL, notify_all = false,
             notify_last_at = NULL, notify_last_error = NULL
         WHERE id = $1 AND org_id = $2 AND user_id IS NOT DISTINCT FROM $3 RETURNING {API_TOKEN_COLS}"
    ))
    .bind(id)
    .bind(org)
    .bind(user)
    .fetch_optional(pool)
    .await?;
    Ok(row.as_ref().map(row_to_api_token))
}

/// Where one connection's updates go.
#[derive(Debug, Clone)]
pub struct UpdateTarget {
    pub token: Uuid,
    pub kind: String,
    pub url: String,
    pub secret: String,
}

fn row_to_target(r: &PgRow) -> UpdateTarget {
    UpdateTarget { token: r.get("id"), kind: r.get("notify_kind"), url: r.get("notify_url"), secret: r.get("notify_secret") }
}

/// One of the caller's connections with updates on (for a test update).
pub async fn chat_update_target(pool: &PgPool, org: Uuid, user: Option<Uuid>, id: Uuid) -> sqlx::Result<Option<UpdateTarget>> {
    let row = sqlx::query(
        "SELECT id, notify_kind, notify_url, notify_secret FROM api_tokens
         WHERE id = $1 AND org_id = $2 AND user_id IS NOT DISTINCT FROM $3 AND revoked_at IS NULL AND notify_url IS NOT NULL",
    )
    .bind(id)
    .bind(org)
    .bind(user)
    .fetch_optional(pool)
    .await?;
    Ok(row.as_ref().map(row_to_target))
}

/// The live connections of `org` to tell about the thread rooted at `root`: those
/// that sent one of its messages, and those that want every task. A token whose
/// user left the org is left out, as it is for the API.
pub async fn chat_update_targets(pool: &PgPool, org: Uuid, root: Uuid) -> sqlx::Result<Vec<UpdateTarget>> {
    let rows = sqlx::query(
        "SELECT t.id, t.notify_kind, t.notify_url, t.notify_secret FROM api_tokens t
         WHERE t.org_id = $1 AND t.revoked_at IS NULL AND t.notify_url IS NOT NULL
           AND (t.user_id IS NULL OR EXISTS (SELECT 1 FROM org_members m WHERE m.org_id = t.org_id AND m.user_id = t.user_id))
           AND (t.notify_all OR EXISTS (SELECT 1 FROM runs r WHERE r.org_id = $1 AND (r.id = $2 OR r.parent_id = $2) AND r.api_token_id = t.id))",
    )
    .bind(org)
    .bind(root)
    .fetch_all(pool)
    .await?;
    Ok(rows.iter().map(row_to_target).collect())
}

/// How the last update went: shown on the Chat apps page.
pub async fn record_chat_update(pool: &PgPool, token: Uuid, error: Option<&str>) -> sqlx::Result<()> {
    sqlx::query("UPDATE api_tokens SET notify_last_at = now(), notify_last_error = $2 WHERE id = $1").bind(token).bind(error).execute(pool).await?;
    Ok(())
}

/// Remember which connection sent a message (a run), so its app hears how it goes.
pub async fn tag_run(pool: &PgPool, run: Uuid, token: Uuid) -> sqlx::Result<()> {
    sqlx::query("UPDATE runs SET api_token_id = $2 WHERE id = $1").bind(run).bind(token).execute(pool).await?;
    Ok(())
}

/// A live API token, resolved to who it acts as.
#[derive(Debug, Clone)]
pub struct ApiTokenUse {
    pub id: Uuid,
    pub name: String,
    pub scope: Scope,
    /// None for a token minted with the developer key.
    pub user: Option<User>,
    pub org: OrgMembership,
}

/// Resolve a presented token: live (not revoked), and its user still a member
/// of its org (the membership carries their current role). Records when it was last used.
pub async fn api_token_use(pool: &PgPool, token_hash: &str) -> sqlx::Result<Option<ApiTokenUse>> {
    let row = sqlx::query(
        "SELECT t.id, t.name, t.scope, t.org_id, t.user_id, t.last_used_at, u.github_id, u.login, u.email
         FROM api_tokens t LEFT JOIN users u ON u.id = t.user_id
         WHERE t.token_hash = $1 AND t.revoked_at IS NULL",
    )
    .bind(token_hash)
    .fetch_optional(pool)
    .await?;
    let Some(row) = row else { return Ok(None) };
    let org_id: Uuid = row.get("org_id");
    let user_id: Option<Uuid> = row.get("user_id");
    let (user, org) = match user_id {
        Some(uid) => {
            let user = User { id: uid, github_id: row.get("github_id"), login: row.get("login"), email: row.get("email") };
            match memberships(pool, uid).await?.into_iter().find(|m| m.id == org_id) {
                Some(m) => (Some(user), m),
                None => return Ok(None),
            }
        }
        None if org_id == DEFAULT_ORG => (None, default_org(pool).await?),
        None => return Ok(None),
    };
    let Some(scope) = Scope::parse(row.get("scope")) else { return Ok(None) };
    let id: Uuid = row.get("id");
    let last: Option<DateTime<Utc>> = row.get("last_used_at");
    if last.is_none_or(|t| Utc::now() - t > Duration::minutes(1)) {
        sqlx::query("UPDATE api_tokens SET last_used_at = now() WHERE id = $1").bind(id).execute(pool).await?;
    }
    Ok(Some(ApiTokenUse { id, name: row.get("name"), scope, user, org }))
}

// ---- enrollment ----

#[derive(Debug, Clone, Serialize)]
pub struct Enrollment {
    pub id: Uuid,
    pub code: String,
    pub machine_name: String,
    pub worker_id: String,
    pub capabilities: Value,
    pub status: String,
    pub created_at: DateTime<Utc>,
    pub expires_at: DateTime<Utc>,
    #[serde(skip)]
    pub org_id: Option<Uuid>,
}

const ENROLL_COLS: &str = "id, code, machine_name, worker_id, capabilities, status, created_at, expires_at, org_id";

fn row_to_enrollment(r: &PgRow) -> Enrollment {
    Enrollment {
        id: r.get("id"),
        code: r.get("code"),
        machine_name: r.get("machine_name"),
        worker_id: r.get("worker_id"),
        capabilities: r.get("capabilities"),
        status: r.get("status"),
        created_at: r.get("created_at"),
        expires_at: r.get("expires_at"),
        org_id: r.get("org_id"),
    }
}

/// `XXXX-XXXX` from an alphabet without look-alikes (no 0/O, 1/I/L).
pub fn new_code() -> String {
    const A: &[u8] = b"ABCDEFGHJKMNPQRSTUVWXYZ23456789";
    let mut rng = rand::thread_rng();
    let mut s: String = (0..8).map(|_| A[rng.gen_range(0..A.len())] as char).collect();
    s.insert(4, '-');
    s
}

/// Codes are typed by people: ignore case, spaces and the dash.
pub fn normalise_code(s: &str) -> String {
    let c: String = s.chars().filter(|c| c.is_ascii_alphanumeric()).map(|c| c.to_ascii_uppercase()).collect();
    if c.len() == 8 { format!("{}-{}", &c[..4], &c[4..]) } else { c }
}

pub enum NewEnrollment {
    Created(Box<Enrollment>, String),
    TooMany,
}

/// A machine asks to join. `org` is set when the control plane has only one org
/// to offer (single-tenant); otherwise whoever approves it picks the org.
pub async fn create_enrollment(pool: &PgPool, org: Option<Uuid>, machine_name: &str, worker_id: &str, caps: &Capabilities) -> sqlx::Result<NewEnrollment> {
    let row = sqlx::query(
        "SELECT count(*) FILTER (WHERE org_id IS NOT DISTINCT FROM $1) AS here, count(*) AS total
         FROM enrollments WHERE status = 'pending' AND expires_at > now()",
    )
    .bind(org)
    .fetch_one(pool)
    .await?;
    if row.get::<i64, _>("here") >= MAX_PENDING_PER_ORG && org.is_some() || row.get::<i64, _>("total") >= MAX_PENDING_TOTAL {
        return Ok(NewEnrollment::TooMany);
    }
    sqlx::query("DELETE FROM enrollments WHERE expires_at < now() - interval '1 day'").execute(pool).await?;
    let secret = random_hex(32);
    // Only what the page needs to recognise the machine; repo paths stay on it.
    let caps_summary = serde_json::json!({
        "hostname": caps.hostname,
        "os": caps.os,
        "version": caps.version,
        "agents": caps.agents,
        "repo_count": caps.repos.len(),
    });
    for _ in 0..5 {
        let res = sqlx::query(&format!(
            "INSERT INTO enrollments (id, code, secret_hash, machine_name, worker_id, capabilities, org_id, expires_at)
             VALUES ($1, $2, $3, $4, $5, $6, $7, now() + make_interval(mins => $8)) ON CONFLICT (code) DO NOTHING RETURNING {ENROLL_COLS}"
        ))
        .bind(Uuid::new_v4())
        .bind(new_code())
        .bind(sha256_hex(&secret))
        .bind(machine_name)
        .bind(worker_id)
        .bind(&caps_summary)
        .bind(org)
        .bind(ENROLLMENT_MINUTES as i32)
        .fetch_optional(pool)
        .await?;
        if let Some(row) = res {
            return Ok(NewEnrollment::Created(Box::new(row_to_enrollment(&row)), secret));
        }
    }
    Err(sqlx::Error::Protocol("could not allocate an enrollment code".into()))
}

pub enum Poll {
    Pending(Enrollment),
    /// Approved: the token, minted now and handed out this once.
    Approved { token: String, org: OrgMembership },
    Denied,
    Expired,
    /// The token was already handed out.
    Collected,
    NotFound,
}

/// The waiting `connect` asks how its request is doing (authenticated by the
/// secret it got when it asked).
pub async fn poll_enrollment(pool: &PgPool, id: Uuid, secret: &str) -> sqlx::Result<Poll> {
    let mut tx = pool.begin().await?;
    let row = sqlx::query(&format!("SELECT {ENROLL_COLS}, secret_hash FROM enrollments WHERE id = $1 FOR UPDATE"))
        .bind(id)
        .fetch_optional(&mut *tx)
        .await?;
    let Some(row) = row else { return Ok(Poll::NotFound) };
    let hash: String = row.get("secret_hash");
    if !crate::auth::digests_equal(&hash, &sha256_hex(secret)) {
        return Ok(Poll::NotFound);
    }
    let e = row_to_enrollment(&row);
    let now = Utc::now();
    let out = match e.status.as_str() {
        "pending" if e.expires_at <= now => Poll::Expired,
        "pending" => Poll::Pending(e),
        "denied" => Poll::Denied,
        "collected" => Poll::Collected,
        "approved" if e.expires_at + Duration::minutes(COLLECT_GRACE_MINUTES) <= now => Poll::Expired,
        "approved" => {
            let org = e.org_id.expect("approved enrollments have an org");
            let (token, value) = create_token(&mut *tx, org, &e.machine_name).await?;
            sqlx::query("UPDATE enrollments SET status = 'collected', token_id = $2 WHERE id = $1").bind(id).bind(token.id).execute(&mut *tx).await?;
            let o = sqlx::query("SELECT id, name, slug, plan, 'member' AS role FROM orgs WHERE id = $1").bind(org).fetch_one(&mut *tx).await?;
            Poll::Approved { token: value, org: row_to_membership(&o) }
        }
        _ => Poll::NotFound,
    };
    tx.commit().await?;
    Ok(out)
}

/// Pending, unexpired requests waiting for `org`.
pub async fn pending_enrollments(pool: &PgPool, org: Uuid) -> sqlx::Result<Vec<Enrollment>> {
    let rows = sqlx::query(&format!(
        "SELECT {ENROLL_COLS} FROM enrollments WHERE org_id = $1 AND status = 'pending' AND expires_at > now() ORDER BY created_at"
    ))
    .bind(org)
    .fetch_all(pool)
    .await?;
    Ok(rows.iter().map(row_to_enrollment).collect())
}

/// A pending request by the code its machine printed, if `org` may see it:
/// either it is waiting for this org, or for whoever has the code.
pub async fn enrollment_by_code(pool: &PgPool, org: Uuid, code: &str) -> sqlx::Result<Option<Enrollment>> {
    let row = sqlx::query(&format!(
        "SELECT {ENROLL_COLS} FROM enrollments WHERE code = $1 AND status = 'pending' AND expires_at > now()
           AND (org_id IS NULL OR org_id = $2)"
    ))
    .bind(normalise_code(code))
    .bind(org)
    .fetch_optional(pool)
    .await?;
    Ok(row.as_ref().map(row_to_enrollment))
}

pub enum Decide {
    Done(Enrollment),
    NotFound,
    Expired,
    AlreadyDecided(String),
}

/// Approve or deny a request for `org`. A request not yet tied to an org needs
/// its code, which proves the person approving can see that machine's terminal.
pub async fn decide_enrollment(pool: &PgPool, org: Uuid, user: Option<Uuid>, id: Uuid, code: Option<&str>, approve: bool) -> sqlx::Result<Decide> {
    let mut tx = pool.begin().await?;
    let row = sqlx::query(&format!("SELECT {ENROLL_COLS} FROM enrollments WHERE id = $1 FOR UPDATE")).bind(id).fetch_optional(&mut *tx).await?;
    let Some(e) = row.as_ref().map(row_to_enrollment) else { return Ok(Decide::NotFound) };
    let visible = match e.org_id {
        Some(o) => o == org,
        None => code.map(normalise_code).is_some_and(|c| c == e.code),
    };
    if !visible {
        return Ok(Decide::NotFound);
    }
    if e.status != "pending" {
        return Ok(Decide::AlreadyDecided(e.status));
    }
    if e.expires_at <= Utc::now() {
        return Ok(Decide::Expired);
    }
    let row = sqlx::query(&format!(
        "UPDATE enrollments SET status = $2, org_id = $3, decided_by = $4 WHERE id = $1 RETURNING {ENROLL_COLS}"
    ))
    .bind(id)
    .bind(if approve { "approved" } else { "denied" })
    .bind(org)
    .bind(user)
    .fetch_one(&mut *tx)
    .await?;
    tx.commit().await?;
    Ok(Decide::Done(row_to_enrollment(&row)))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn codes_are_typable() {
        let c = new_code();
        assert_eq!(c.len(), 9);
        assert_eq!(normalise_code(&c.to_lowercase().replace('-', " ")), c);
    }

    #[test]
    fn slugs() {
        assert_eq!(slugify("Foo Bar_Baz"), "foo-bar-baz");
        assert_eq!(slugify("--"), "org");
    }

    #[test]
    fn default_org_is_unlimited() {
        assert_eq!(limits(DEFAULT_ORG, "free"), None);
        assert_eq!(limits(Uuid::new_v4(), "free").unwrap().machines, 2);
    }
}
