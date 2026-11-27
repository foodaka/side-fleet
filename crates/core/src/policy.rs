//! The policy engine: what an agent's tool call is (as actions), and what the org's
//! policies decide about it. Shared by the control plane (which decides) and the worker
//! (which classifies on its own only to fail closed when the control plane is out of reach).
//!
//! A tool call becomes one or more [`Action`]s: `Bash` running `git push origin main` is
//! `shell.exec` and `git.push main`. For each action the matching rule with the lowest
//! priority number decides, and no match is a deny. The call gets the strictest decision
//! of its actions.

use serde::{Deserialize, Serialize};
use serde_json::Value;
use sha2::{Digest, Sha256};
use uuid::Uuid;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Decision {
    Allow,
    RequireApproval,
    Deny,
}

impl Decision {
    pub fn as_str(self) -> &'static str {
        match self {
            Decision::Allow => "allow",
            Decision::RequireApproval => "require_approval",
            Decision::Deny => "deny",
        }
    }

    pub fn parse(s: &str) -> Option<Decision> {
        Some(match s {
            "allow" => Decision::Allow,
            "require_approval" => Decision::RequireApproval,
            "deny" => Decision::Deny,
            _ => return None,
        })
    }

    /// Deny is stricter than approval, which is stricter than allow.
    fn strictness(self) -> u8 {
        match self {
            Decision::Allow => 0,
            Decision::RequireApproval => 1,
            Decision::Deny => 2,
        }
    }
}

/// Something an agent does, in the control plane's vocabulary: `tool` such as
/// `shell.exec` or `git.push`, and `target`, what it acts on (a command, path or branch).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Action {
    pub tool: String,
    #[serde(default)]
    pub target: String,
}

impl Action {
    fn new(tool: impl Into<String>, target: impl Into<String>) -> Action {
        Action { tool: tool.into(), target: target.into() }
    }
}

/// A policy as the engine sees it.
#[derive(Debug, Clone)]
pub struct Rule {
    pub id: Uuid,
    pub name: String,
    pub priority: i32,
    pub decision: Decision,
    /// Patterns: `shell.exec`, `deploy.*`, `git.push: main, release/*`.
    pub tools: Vec<String>,
}

/// A rule that matched one of the actions.
#[derive(Debug, Clone, PartialEq)]
pub struct Match {
    pub rule: Uuid,
    pub action: usize,
}

#[derive(Debug, Clone)]
pub struct Evaluation {
    pub decision: Decision,
    /// The rule that decided; None when nothing matched the deciding action (built-in deny).
    pub rule: Option<Uuid>,
    /// The action that decided (index into the actions).
    pub action: usize,
    /// Every rule that matched any action, best first, the decider included.
    pub matched: Vec<Uuid>,
}

/// Decide `actions` under `rules` (only the active ones that apply to the agent).
pub fn evaluate(rules: &[Rule], actions: &[Action]) -> Evaluation {
    let mut sorted: Vec<&Rule> = rules.iter().collect();
    sorted.sort_by_key(|r| (r.priority, r.name.clone()));
    let mut matched: Vec<Uuid> = Vec::new();
    // Per action: the best rule, or None for the fallback deny.
    let mut best: Option<(Decision, Option<&Rule>, usize)> = None;
    for (i, a) in actions.iter().enumerate() {
        let hits: Vec<&&Rule> = sorted.iter().filter(|r| r.tools.iter().any(|p| pattern_matches(p, a))).collect();
        for h in &hits {
            if !matched.contains(&h.id) {
                matched.push(h.id);
            }
        }
        let (decision, rule) = match hits.first() {
            Some(r) => (r.decision, Some(**r)),
            None => (Decision::Deny, None),
        };
        let stricter = match &best {
            None => true,
            Some((d, r, _)) => {
                decision.strictness() > d.strictness()
                    || (decision == *d && rule.map_or(i32::MAX, |x| x.priority) < r.map_or(i32::MAX, |x| x.priority))
            }
        };
        if stricter {
            best = Some((decision, rule, i));
        }
    }
    let (decision, rule, action) = best.unwrap_or((Decision::Deny, None, 0));
    matched.sort_by_key(|id| sorted.iter().position(|r| r.id == *id));
    Evaluation { decision, rule: rule.map(|r| r.id), action, matched }
}

