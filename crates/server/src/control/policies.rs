//! Policies: what an org allows, holds for approval or denies. Stored per org; a new org
//! gets [`SEEDS`] the first time anything reads its policies.

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use sidefleet_core::policy::{Decision, Rule};
use sqlx::{postgres::PgRow, PgPool, Row};
use uuid::Uuid;

use super::agents::AgentRow;
use super::audit::{self, Actor, Investigate, NewRecord};

#[derive(Debug, Clone, Serialize)]
pub struct Policy {
    pub id: Uuid,
    pub name: String,
    pub description: String,
    pub scope: String,
    pub applies_to: String,
    pub tools: Vec<String>,
    pub category: String,
    pub decision: String,
    pub priority: i32,
    pub active: bool,
    pub builtin: bool,
    #[serde(skip)]
    pub builtin_key: Option<String>,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
    pub updated_by: Option<String>,
}

impl Policy {
    pub fn rule(&self) -> Rule {
        Rule {
            id: self.id,
            name: self.name.clone(),
            priority: self.priority,
            decision: Decision::parse(&self.decision).unwrap_or(Decision::Deny),
            tools: self.tools.clone(),
        }
    }

    /// `organisation, priority 10` / `team Platform, priority 20`.
    pub fn scope_label(&self) -> String {
        let s = match self.scope.as_str() {
            "team" => format!("team {}", self.applies_to),
            "agent" => "one agent".to_string(),
            _ => "organisation".to_string(),
        };
        format!("{s}, priority {}", self.priority)
    }

    pub fn applies(&self, agent: Option<&AgentRow>) -> bool {
        match self.scope.as_str() {
            "org" => true,
            "team" => agent.is_some_and(|a| !a.team.is_empty() && a.team.eq_ignore_ascii_case(self.applies_to.trim())),
            "agent" => agent.is_some_and(|a| a.id.to_string() == self.applies_to.trim()),
            _ => false,
        }
    }
}

const COLS: &str = "id, name, description, scope, applies_to, tools, category, decision, priority, active, builtin, created_at, updated_at, updated_by";

fn row_to_policy(r: &PgRow) -> Policy {
    let builtin: Option<String> = r.get("builtin");
    Policy {
        id: r.get("id"),
        name: r.get("name"),
        description: r.get("description"),
        scope: r.get("scope"),
        applies_to: r.get("applies_to"),
        tools: r.get("tools"),
        category: r.get("category"),
        decision: r.get("decision"),
        priority: r.get("priority"),
        active: r.get("active"),
        builtin: builtin.as_deref() == Some("default_deny"),
        builtin_key: builtin,
        created_at: r.get("created_at"),
        updated_at: r.get("updated_at"),
        updated_by: r.get("updated_by"),
    }
}

