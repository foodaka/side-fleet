//! A pull request's title and description, written by the thread's agent from what
//! the branch changes: its messages, commits, diff and the repo's pull request
//! template. One short non-interactive call with no tools; anything that goes wrong
//! (no agent, signed out, slow, nonsense) leaves the caller to fall back to the task.

use std::path::Path;
use std::process::Stdio;
use std::time::Duration;

use anyhow::{Context, Result};
use serde_json::Value;
use sidefleet_core::Agent;
use tokio::io::AsyncWriteExt;

use crate::checkpoint::git;

const TIMEOUT: Duration = Duration::from_secs(120);
const MAX_PATCH_CHARS: usize = 60_000;
const MAX_LOG_CHARS: usize = 8_000;
const MAX_MESSAGES_CHARS: usize = 12_000;
const MAX_TEMPLATE_CHARS: usize = 6_000;
const MAX_TITLE_CHARS: usize = 120;
const MAX_BODY_CHARS: usize = 20_000;

const SCHEMA: &str = r#"{"type":"object","properties":{"title":{"type":"string"},"body":{"type":"string"}},"required":["title","body"],"additionalProperties":false}"#;

/// Which agent CLIs this machine has, and which one the thread used.
pub struct Writer<'a> {
    pub agent: Agent,
    pub claude_bin: &'a str,
    pub codex_bin: &'a str,
}

/// What the pull request is about, gathered from the worktree with the commit's files staged.
pub struct Material {
    pub messages: Vec<String>,
    pub log: String,
    pub stat: String,
    pub patch: String,
    pub template: Option<String>,
}

/// Gather the material: everything since `base` (a commit), with what is staged.
pub async fn material(worktree: &Path, base: &str, messages: &[String]) -> Result<Material> {
    let log = git(worktree, &["log", "--format=%s%n%b", &format!("{base}..HEAD")], None).await.unwrap_or_default();
    let stat = git(worktree, &["diff", "--cached", "--stat", base], None).await?;
    let patch = git(worktree, &["diff", "--cached", "--no-color", "--no-ext-diff", base], None).await?;
    let template = [".github/pull_request_template.md", ".github/PULL_REQUEST_TEMPLATE.md", "pull_request_template.md", "PULL_REQUEST_TEMPLATE.md", "docs/pull_request_template.md"]
        .iter()
        .find_map(|p| std::fs::read_to_string(worktree.join(p)).ok())
        .filter(|t| !t.trim().is_empty());
    Ok(Material { messages: messages.to_vec(), log, stat, patch, template })
}

fn cut(s: &str, n: usize) -> &str {
    match s.char_indices().nth(n) {
        Some((i, _)) => &s[..i],
        None => s,
    }
}

/// The instructions and the material, in one prompt.
pub fn prompt(m: &Material) -> String {
    let mut messages = String::new();
    for (i, msg) in m.messages.iter().enumerate() {
        messages.push_str(&format!("Message {}:\n{}\n\n", i + 1, msg.trim()));
    }
    let mut p = String::from(
        "Write the title and description of a GitHub pull request for the changes below, which a coding agent made on this branch. \
Answer with JSON only: {\"title\": \"...\", \"body\": \"...\"}.\n\n\
- Title: one line, under 72 characters, plain words saying what changes (no prefix like \"feat:\", no trailing period).\n\
- Body: GitHub Markdown. Start with a short paragraph on what changes and why, then the notable changes as a short list. \
Mention anything a reviewer should check. Don't invent testing that isn't shown; don't list every file.\n",
    );
    if let Some(t) = &m.template {
        p.push_str("- The repository has a pull request template; fill its sections in for the body:\n\n");
        p.push_str(cut(t, MAX_TEMPLATE_CHARS));
        p.push('\n');
    }
    p.push_str("\n## What was asked\n\n");
    p.push_str(cut(&messages, MAX_MESSAGES_CHARS));
    if !m.log.trim().is_empty() {
        p.push_str("\n## Commits\n\n");
        p.push_str(cut(&m.log, MAX_LOG_CHARS));
        p.push('\n');
    }
    p.push_str("\n## Files\n\n");
    p.push_str(&m.stat);
    p.push_str("\n\n## Diff\n\n");
    p.push_str(cut(&m.patch, MAX_PATCH_CHARS));
    if m.patch.chars().count() > MAX_PATCH_CHARS {
        p.push_str("\n[the rest of the diff is cut]\n");
    }
    p
}