// ---- patterns ----

/// Whether one `tools` entry matches an action. `tool[: arg, arg]`: the tool part is a
/// glob on the action's tool; each argument matches when the target, or one part of a
/// shell command, starts with it word for word, or matches it as a glob.
pub fn pattern_matches(pattern: &str, action: &Action) -> bool {
    let (tool, args) = match pattern.split_once(':') {
        Some((t, a)) => (t.trim(), Some(a)),
        None => (pattern.trim(), None),
    };
    if tool.is_empty() || !glob(&tool.to_lowercase(), &action.tool.to_lowercase()) {
        return false;
    }
    let Some(args) = args else { return true };
    let args: Vec<String> = args.split(',').map(|a| a.trim().to_lowercase()).filter(|a| !a.is_empty()).collect();
    if args.is_empty() {
        return true;
    }
    let target = action.target.to_lowercase();
    let mut parts = vec![target.trim().to_string()];
    parts.extend(shell_segments(&target).into_iter().map(|s| strip_env_prefix(&s)));
    args.iter().any(|arg| parts.iter().any(|p| if arg.contains('*') { glob(arg, p) } else { starts_with_words(p, arg) }))
}

fn starts_with_words(text: &str, prefix: &str) -> bool {
    text.starts_with(prefix) && text[prefix.len()..].chars().next().is_none_or(|c| !(c.is_alphanumeric() || c == '_' || c == '-'))
}

/// `*` matches any run of characters (including `/` and `.`); everything else is literal.
pub fn glob(pattern: &str, text: &str) -> bool {
    let p: Vec<char> = pattern.chars().collect();
    let t: Vec<char> = text.chars().collect();
    let (mut pi, mut ti, mut star, mut mark) = (0, 0, None, 0);
    while ti < t.len() {
        if pi < p.len() && p[pi] == '*' {
            star = Some(pi);
            mark = ti;
            pi += 1;
        } else if pi < p.len() && p[pi] == t[ti] {
            pi += 1;
            ti += 1;
        } else if let Some(s) = star {
            pi = s + 1;
            mark += 1;
            ti = mark;
        } else {
            return false;
        }
    }
    p[pi..].iter().all(|c| *c == '*')
}

// ---- classification ----

/// Claude Code tools that only read.
const READ_TOOLS: &[&str] = &["Read", "Glob", "Grep", "LS", "NotebookRead"];
const WRITE_TOOLS: &[&str] = &["Write", "Edit", "MultiEdit", "NotebookEdit"];
/// Claude Code's own bookkeeping: plans, to-do lists, subagents, its background shells.
const AGENT_TOOLS: &[&str] = &[
    "TodoWrite", "TodoRead", "Task", "Agent", "ExitPlanMode", "EnterPlanMode", "Skill", "SlashCommand", "AskUserQuestion",
    "BashOutput", "KillShell", "KillBash", "ToolSearch", "ListMcpResourcesTool", "ReadMcpResourceTool", "TaskOutput", "TaskStop",
];