/// (key, name, description, tools, category, decision, priority)
type Seed = (&'static str, &'static str, &'static str, &'static [&'static str], &'static str, &'static str, i32);

pub const SEEDS: &[Seed] = &[
    ("destructive", "Destructive production ops", "Blocks deletes, drops and truncates against production data.",
     &["database.write: DELETE, DROP, TRUNCATE", "shell.exec: terraform destroy"], "Delete production data", "deny", 1),
    ("protected_branches", "Push to protected branches", "Agents open pull requests. They never push to main or release branches.",
     &["git.push: main, master, release/*"], "Source control", "deny", 5),
    ("production_data", "Production data access", "Any read or write against a production database needs a human.",
     &["database.query", "shell.exec: psql, mysql"], "Access production database", "require_approval", 10),
    ("credentials", "External credentials", "Reading a secret Sidefleet does not manage needs approval every time.",
     &["secrets.read"], "Access external credentials", "require_approval", 15),
    ("deploys", "Production deploys", "Deploying to production needs sign-off.",
     &["deploy.*", "shell.exec: terraform apply"], "Deploy to production", "require_approval", 20),
    ("task_branches", "Push task branches", "Agents may push the branches they work on.",
     &["git.push"], "Source control", "allow", 60),
    ("shell", "Shell in worktree", "Shell commands run inside the task worktree.",
     &["shell.exec"], "Execute shell commands", "allow", 90),
    ("housekeeping", "Agent housekeeping", "To-do lists, plans, subagents and the agent's own background shells.",
     &["agent.*"], "Agent bookkeeping", "allow", 95),
    ("web", "Web access", "Fetching pages and searching the web.",
     &["net.fetch"], "Network access", "allow", 96),
    ("mcp", "MCP tools", "Tools from MCP servers can reach other systems, so each call needs approval.",
     &["mcp.*"], "Third-party tools", "require_approval", 97),
    ("workspace", "Project workspace", "Read files anywhere and write inside the task repository checkout.",
     &["fs.read", "fs.write"], "Read and create project files", "allow", 100),
    ("temp", "Temporary files", "Scratch files such as screenshots in the temporary folder.",
     &["fs.write.outside: /tmp/*, /private/tmp/*, /var/folders/*"], "Read and create project files", "allow", 105),
    ("default_deny", "Default deny", "Anything no other policy allows is denied, including when evaluation is unavailable.",
     &["*"], "Everything else", "deny", 1000),
];

/// Give `org` the default policies, once.
pub async fn ensure_seeded(pool: &PgPool, org: Uuid) -> sqlx::Result<()> {
    let seeded: Option<bool> = sqlx::query_scalar("SELECT policies_seeded FROM orgs WHERE id = $1").bind(org).fetch_optional(pool).await?;
    if seeded != Some(false) {
        return Ok(());
    }
    let mut tx = pool.begin().await?;
    let claimed = sqlx::query("UPDATE orgs SET policies_seeded = true WHERE id = $1 AND NOT policies_seeded").bind(org).execute(&mut *tx).await?;
    if claimed.rows_affected() == 0 {
        return Ok(());
    }
    for (key, name, desc, tools, category, decision, priority) in SEEDS {
        sqlx::query(
            "INSERT INTO policies (id, org_id, name, description, scope, tools, category, decision, priority, builtin, updated_by)
             VALUES ($1, $2, $3, $4, 'org', $5, $6, $7, $8, $9, 'Sidefleet')",
        )
        .bind(Uuid::new_v4())
        .bind(org)
        .bind(name)
        .bind(desc)
        .bind(tools.iter().map(|t| t.to_string()).collect::<Vec<_>>())
        .bind(category)
        .bind(decision)
        .bind(priority)
        .bind(key)
        .execute(&mut *tx)
        .await?;
    }
    audit::append(
        &mut tx,
        org,
        NewRecord {
            actor: Actor::system(),
            event: "policy.seeded",
            agent_id: None,
            target: "Default policies".into(),
            detail: format!("{} policies, default deny at priority 1000", SEEDS.len()),
            data: Investigate {
                what: "The org got Sidefleet's default policies.".into(),
                agent: "Every agent".into(),
                owner: "Not applicable".into(),
                permissions: "Defaults: workspace and shell allowed, production access held for approval, the rest denied".into(),
                policy: "All default policies".into(),
                approval: "Not applicable".into(),
                decided_by: "Sidefleet".into(),
                outcome: "Live from the next tool call".into(),
            },
        },
    )
    .await?;
    tx.commit().await
}

pub async fn list(pool: &PgPool, org: Uuid) -> sqlx::Result<Vec<Policy>> {
    ensure_seeded(pool, org).await?;
    let rows = sqlx::query(&format!("SELECT {COLS} FROM policies WHERE org_id = $1 ORDER BY priority, name")).bind(org).fetch_all(pool).await?;
    Ok(rows.iter().map(row_to_policy).collect())
}

pub async fn get(pool: &PgPool, org: Uuid, id: Uuid) -> sqlx::Result<Option<Policy>> {
    let row = sqlx::query(&format!("SELECT {COLS} FROM policies WHERE org_id = $1 AND id = $2")).bind(org).bind(id).fetch_optional(pool).await?;
    Ok(row.as_ref().map(row_to_policy))
}

/// The active policies that apply to `agent` (org-wide ones only without an agent).
pub async fn for_agent(pool: &PgPool, org: Uuid, agent: Option<&AgentRow>) -> sqlx::Result<Vec<Policy>> {
    Ok(list(pool, org).await?.into_iter().filter(|p| p.active && p.applies(agent)).collect())
}

#[derive(Debug, Clone, Default, Deserialize)]
pub struct PolicyInput {
    pub name: Option<String>,
    pub description: Option<String>,
    pub scope: Option<String>,
    pub applies_to: Option<String>,
    pub tools: Option<Vec<String>>,
    pub category: Option<String>,
    pub decision: Option<String>,
    pub priority: Option<i32>,
    pub active: Option<bool>,
}

/// A policy after applying `input` to `base`, checked. Err is what to tell the caller.
pub fn merged(base: Option<&Policy>, input: &PolicyInput) -> Result<Policy, String> {
    let now = Utc::now();
    let mut p = base.cloned().unwrap_or(Policy {
        id: Uuid::new_v4(),
        name: String::new(),
        description: String::new(),
        scope: "org".into(),
        applies_to: String::new(),
        tools: vec![],
        category: String::new(),
        decision: String::new(),
        priority: 100,
        active: true,
        builtin: false,
        builtin_key: None,
        created_at: now,
        updated_at: now,
        updated_by: None,
    });
    if let Some(v) = &input.name {
        p.name = v.trim().chars().take(120).collect();
    }
    if let Some(v) = &input.description {
        p.description = v.trim().chars().take(1000).collect();
    }
    if let Some(v) = &input.scope {
        p.scope = v.trim().to_lowercase();
    }
    if let Some(v) = &input.applies_to {
        p.applies_to = v.trim().chars().take(200).collect();
    }
    if let Some(v) = &input.tools {
        p.tools = v.iter().map(|t| t.trim().chars().take(300).collect::<String>()).filter(|t| !t.is_empty()).take(50).collect();
    }
    if let Some(v) = &input.category {
        p.category = v.trim().chars().take(120).collect();
    }
    if let Some(v) = &input.decision {
        p.decision = v.trim().to_string();
    }
    if let Some(v) = input.priority {
        p.priority = v;
    }
    if let Some(v) = input.active {
        p.active = v;
    }
    if p.name.is_empty() {
        return Err("a policy needs a name".into());
    }
    if !matches!(p.scope.as_str(), "org" | "team" | "agent") {
        return Err("scope must be org, team or agent".into());
    }
    if p.scope == "org" {
        p.applies_to.clear();
    } else if p.applies_to.is_empty() {
        return Err(format!("a {} policy needs applies_to", p.scope));
    }
    if p.scope == "agent" && Uuid::parse_str(&p.applies_to).is_err() {
        return Err("an agent policy's applies_to is the agent's id".into());
    }
    if p.tools.is_empty() {
        return Err("a policy needs at least one tool pattern".into());
    }
    if Decision::parse(&p.decision).is_none() {
        return Err("decision must be allow, require_approval or deny".into());
    }
    if !(1..=100_000).contains(&p.priority) {
        return Err("priority must be between 1 and 100000".into());
    }
    if p.builtin && p.tools != ["*"] {
        return Err("the Default deny policy always matches everything (`*`)".into());
    }
    Ok(p)
}

pub async fn insert(pool: &PgPool, org: Uuid, p: &Policy, by: &str) -> sqlx::Result<Policy> {
    let row = sqlx::query(&format!(
        "INSERT INTO policies (id, org_id, name, description, scope, applies_to, tools, category, decision, priority, active, updated_by)
         VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10, $11, $12) RETURNING {COLS}"
    ))
    .bind(p.id)
    .bind(org)
    .bind(&p.name)
    .bind(&p.description)
    .bind(&p.scope)
    .bind(&p.applies_to)
    .bind(&p.tools)
    .bind(&p.category)
    .bind(&p.decision)
    .bind(p.priority)
    .bind(p.active)
    .bind(by)
    .fetch_one(pool)
    .await?;
    Ok(row_to_policy(&row))
}

pub async fn update(tx: &mut sqlx::Transaction<'_, sqlx::Postgres>, org: Uuid, p: &Policy, by: &str) -> sqlx::Result<Policy> {
    let row = sqlx::query(&format!(
        "UPDATE policies SET name = $3, description = $4, scope = $5, applies_to = $6, tools = $7, category = $8, decision = $9,
                priority = $10, active = $11, updated_by = $12, updated_at = now()
          WHERE org_id = $1 AND id = $2 RETURNING {COLS}"
    ))
    .bind(org)
    .bind(p.id)
    .bind(&p.name)
    .bind(&p.description)
    .bind(&p.scope)
    .bind(&p.applies_to)
    .bind(&p.tools)
    .bind(&p.category)
    .bind(&p.decision)
    .bind(p.priority)
    .bind(p.active)
    .bind(by)
    .fetch_one(&mut **tx)
    .await?;
    Ok(row_to_policy(&row))
}

/// `Allowed` / `Approval required` / `Denied`.
pub fn decision_label(d: &str) -> &'static str {
    match d {
        "allow" => "allowed",
        "require_approval" => "approval required",
        _ => "denied",
    }
}

/// What changed between two versions of a policy, for the audit detail.
pub fn changes(old: &Policy, new: &Policy) -> String {
    let mut out = Vec::new();
    if old.name != new.name {
        out.push(format!("renamed from {}", old.name));
    }
    if old.decision != new.decision {
        out.push(format!("{} → {}", decision_label(&old.decision), decision_label(&new.decision)));
    }
    if old.priority != new.priority {
        out.push(format!("priority {} → {}", old.priority, new.priority));
    }
    if old.scope != new.scope || old.applies_to != new.applies_to {
        out.push(format!("scope {}", new.scope_label()));
    }
    if old.tools != new.tools {
        out.push(format!("tools: {}", new.tools.join("; ")));
    }
    if old.description != new.description || old.category != new.category {
        out.push("description".into());
    }
    if out.is_empty() {
        "no changes".into()
    } else {
        out.join(", ")
    }
}