/// Have the agent write it. The thread's own agent goes first, then any other this machine has.
pub async fn write(w: &Writer<'_>, dir: &Path, m: &Material) -> Result<(String, String)> {
    let prompt = prompt(m);
    let order: &[Agent] = match w.agent {
        Agent::Codex => &[Agent::Codex, Agent::Claude],
        _ => &[Agent::Claude, Agent::Codex],
    };
    let mut last = anyhow::anyhow!("no agent could write it");
    for agent in order {
        let answer = match agent {
            Agent::Codex => codex(w.codex_bin, dir, &prompt).await,
            _ => claude(w.claude_bin, dir, &prompt).await,
        };
        match answer.and_then(|text| parse(&text)) {
            Ok(r) => return Ok(r),
            Err(e) => last = e,
        }
    }
    Err(last)
}

/// `{"title", "body"}` from what the agent printed: Claude's JSON envelope (structured
/// output or a result string), or the bare object, maybe in a code fence.
pub fn parse(out: &str) -> Result<(String, String)> {
    let v: Value = serde_json::from_str(out.trim()).unwrap_or(Value::Null);
    if v["is_error"] == true {
        anyhow::bail!("the agent answered with an error: {}", v["result"].as_str().unwrap_or(""));
    }
    let obj = if v["structured_output"].is_object() {
        v["structured_output"].clone()
    } else if v["title"].is_string() {
        v
    } else {
        let text = v["result"].as_str().unwrap_or(out).trim();
        let text = text.trim_start_matches("```json").trim_start_matches("```").trim_end_matches("```").trim();
        let start = text.find('{').context("no JSON in the agent's answer")?;
        let end = text.rfind('}').context("no JSON in the agent's answer")?;
        serde_json::from_str(&text[start..=end]).context("reading the agent's answer")?
    };
    let title = obj["title"].as_str().unwrap_or("").lines().next().unwrap_or("").trim().trim_end_matches('.').to_string();
    let body = obj["body"].as_str().unwrap_or("").trim().to_string();
    if title.is_empty() {
        anyhow::bail!("the agent wrote no title");
    }
    Ok((cut(&title, MAX_TITLE_CHARS).to_string(), cut(&body, MAX_BODY_CHARS).to_string()))
}

async fn claude(bin: &str, dir: &Path, prompt: &str) -> Result<String> {
    let args = [
        "-p", "--model", "haiku", "--tools", "", "--no-session-persistence", "--strict-mcp-config", "--output-format", "json", "--json-schema", SCHEMA,
    ];
    run(bin, &args, dir, prompt).await
}

async fn codex(bin: &str, dir: &Path, prompt: &str) -> Result<String> {
    let tmp = std::env::temp_dir().join(format!("sidefleet-pr-{}", uuid::Uuid::new_v4()));
    std::fs::create_dir_all(&tmp)?;
    let (schema, answer) = (tmp.join("schema.json"), tmp.join("answer.json"));
    std::fs::write(&schema, SCHEMA)?;
    let (s, a) = (schema.to_string_lossy().to_string(), answer.to_string_lossy().to_string());
    let args = ["exec", "--sandbox", "read-only", "--skip-git-repo-check", "--output-schema", &s, "--output-last-message", &a, "-"];
    let out = run(bin, &args, dir, prompt).await.and_then(|_| std::fs::read_to_string(&answer).context("reading codex's answer"));
    let _ = std::fs::remove_dir_all(&tmp);
    out
}