/// The actions a tool call is. `tool_name` is the agent's tool (`Bash`, `mcp__github__create_issue`)
/// or, from a custom agent, already an action tool (`secrets.read`), when `input` may be the
/// target as a string or `{"action": …}`. `workdir` is the task's worktree, to tell writes
/// inside it from writes elsewhere.
pub fn classify(tool_name: &str, input: &Value, workdir: Option<&str>) -> Vec<Action> {
    let s = |k: &str| input.get(k).and_then(Value::as_str).unwrap_or_default().to_string();
    if tool_name.contains('.') {
        let target = match input {
            Value::String(t) => t.clone(),
            _ => input.get("action").or_else(|| input.get("target")).and_then(Value::as_str).unwrap_or_default().to_string(),
        };
        return vec![Action::new(tool_name.to_lowercase(), target)];
    }
    if READ_TOOLS.contains(&tool_name) {
        let target = [s("file_path"), s("notebook_path"), s("path"), s("pattern")].into_iter().find(|v| !v.is_empty()).unwrap_or_default();
        return vec![Action::new("fs.read", target)];
    }
    if WRITE_TOOLS.contains(&tool_name) {
        let path = [s("file_path"), s("notebook_path"), s("path")].into_iter().find(|v| !v.is_empty()).unwrap_or_default();
        return vec![write_action(&path, workdir)];
    }
    if tool_name == "Bash" {
        return shell_actions(&s("command"));
    }
    if tool_name == "WebFetch" || tool_name == "WebSearch" {
        let target = [s("url"), s("query")].into_iter().find(|v| !v.is_empty()).unwrap_or_default();
        return vec![Action::new("net.fetch", target)];
    }
    if AGENT_TOOLS.contains(&tool_name) {
        return vec![Action::new(format!("agent.{}", tool_name.to_lowercase()), "")];
    }
    if let Some(rest) = tool_name.strip_prefix("mcp__") {
        let (server, tool) = rest.split_once("__").unwrap_or((rest, ""));
        let target = crate::summarize_tool_input(tool_name, input);
        return vec![Action::new(format!("mcp.{}.{}", server.to_lowercase(), tool.to_lowercase()).trim_end_matches('.').to_string(), target)];
    }
    vec![Action::new(format!("tool.{}", tool_name.to_lowercase()), crate::summarize_tool_input(tool_name, input))]
}

fn write_action(path: &str, workdir: Option<&str>) -> Action {
    let inside = |root: &str| -> Option<String> {
        let root = root.trim_end_matches('/');
        if root.is_empty() {
            return None;
        }
        let rel = path.strip_prefix(root)?;
        (rel.is_empty() || rel.starts_with('/')).then(|| rel.trim_start_matches('/').to_string())
    };
    let relative = !path.starts_with('/') && !path.starts_with('~') && !path.split('/').any(|p| p == "..");
    match workdir.and_then(inside) {
        Some(rel) if !rel.split('/').any(|p| p == "..") => Action::new("fs.write", rel),
        _ if relative && workdir.is_some() => Action::new("fs.write", path),
        _ => Action::new("fs.write.outside", path),
    }
}

/// The parts of a shell command between `&&`, `||`, `;`, `|` and newlines. Quotes are
/// not parsed: good enough to spot commands, not a shell.
pub fn shell_segments(command: &str) -> Vec<String> {
    let mut out = Vec::new();
    let mut cur = String::new();
    let mut chars = command.chars().peekable();
    while let Some(c) = chars.next() {
        let split = match c {
            ';' | '\n' => true,
            '&' if chars.peek() == Some(&'&') => {
                chars.next();
                true
            }
            '|' => {
                if chars.peek() == Some(&'|') {
                    chars.next();
                }
                true
            }
            _ => false,
        };
        if split {
            if !cur.trim().is_empty() {
                out.push(cur.trim().to_string());
            }
            cur.clear();
        } else {
            cur.push(c);
        }
    }
    if !cur.trim().is_empty() {
        out.push(cur.trim().to_string());
    }
    out
}

/// Drop leading `VAR=value` assignments, `sudo`, `env`, `time`, `exec` and `(`.
fn strip_env_prefix(segment: &str) -> String {
    let mut words: Vec<&str> = segment.trim_start_matches(['(', '{', ' ']).split_whitespace().collect();
    while let Some(w) = words.first() {
        let assignment = w.split_once('=').is_some_and(|(k, _)| !k.is_empty() && k.chars().all(|c| c.is_ascii_alphanumeric() || c == '_'));
        if assignment || matches!(*w, "sudo" | "env" | "time" | "exec" | "nohup" | "command") {
            words.remove(0);
        } else {
            break;
        }
    }
    words.join(" ")
}

