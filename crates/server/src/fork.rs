//! The handoff a fork gives its agent: the conversation so far, as plain text,
//! since a session can't move between agents (or even between runs of different
//! agents). It holds the messages, the replies and a line per tool call; tool
//! output and thinking are left out.

use sidefleet_core::{Agent, Run, RunEvent};
use serde_json::Value;

/// At most this many bytes of transcript. A task is passed to some agents as a
/// single argument, and Linux caps one argument at 128 KiB.
pub const MAX_CONTEXT_BYTES: usize = 32 * 1024;

/// Bytes of transcript a message leaves room for: task and transcript go to the
/// agent together, as one argument for some agents.
pub fn budget(task: &str) -> usize {
    MAX_CONTEXT_BYTES.min(120_000usize.saturating_sub(task.len()))
}

fn agent_name(a: Agent) -> &'static str {
    match a {
        Agent::Claude => "Claude Code",
        Agent::Codex => "Codex",
        Agent::Opencode => "opencode",
    }
}

/// `runs` are the thread's runs up to and including the fork point, in order,
/// each with its events. The newest messages are kept when it is too long.
pub fn transcript(runs: &[(Run, Vec<RunEvent>)], budget: usize) -> String {
    let Some((root, _)) = runs.first() else { return String::new() };
    // Every agent that took part, in order of first reply (a thread can switch agents).
    let mut agents = vec![root.agent];
    for (r, _) in runs {
        if r.kind != "restore" && !agents.contains(&r.agent) {
            agents.push(r.agent);
        }
    }
    let with: Vec<&str> = agents.iter().map(|a| agent_name(*a)).collect();
    let header = format!(
        "This task continues a conversation that started in another session (with {}). \
         The transcript up to this point is below, for context. The files in your working \
         directory already include every change made in it. Reply to the new message after the transcript.\n\n<transcript>\n",
        with.join(", then ")
    );
    let footer = "</transcript>\n\nNew message:";
    let blocks: Vec<String> = runs.iter().map(|(run, events)| block(run, events)).collect();

    let room = budget.saturating_sub(header.len() + footer.len() + 64);
    let mut kept: Vec<&str> = Vec::new();
    let mut used = 0;
    for b in blocks.iter().rev() {
        if used + b.len() > room {
            break;
        }
        used += b.len();
        kept.push(b);
    }
    kept.reverse();
    let mut out = header;
    if kept.len() < blocks.len() {
        if kept.is_empty() {
            // Even the last message alone is too long: keep its end.
            let last = blocks.last().unwrap();
            let mut cut = last.len() - room.min(last.len());
            while !last.is_char_boundary(cut) {
                cut += 1;
            }
            out.push_str("(earlier messages omitted)\n\n…");
            out.push_str(&last[cut..]);
        } else {
            out.push_str(&format!("({} earlier message{} omitted)\n\n", blocks.len() - kept.len(), if blocks.len() - kept.len() == 1 { "" } else { "s" }));
            out.push_str(&kept.concat());
        }
    } else {
        out.push_str(&kept.concat());
    }
    out.push_str(footer);
    out
}

