//! The slash commands and skills the composer offers after `/`: Claude Code's
//! own list for the machine, and each checkout's project skills and commands.

use std::path::Path;
use std::process::Stdio;
use std::time::Duration;

use sidefleet_core::SlashCommand;
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};

use crate::detect::{agent_command, STRIPPED_ENV};

/// Long skill descriptions are cut to this many characters; the menu shows one or two lines.
const DESCRIPTION_MAX: usize = 200;
/// A checkout's own commands past this many are left out.
const PROJECT_MAX: usize = 100;

/// Claude Code's commands outside any project: built in, the user's own skills
/// and commands, and the plugins'. Asked with the stream-json `initialize`
/// request, which answers before any message is sent, so no turn runs; MCP
/// servers are not started for it.
pub async fn claude_commands(bin: &str) -> Vec<SlashCommand> {
    let mut cmd = agent_command(bin);
    cmd.args([
        "-p",
        "--input-format",
        "stream-json",
        "--output-format",
        "stream-json",
        "--verbose",
        "--no-session-persistence",
        "--strict-mcp-config",
    ]);
    for k in STRIPPED_ENV {
        cmd.env_remove(k);
    }
    cmd.current_dir(std::env::temp_dir()).stdin(Stdio::piped()).stdout(Stdio::piped()).stderr(Stdio::null()).kill_on_drop(true);
    let Ok(mut child) = cmd.spawn() else { return vec![] };
    let (Some(mut stdin), Some(stdout)) = (child.stdin.take(), child.stdout.take()) else { return vec![] };
    let ask = async {
        let req = r#"{"type":"control_request","request_id":"sidefleet-commands","request":{"subtype":"initialize"}}"#;
        stdin.write_all(format!("{req}\n").as_bytes()).await.ok()?;
        stdin.flush().await.ok()?;
        let mut lines = BufReader::new(stdout).lines();
        while let Ok(Some(line)) = lines.next_line().await {
            if let Some(commands) = initialize_commands(&line) {
                return Some(commands);
            }
        }
        None
    };
    let out = tokio::time::timeout(Duration::from_secs(30), ask).await.ok().flatten().unwrap_or_default();
    let _ = child.kill().await;
    out
}

/// The commands in the `initialize` control response, if this line is it.
fn initialize_commands(line: &str) -> Option<Vec<SlashCommand>> {
    let v: serde_json::Value = serde_json::from_str(line).ok()?;
    if v.get("type")?.as_str()? != "control_response" {
        return None;
    }
    let r = v.get("response")?;
    if r.get("request_id")?.as_str()? != "sidefleet-commands" {
        return None;
    }
    let list = r.get("response").and_then(|x| x.get("commands")).and_then(|c| c.as_array()).cloned().unwrap_or_default();
    Some(
        list.iter()
            .filter_map(|c| {
                let name = c.get("name")?.as_str()?.trim();
                // `__remote-workflow` and the like are Claude Code's own plumbing.
                if name.is_empty() || name.starts_with("__") {
                    return None;
                }
                let text = |k: &str| c.get(k).and_then(|x| x.as_str()).unwrap_or("").trim().to_string();
                Some(SlashCommand { name: name.to_string(), description: clean_description(&text("description")), argument_hint: text("argumentHint") })
            })
            .collect(),
    )
}

/// One line, without the ` (user)` Claude Code adds to say where a skill lives, and short.
fn clean_description(s: &str) -> String {
    let s = s.split_whitespace().collect::<Vec<_>>().join(" ");
    let s = s.strip_suffix(" (user)").unwrap_or(&s);
    if s.chars().count() <= DESCRIPTION_MAX {
        return s.to_string();
    }
    let cut: String = s.chars().take(DESCRIPTION_MAX - 1).collect();
    format!("{}…", cut.trim_end())
}

/// A checkout's own skills (`.claude/skills/<name>/SKILL.md`) and commands
/// (`.claude/commands/**/*.md`, a subfolder making `folder:name`).
pub fn project_commands(repo: &Path) -> Vec<SlashCommand> {
    let mut out = Vec::new();
    let skills = repo.join(".claude/skills");
    if let Ok(rd) = std::fs::read_dir(&skills) {
        let mut dirs: Vec<_> = rd.filter_map(|e| e.ok()).map(|e| e.path()).collect();
        dirs.sort();
        for dir in dirs {
            let Ok(text) = std::fs::read_to_string(dir.join("SKILL.md")) else { continue };
            let fm = frontmatter(&text);
            let folder = dir.file_name().map(|n| n.to_string_lossy().to_string()).unwrap_or_default();
            let name = fm.get("name").filter(|n| !n.is_empty()).cloned().unwrap_or(folder);
            if fm.get("user-invocable").is_some_and(|v| v == "false") {
                continue;
            }
            out.push(command(name, &fm));
        }
    }
    let commands = repo.join(".claude/commands");
    let mut files = Vec::new();
    collect_md(&commands, 0, &mut files);
    files.sort();
    for f in files {
        let Ok(rel) = f.strip_prefix(&commands) else { continue };
        let name = rel.with_extension("").to_string_lossy().replace('/', ":");
        let text = std::fs::read_to_string(&f).unwrap_or_default();
        let mut fm = frontmatter(&text);
        if !fm.contains_key("description") {
            // Without frontmatter, Claude Code describes a command by its first line.
            let body = text.trim_start_matches("---").lines().find(|l| !l.trim().is_empty() && l.trim() != "---").unwrap_or("");
            fm.insert("description".into(), body.trim_start_matches('#').trim().to_string());
        }
        out.push(command(name, &fm));
    }
    let mut seen = std::collections::HashSet::new();
    out.retain(|c| seen.insert(c.name.clone()));
    out.truncate(PROJECT_MAX);
    out
}