/// `bin args` in `dir` with `prompt` on stdin, never waiting long.
async fn run(bin: &str, args: &[&str], dir: &Path, prompt: &str) -> Result<String> {
    let mut child = tokio::process::Command::new(bin)
        .args(args)
        .current_dir(dir)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .kill_on_drop(true)
        .spawn()
        .with_context(|| format!("starting {bin}"))?;
    let mut stdin = child.stdin.take().context("no stdin")?;
    let input = prompt.to_string();
    tokio::spawn(async move {
        let _ = stdin.write_all(input.as_bytes()).await;
    });
    let out = tokio::time::timeout(TIMEOUT, child.wait_with_output()).await.map_err(|_| anyhow::anyhow!("{bin} took too long"))??;
    if !out.status.success() {
        let err = String::from_utf8_lossy(&out.stderr);
        anyhow::bail!("{bin} failed: {}", cut(err.trim(), 300));
    }
    Ok(String::from_utf8_lossy(&out.stdout).to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reads_what_agents_print() {
        let env = r###"{"type":"result","is_error":false,"result":"","structured_output":{"title":"Retry webhooks with backoff.","body":"## Why\nThey dropped."}}"###;
        assert_eq!(parse(env).unwrap(), ("Retry webhooks with backoff".into(), "## Why\nThey dropped.".into()));
        let text = r###"{"type":"result","is_error":false,"result":"```json\n{\"title\": \"Fix it\", \"body\": \"Done\"}\n```"}"###;
        assert_eq!(parse(text).unwrap(), ("Fix it".into(), "Done".into()));
        assert_eq!(parse(r#"{"title":"Bare","body":""}"#).unwrap(), ("Bare".into(), String::new()));
        assert!(parse(r#"{"type":"result","is_error":true,"result":"Not logged in"}"#).is_err());
        assert!(parse(r#"{"title":"","body":"x"}"#).is_err());
    }

    #[test]
    fn the_prompt_has_the_template_and_cuts_long_diffs() {
        let m = Material {
            messages: vec!["Fix the login loop".into()],
            log: "Fix the login loop\n".into(),
            stat: " src/a.rs | 2 +-".into(),
            patch: "x".repeat(MAX_PATCH_CHARS + 10),
            template: Some("## Summary\n## Testing".into()),
        };
        let p = prompt(&m);
        assert!(p.contains("Message 1:\nFix the login loop") && p.contains("## Summary\n## Testing") && p.contains("[the rest of the diff is cut]"));
        assert!(p.len() < MAX_PATCH_CHARS + 5_000);
    }

    /// A stand-in `claude` that checks it got the prompt and answers.
    #[tokio::test]
    async fn asks_claude_and_falls_back_to_codex() {
        use std::os::unix::fs::PermissionsExt;
        let tmp = tempfile::tempdir().unwrap();
        let claude = tmp.path().join("claude");
        std::fs::write(&claude, "#!/bin/sh\ngrep -q 'Fix the login loop' || exit 3\necho '{\"is_error\":false,\"structured_output\":{\"title\":\"Stop the login loop\",\"body\":\"It looped.\"}}'\n").unwrap();
        std::fs::set_permissions(&claude, std::fs::Permissions::from_mode(0o755)).unwrap();
        let m = Material { messages: vec!["Fix the login loop".into()], log: String::new(), stat: String::new(), patch: String::new(), template: None };
        let w = Writer { agent: Agent::Claude, claude_bin: claude.to_str().unwrap(), codex_bin: "/nonexistent/codex" };
        assert_eq!(write(&w, tmp.path(), &m).await.unwrap(), ("Stop the login loop".into(), "It looped.".into()));

        // A Codex thread tries codex first; it's missing here, so Claude writes it.
        let w = Writer { agent: Agent::Codex, ..w };
        assert_eq!(write(&w, tmp.path(), &m).await.unwrap().0, "Stop the login loop");

        // Neither works: an error, for the caller to fall back on.
        let w = Writer { agent: Agent::Claude, claude_bin: "/nonexistent/claude", codex_bin: "/nonexistent/codex" };
        assert!(write(&w, tmp.path(), &m).await.is_err());
    }
}