fn block(run: &Run, events: &[RunEvent]) -> String {
    if run.kind == "restore" {
        return "[The files were restored to how they were before an earlier message.]\n\n".into();
    }
    let mut out = format!("## User\n{}\n\n## {}\n", run.task.trim(), agent_name(run.agent));
    let mut last_text = String::new();
    let mut said_anything = false;
    for e in events {
        let p = &e.payload;
        let s = |k: &str| p.get(k).and_then(Value::as_str).unwrap_or("").trim().to_string();
        match e.kind.as_str() {
            "assistant_text" => {
                let t = s("text");
                if !t.is_empty() {
                    out.push_str(&t);
                    out.push_str("\n\n");
                    last_text = t;
                    said_anything = true;
                }
            }
            "tool_use" => {
                let summary = s("summary");
                out.push_str(&format!("[{}: {}]\n", s("name"), if summary.is_empty() { "…".into() } else { summary }));
                said_anything = true;
            }
            "result" => {
                let t = s("result");
                if !t.is_empty() && t != last_text {
                    out.push_str(&t);
                    out.push_str("\n\n");
                    said_anything = true;
                }
            }
            _ => {}
        }
    }
    match run.status.as_str() {
        "failed" => out.push_str("[This message failed before finishing.]\n"),
        "stopped" => out.push_str("[This message was stopped before finishing.]\n"),
        _ if !said_anything => out.push_str("[No reply.]\n"),
        _ => {}
    }
    out.push('\n');
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::Utc;
    use serde_json::json;
    use uuid::Uuid;

    fn run(task: &str, agent: Agent, kind: &str) -> Run {
        serde_json::from_value(json!({
            "id": Uuid::new_v4(), "task": task, "agent": agent.as_str(), "model": "m", "repo": "r",
            "status": "succeeded", "worker_id": null, "stop_requested": false, "created_at": Utc::now(),
            "started_at": null, "finished_at": null, "result": null, "kind": kind
        }))
        .unwrap()
    }

    fn ev(kind: &str, payload: Value) -> RunEvent {
        RunEvent { run_id: Uuid::nil(), seq: 0, kind: kind.into(), payload, at: Utc::now() }
    }

    #[test]
    fn lists_messages_replies_and_tool_calls_but_not_their_output() {
        let r1 = run("fix the login bug", Agent::Claude, "agent");
        let e1 = vec![
            ev("assistant_thinking", json!({"text": "hmm secret thoughts"})),
            ev("assistant_text", json!({"text": "Looking at auth.rs."})),
            ev("tool_use", json!({"name": "Edit", "summary": "src/auth.rs"})),
            ev("tool_result", json!({"content": "big output"})),
            ev("result", json!({"result": "Fixed: the token expiry check was inverted."})),
        ];
        let r2 = run("undo", Agent::Claude, "restore");
        let t = transcript(&[(r1, e1), (r2, vec![])], MAX_CONTEXT_BYTES);
        assert!(t.starts_with("This task continues a conversation that started in another session (with Claude Code)"));
        assert!(t.contains("## User\nfix the login bug\n\n## Claude Code\nLooking at auth.rs.\n\n[Edit: src/auth.rs]\nFixed: the token expiry check"));
        assert!(t.contains("restored to how they were before"));
        assert!(!t.contains("secret thoughts") && !t.contains("big output"));
        assert!(t.ends_with("New message:"));
    }

    #[test]
    fn names_every_agent_that_took_part() {
        let runs = vec![
            (run("plan it", Agent::Claude, "agent"), vec![]),
            (run("build it", Agent::Codex, "agent"), vec![]),
            (run("review it", Agent::Claude, "agent"), vec![]),
        ];
        let t = transcript(&runs, MAX_CONTEXT_BYTES);
        assert!(t.starts_with("This task continues a conversation that started in another session (with Claude Code, then Codex)"), "{t}");
        assert!(t.contains("## User\nbuild it\n\n## Codex\n"));
    }

    #[test]
    fn keeps_the_newest_messages_within_the_budget() {
        let runs: Vec<(Run, Vec<RunEvent>)> = (0..50)
            .map(|i| (run(&format!("message {i} {}", "x".repeat(200)), Agent::Codex, "agent"), vec![]))
            .collect();
        let t = transcript(&runs, 4000);
        assert!(t.len() <= 4000, "{}", t.len());
        assert!(t.contains("message 49") && !t.contains("message 0 "));
        assert!(t.contains("earlier messages omitted"));
        // A single message longer than the budget keeps its end.
        let big = vec![(run(&format!("{}TAIL", "é".repeat(5000)), Agent::Opencode, "agent"), vec![])];
        let t = transcript(&big, 2000);
        assert!(t.len() <= 2000 && t.contains("TAIL"), "{}", t.len());
    }
}