const DB_CLIENTS: &[&str] = &["psql", "mysql", "mariadb", "sqlite3", "pg_dump", "pg_restore", "mongosh", "mongo", "redis-cli", "cqlsh", "clickhouse-client"];
const DESTRUCTIVE_SQL: &[&str] = &["DELETE", "DROP", "TRUNCATE"];

fn unquote(w: &str) -> &str {
    w.trim_matches(|c| c == '"' || c == '\'')
}

/// `shell.exec` for the command, plus what it does that policies care about.
fn shell_actions(command: &str) -> Vec<Action> {
    let mut out = vec![Action::new("shell.exec", command)];
    let mut push = |a: Action| {
        if !out.contains(&a) {
            out.push(a);
        }
    };
    for seg in shell_segments(command) {
        let seg = strip_env_prefix(&seg);
        let words: Vec<&str> = seg.split_whitespace().map(unquote).collect();
        let Some(first) = words.first().copied() else { continue };
        let prog = first.rsplit('/').next().unwrap_or(first);
        // git [-C dir] push [flags] [remote] [refspec]
        if prog == "git" {
            let mut i = 1;
            while i < words.len() && words[i].starts_with('-') {
                i += if matches!(words[i], "-C" | "-c") { 2 } else { 1 };
            }
            if words.get(i) == Some(&"push") {
                let positional: Vec<&str> = words[i + 1..].iter().copied().filter(|w| !w.starts_with('-')).collect();
                let branch = match positional.get(1) {
                    Some(refspec) => {
                        let dst = refspec.rsplit(':').next().unwrap_or(refspec);
                        dst.trim_start_matches('+').trim_start_matches("refs/heads/").to_string()
                    }
                    None => "HEAD".to_string(),
                };
                push(Action::new("git.push", branch));
            }
        }
        if DB_CLIENTS.contains(&prog) {
            push(Action::new("database.query", seg.clone()));
        }
        let upper = seg.to_uppercase();
        if DB_CLIENTS.contains(&prog) || upper.contains(" FROM ") || upper.contains("TABLE ") {
            for kw in DESTRUCTIVE_SQL {
                if let Some(pos) = find_word(&upper, kw) {
                    let stmt = seg[pos..].trim_matches(|c| c == '"' || c == '\'').to_string();
                    push(Action::new("database.write", stmt));
                }
            }
        }
        if let Some(d) = deploy_action(prog, &words) {
            push(Action::new(d, seg.clone()));
        }
        if secret_command(prog, &words) {
            push(Action::new("secrets.read", seg.clone()));
        }
    }
    for var in secret_vars(command) {
        push(Action::new("secrets.read", var));
    }
    out
}

fn find_word(haystack: &str, word: &str) -> Option<usize> {
    let mut from = 0;
    while let Some(i) = haystack[from..].find(word) {
        let at = from + i;
        let before = haystack[..at].chars().next_back().is_none_or(|c| !c.is_alphanumeric() && c != '_');
        let after = haystack[at + word.len()..].chars().next().is_none_or(|c| !c.is_alphanumeric() && c != '_');
        if before && after {
            return Some(at);
        }
        from = at + word.len();
    }
    None
}

fn deploy_action(prog: &str, words: &[&str]) -> Option<String> {
    let has = |w: &str| words.iter().skip(1).any(|x| *x == w);
    let sub = words.get(1).copied().unwrap_or_default();
    let yes = match prog {
        "kubectl" => matches!(sub, "apply" | "delete" | "rollout" | "scale" | "set" | "patch" | "replace" | "create"),
        "helm" => matches!(sub, "install" | "upgrade" | "uninstall" | "rollback"),
        "terraform" | "tofu" => matches!(sub, "apply" | "destroy"),
        "fly" | "flyctl" => sub == "deploy",
        "vercel" | "netlify" => has("--prod"),
        "gcloud" | "aws" | "az" => has("deploy"),
        "eb" | "cap" | "railway" => sub == "deploy" || sub == "up",
        "npm" | "pnpm" | "yarn" | "cargo" | "gem" => sub == "publish",
        "twine" => sub == "upload",
        "fastlane" => true,
        _ => false,
    };
    yes.then(|| format!("deploy.{prog}"))
}

