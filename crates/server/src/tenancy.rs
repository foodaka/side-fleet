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