fn command(name: String, fm: &std::collections::HashMap<String, String>) -> SlashCommand {
    let get = |k: &str| fm.get(k).cloned().unwrap_or_default();
    SlashCommand { name, description: clean_description(&get("description")), argument_hint: get("argument-hint") }
}

fn collect_md(dir: &Path, depth: usize, out: &mut Vec<std::path::PathBuf>) {
    let Ok(rd) = std::fs::read_dir(dir) else { return };
    for e in rd.filter_map(|e| e.ok()) {
        let p = e.path();
        if p.is_dir() && depth < 2 {
            collect_md(&p, depth + 1, out);
        } else if p.extension().is_some_and(|x| x == "md") {
            out.push(p);
        }
    }
}

/// The top-level `key: value` pairs of a markdown file's YAML frontmatter.
/// Enough YAML for skills: quoted values and `|`/`>` blocks, joined into one line.
fn frontmatter(text: &str) -> std::collections::HashMap<String, String> {
    let mut out = std::collections::HashMap::new();
    let mut lines = text.lines();
    if lines.next().map(str::trim) != Some("---") {
        return out;
    }
    let mut block: Option<(String, Vec<String>)> = None;
    for line in lines {
        if line.trim() == "---" {
            break;
        }
        if let Some((key, parts)) = block.as_mut() {
            if line.starts_with(' ') || line.starts_with('\t') || line.trim().is_empty() {
                parts.push(line.trim().to_string());
                continue;
            }
            out.insert(key.clone(), parts.join(" ").trim().to_string());
            block = None;
        }
        if line.starts_with(' ') || line.starts_with('\t') {
            continue;
        }
        let Some((k, v)) = line.split_once(':') else { continue };
        let (k, v) = (k.trim().to_string(), v.trim());
        if matches!(v, "|" | ">" | "|-" | ">-" | "|+" | ">+") {
            block = Some((k, vec![]));
            continue;
        }
        let v = v.strip_prefix('"').and_then(|x| x.strip_suffix('"')).or_else(|| v.strip_prefix('\'').and_then(|x| x.strip_suffix('\''))).unwrap_or(v);
        out.insert(k, v.to_string());
    }
    if let Some((key, parts)) = block {
        out.insert(key, parts.join(" ").trim().to_string());
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reads_the_initialize_response() {
        let line = r#"{"type":"control_response","response":{"subtype":"success","request_id":"sidefleet-commands","response":{"commands":[
            {"name":"review","description":"Pre-landing PR review. (user)","argumentHint":""},
            {"name":"__remote-workflow","description":"plumbing","argumentHint":""},
            {"name":"stripe:test-cards","description":"(Stripe) Test cards","argumentHint":"scenario"}]}}}"#;
        let got = initialize_commands(&line.replace('\n', "")).unwrap();
        assert_eq!(got.iter().map(|c| c.name.as_str()).collect::<Vec<_>>(), ["review", "stripe:test-cards"]);
        assert_eq!(got[0].description, "Pre-landing PR review.");
        assert_eq!(got[1].argument_hint, "scenario");
        assert!(initialize_commands(r#"{"type":"system","subtype":"init"}"#).is_none());
        assert!(initialize_commands(r#"{"type":"control_response","response":{"request_id":"other"}}"#).is_none());
    }

    #[test]
    fn cuts_long_descriptions() {
        let long = "word ".repeat(100);
        let d = clean_description(&long);
        assert!(d.chars().count() <= DESCRIPTION_MAX && d.ends_with('…'));
        assert_eq!(clean_description("Two\n  lines"), "Two lines");
    }

    #[test]
    fn finds_project_skills_and_commands() {
        let tmp = tempfile::tempdir().unwrap();
        let repo = tmp.path();
        let write = |p: &str, s: &str| {
            let p = repo.join(p);
            std::fs::create_dir_all(p.parent().unwrap()).unwrap();
            std::fs::write(p, s).unwrap();
        };
        write(".claude/skills/deploy/SKILL.md", "---\nname: deploy\ndescription: |\n  Ship it\n  to prod\nargument-hint: \"[env]\"\n---\nBody");
        write(".claude/skills/hidden/SKILL.md", "---\nname: hidden\nuser-invocable: false\n---\n");
        write(".claude/skills/bare/SKILL.md", "No frontmatter");
        write(".claude/commands/fix-issue.md", "# Fix a GitHub issue\n\nSteps");
        write(".claude/commands/db/migrate.md", "---\ndescription: 'Run migrations'\n---\n");
        let got = project_commands(repo);
        let names: Vec<_> = got.iter().map(|c| c.name.as_str()).collect();
        assert_eq!(names, ["bare", "deploy", "db:migrate", "fix-issue"]);
        assert_eq!(got[1].description, "Ship it to prod");
        assert_eq!(got[1].argument_hint, "[env]");
        assert_eq!(got[2].description, "Run migrations");
        assert_eq!(got[3].description, "Fix a GitHub issue");
        assert!(project_commands(&repo.join("nope")).is_empty());
    }
}