fn secret_command(prog: &str, words: &[&str]) -> bool {
    let sub = words.get(1).copied().unwrap_or_default();
    let sub2 = words.get(2).copied().unwrap_or_default();
    match prog {
        "gh" => sub == "auth" && sub2 == "token",
        "aws" => matches!(sub, "secretsmanager" | "ssm") && sub2.starts_with("get"),
        "vault" => matches!(sub, "read" | "kv"),
        "op" => matches!(sub, "read" | "item"),
        "gcloud" => sub == "secrets",
        "printenv" => words.len() > 1 && words[1..].iter().any(|w| secret_name(w)),
        "security" => sub.starts_with("find-") && sub.ends_with("-password"),
        _ => false,
    }
}

/// Environment variables whose names look like credentials: `$GITHUB_TOKEN`, `${API_KEY}`.
fn secret_vars(command: &str) -> Vec<String> {
    let mut out = Vec::new();
    let b = command.as_bytes();
    let mut i = 0;
    while i < b.len() {
        if b[i] == b'$' {
            let mut j = i + 1;
            let braced = j < b.len() && b[j] == b'{';
            if braced {
                j += 1;
            }
            let start = j;
            while j < b.len() && (b[j].is_ascii_alphanumeric() || b[j] == b'_') {
                j += 1;
            }
            let name = &command[start..j];
            if secret_name(name) && !out.iter().any(|n: &String| n == name) {
                out.push(name.to_string());
            }
            i = j.max(i + 1);
        } else {
            i += 1;
        }
    }
    out
}

fn secret_name(name: &str) -> bool {
    let n = name.to_uppercase();
    !n.is_empty() && ["TOKEN", "SECRET", "PASSWORD", "PASSWD", "API_KEY", "APIKEY", "PRIVATE_KEY", "CREDENTIAL", "ACCESS_KEY"].iter().any(|k| n.contains(k))
}

/// Actions that may go ahead when the control plane cannot be asked: reading files and the
/// agent's own bookkeeping. Everything else is protected and fails closed.
pub fn unprotected(actions: &[Action]) -> bool {
    !actions.is_empty() && actions.iter().all(|a| a.tool == "fs.read" || a.tool.starts_with("agent."))
}

/// What an approval is bound to: SHA-256 over the tool name and its input (keys sorted),
/// as `sha256:<hex>`. Computed on the redacted input both sides see.
pub fn action_digest(tool_name: &str, input: &Value) -> String {
    let mut h = Sha256::new();
    h.update(tool_name.as_bytes());
    h.update(b"\n");
    h.update(canonical(input).as_bytes());
    let d = h.finalize();
    format!("sha256:{}", d.iter().map(|b| format!("{b:02x}")).collect::<String>())
}

/// JSON with object keys sorted at every level.
pub fn canonical(v: &Value) -> String {
    match v {
        Value::Object(m) => {
            let mut keys: Vec<&String> = m.keys().collect();
            keys.sort();
            let parts: Vec<String> = keys.iter().map(|k| format!("{}:{}", Value::String((*k).clone()), canonical(&m[*k]))).collect();
            format!("{{{}}}", parts.join(","))
        }
        Value::Array(a) => format!("[{}]", a.iter().map(canonical).collect::<Vec<_>>().join(",")),
        other => other.to_string(),
    }
}

/// The action category a decision is about, for the agent page's "what it can do" table:
/// label, the action tool shown, and a sample action to evaluate.
pub const PERMISSION_SAMPLES: &[(&str, &str, &str, &str)] = &[
    ("Read project files", "fs.read", "fs.read", "src/main.rs"),
    ("Create and edit local files", "fs.write", "fs.write", "src/main.rs"),
    ("Run shell commands", "shell.exec", "shell.exec", "ls"),
    ("Access production database", "database.query", "database.query", "psql"),
    ("Deploy to production", "deploy.*", "deploy.kubectl", "kubectl apply -f production.yaml"),
    ("Delete production data", "database.write", "database.write", "DELETE FROM ledger"),
    ("Read external credentials", "secrets.read", "secrets.read", "API_TOKEN"),
    ("Push to protected branches", "git.push", "git.push", "main"),
];

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn rule(name: &str, priority: i32, decision: Decision, tools: &[&str]) -> Rule {
        Rule { id: Uuid::new_v4(), name: name.into(), priority, decision, tools: tools.iter().map(|s| s.to_string()).collect() }
    }

    fn defaults() -> Vec<Rule> {
        vec![
            rule("Destructive production ops", 1, Decision::Deny, &["database.write: DELETE, DROP, TRUNCATE", "shell.exec: terraform destroy"]),
            rule("Push to protected branches", 5, Decision::Deny, &["git.push: main, master, release/*"]),
            rule("Production data access", 10, Decision::RequireApproval, &["database.query", "shell.exec: psql, mysql"]),
            rule("External credentials", 15, Decision::RequireApproval, &["secrets.read"]),
            rule("Production deploys", 20, Decision::RequireApproval, &["deploy.*", "shell.exec: terraform apply"]),
            rule("Push task branches", 60, Decision::Allow, &["git.push"]),
            rule("Shell in worktree", 90, Decision::Allow, &["shell.exec"]),
            rule("Project workspace", 100, Decision::Allow, &["fs.read", "fs.write"]),
            rule("Default deny", 1000, Decision::Deny, &["*"]),
        ]
    }

    fn decide(tool: &str, input: Value) -> (Decision, String) {
        let rules = defaults();
        let e = evaluate(&rules, &classify(tool, &input, Some("/work/tree")));
        (e.decision, e.rule.and_then(|id| rules.iter().find(|r| r.id == id)).map(|r| r.name.clone()).unwrap_or_default())
    }

    #[test]
    fn the_defaults_decide_like_the_mockup() {
        assert_eq!(decide("Read", json!({"file_path": "/etc/hosts"})).0, Decision::Allow);
        assert_eq!(decide("Edit", json!({"file_path": "/work/tree/src/a.rs"})), (Decision::Allow, "Project workspace".into()));
        assert_eq!(decide("Write", json!({"file_path": "/home/me/.bashrc"})), (Decision::Deny, "Default deny".into()));
        assert_eq!(decide("Bash", json!({"command": "pnpm test --filter checkout"})), (Decision::Allow, "Shell in worktree".into()));
        assert_eq!(
            decide("Bash", json!({"command": "psql \"$PROD_DB\" -f migrations/0142.sql"})),
            (Decision::RequireApproval, "Production data access".into())
        );
        assert_eq!(decide("Bash", json!({"command": "git push origin main"})), (Decision::Deny, "Push to protected branches".into()));
        assert_eq!(decide("Bash", json!({"command": "git push --force origin HEAD:refs/heads/main"})).0, Decision::Deny);
        assert_eq!(decide("Bash", json!({"command": "git push -u origin sidefleet/a1f3"})), (Decision::Allow, "Push task branches".into()));
        assert_eq!(decide("Bash", json!({"command": "git push origin release/4.18"})).0, Decision::Deny);
        assert_eq!(decide("Bash", json!({"command": "git push origin maintenance"})).0, Decision::Allow);
        assert_eq!(
            decide("Bash", json!({"command": "psql $DB -c \"DELETE FROM ledger_entries WHERE batch_id = 8812\""})),
            (Decision::Deny, "Destructive production ops".into())
        );
        assert_eq!(decide("Bash", json!({"command": "cd infra && terraform apply prod.tfplan"})), (Decision::RequireApproval, "Production deploys".into()));
        assert_eq!(decide("Bash", json!({"command": "terraform destroy"})).0, Decision::Deny);
        assert_eq!(decide("Bash", json!({"command": "curl -H \"Authorization: $GITHUB_TOKEN\" api"})), (Decision::RequireApproval, "External credentials".into()));
        assert_eq!(decide("Bash", json!({"command": "PGPASSWORD=x psql -h db"})).0, Decision::RequireApproval);
        assert_eq!(decide("secrets.read", json!("ZENDESK_API_TOKEN")).0, Decision::RequireApproval);
        assert_eq!(decide("mcp__linear__create_issue", json!({"title": "x"})).0, Decision::Deny);
    }

    #[test]
    fn lower_priority_numbers_win_and_the_others_are_listed() {
        let rules = defaults();
        let e = evaluate(&rules, &classify("shell.exec", &json!("psql \"$PROD_DB\" -c \"SELECT count(*) FROM invoices\""), None));
        assert_eq!(e.decision, Decision::RequireApproval);
        let names: Vec<&str> = e.matched.iter().map(|id| rules.iter().find(|r| r.id == *id).unwrap().name.as_str()).collect();
        assert_eq!(names, ["Production data access", "Shell in worktree", "Default deny"]);
    }

    #[test]
    fn nothing_matching_is_a_deny_even_without_a_default_policy() {
        let e = evaluate(&[], &[Action::new("shell.exec", "ls")]);
        assert_eq!(e.decision, Decision::Deny);
        assert!(e.rule.is_none());
    }

    #[test]
    fn patterns() {
        let a = |t: &str, x: &str| Action::new(t, x);
        assert!(pattern_matches("deploy.*", &a("deploy.kubectl", "")));
        assert!(!pattern_matches("deploy.*", &a("shell.exec", "")));
        assert!(pattern_matches("*", &a("anything", "")));
        assert!(pattern_matches("shell.exec: terraform apply", &a("shell.exec", "cd x && terraform apply plan")));
        assert!(!pattern_matches("shell.exec: terraform apply", &a("shell.exec", "terraform plan")));
        assert!(pattern_matches("fs.write.outside: /tmp/*", &a("fs.write.outside", "/tmp/shot.png")));
        assert!(!pattern_matches("fs.write.outside: /tmp/*", &a("fs.write.outside", "/etc/passwd")));
        assert!(pattern_matches("git.push: release/*", &a("git.push", "release/1.2")));
    }

    #[test]
    fn writes_are_told_apart_by_where_they_land() {
        let w = |p: &str| classify("Write", &json!({"file_path": p}), Some("/w/t"))[0].clone();
        assert_eq!(w("/w/t/src/a.rs"), Action::new("fs.write", "src/a.rs"));
        assert_eq!(w("src/a.rs"), Action::new("fs.write", "src/a.rs"));
        assert_eq!(w("/w/t/../x"), Action::new("fs.write.outside", "/w/t/../x"));
        assert_eq!(w("/w/tree2/a"), Action::new("fs.write.outside", "/w/tree2/a"));
        assert_eq!(w("/tmp/a.png"), Action::new("fs.write.outside", "/tmp/a.png"));
    }

    #[test]
    fn digests_ignore_key_order_and_follow_the_input() {
        let a = action_digest("Bash", &json!({"command": "ls", "description": "list"}));
        let b = action_digest("Bash", &json!({"description": "list", "command": "ls"}));
        assert_eq!(a, b);
        assert!(a.starts_with("sha256:"));
        assert_ne!(a, action_digest("Bash", &json!({"command": "ls -la", "description": "list"})));
    }

    #[test]
    fn only_reads_and_bookkeeping_go_ahead_unchecked() {
        assert!(unprotected(&classify("Read", &json!({"file_path": "a"}), None)));
        assert!(unprotected(&classify("TodoWrite", &json!({}), None)));
        assert!(!unprotected(&classify("Bash", &json!({"command": "ls"}), None)));
        assert!(!unprotected(&classify("Write", &json!({"file_path": "a"}), None)));
    }
}
