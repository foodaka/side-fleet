//! Updates in chat: when a task finishes, fails, waits for approval or gets a
//! pull request, tell the chat apps (Hermes, OpenClaw, any webhook) of the
//! connections that started it, so the assistant brings it up in the
//! conversation. Sent in the background with a few retries; never holds up or
//! fails the worker's call. How the last one went is shown on the Chat apps page.
//!
//! - `hermes`: a Hermes webhook route, signed "Generic V2" (`X-Webhook-Timestamp`,
//!   `X-Webhook-Signature-V2` = hex HMAC-SHA256 of `<timestamp>.<body>`). The
//!   route's prompt is `{text}`.
//! - `openclaw`: OpenClaw's `POST /hooks/agent` with its hook token, `{message, name}`.
//! - `webhook`: the JSON payload, signed like Hermes.

use std::net::{IpAddr, SocketAddr};
use std::time::Duration;

use serde_json::{json, Value};
use sha2::{Digest, Sha256};
use sidefleet_core::{Approval, Run, RunStatus};
use uuid::Uuid;

use crate::tenancy::{self, UpdateTarget};
use crate::{db, AppState};

/// Waits before each retry of an update that failed.
const RETRY_AFTER: &[u64] = &[2, 10];
pub const KINDS: &[&str] = &["hermes", "openclaw", "webhook"];

/// A message of the thread finished: tell its connections, unless another message waits behind it.
pub fn run_finished(state: &AppState, org: Uuid, run: &Run) {
    if run.kind == "restore" || run.status == RunStatus::Stopped {
        return;
    }
    let (state, run) = (state.clone(), run.clone());
    tokio::spawn(async move {
        let root = run.parent_id.unwrap_or(run.id);
        let Ok(targets) = tenancy::chat_update_targets(&state.pool, org, root).await else { return };
        if targets.is_empty() {
            return;
        }
        // A queued follow-up carries the task on: wait for that one instead.
        if db::thread_runs(&state.pool, org, root).await.ok().and_then(|t| t.last().map(|r| r.id)) != Some(run.id) {
            return;
        }
        let Ok(Some(task)) = crate::summary::thread(&state.pool, org, root, &base(&state)).await else { return };
        let (event, text) = match (run.kind.as_str(), run.status) {
            ("pr", RunStatus::Succeeded) if !task["pr"].is_null() => ("task.pull_request", pr_text(&task)),
            ("pr", _) => ("task.failed", failed_text(&task, "couldn't open a pull request")),
            (_, RunStatus::Succeeded) => ("task.finished", finished_text(&task)),
            _ => ("task.failed", failed_text(&task, "failed")),
        };
        let payload = json!({ "event_type": event, "text": text, "task": task });
        deliver_all(&state, &targets, &payload).await;
    });
}

/// A task waits for someone to approve a tool call.
pub fn approval_requested(state: &AppState, org: Uuid, approval: &Approval) {
    let (state, approval) = (state.clone(), approval.clone());
    tokio::spawn(async move {
        let Ok(Some(task)) = crate::summary::thread(&state.pool, org, approval.run_id, &base(&state)).await else { return };
        let Some(root) = task["id"].as_str().and_then(|s| Uuid::parse_str(s).ok()) else { return };
        let Ok(targets) = tenancy::chat_update_targets(&state.pool, org, root).await else { return };
        if targets.is_empty() {
            return;
        }
        let text = approval_text(&task, &approval);
        let payload = json!({ "event_type": "task.needs_approval", "text": text, "task": task, "approval": approval });
        deliver_all(&state, &targets, &payload).await;
    });
}

/// Send a test update to one connection, once, and say how it went.
pub async fn send_test(state: &AppState, target: &UpdateTarget) -> Result<(), String> {
    let payload = json!({
        "event_type": "test",
        "text": "Sidefleet: this is a test update. Updates in chat are working: tell the user in one short message that from now on \
                 you'll let them know here when their Sidefleet tasks finish or need their OK.",
    });
    let r = send(state, target, &payload, &Uuid::new_v4().to_string()).await;
    let _ = tenancy::record_chat_update(&state.pool, target.token, r.as_ref().err().map(String::as_str)).await;
    r
}

async fn deliver_all(state: &AppState, targets: &[UpdateTarget], payload: &Value) {
    // One id per update: retries carry the same one, so the app can drop repeats.
    let id = Uuid::new_v4().to_string();
    for t in targets {
        let mut result = send(state, t, payload, &id).await;
        for wait in RETRY_AFTER {
            if result.is_ok() {
                break;
            }
            tokio::time::sleep(Duration::from_secs(*wait)).await;
            result = send(state, t, payload, &id).await;
        }
        if let Err(e) = &result {
            tracing::warn!(token = %t.token, error = %e, "chat update not delivered");
        }
        let _ = tenancy::record_chat_update(&state.pool, t.token, result.as_ref().err().map(String::as_str)).await;
    }
}

fn base(state: &AppState) -> String {
    state.config.public_url.clone().unwrap_or_default()
}

fn app_name(kind: &str) -> &'static str {
    match kind {
        "hermes" => "Hermes",
        "openclaw" => "OpenClaw",
        _ => "The webhook",
    }
}

async fn send(state: &AppState, t: &UpdateTarget, payload: &Value, id: &str) -> Result<(), String> {
    let (url, pin) = check_url(&t.url, state.config.private_webhooks).await?;
    let mut http = reqwest::Client::builder().timeout(Duration::from_secs(15)).redirect(reqwest::redirect::Policy::none());
    if let Some((host, addr)) = pin {
        // Connect to the address that was checked, not whatever the name resolves to next.
        http = http.resolve(&host, addr);
    }
    let http = http.build().map_err(|e| e.to_string())?;
    let req = match t.kind.as_str() {
        "openclaw" => http
            .post(url)
            .bearer_auth(&t.secret)
            .header("Idempotency-Key", id)
            .json(&json!({ "message": payload["text"], "name": "Sidefleet" })),
        _ => {
            let body = payload.to_string();
            let ts = chrono::Utc::now().timestamp().to_string();
            let sig = hex(&hmac_sha256(t.secret.as_bytes(), format!("{ts}.{body}").as_bytes()));
            http.post(url)
                .header("Content-Type", "application/json")
                .header("X-Webhook-Timestamp", ts)
                .header("X-Webhook-Signature-V2", sig)
                .header("X-Sidefleet-Event", payload["event_type"].as_str().unwrap_or(""))
                .header("X-Sidefleet-Delivery", id)
                .body(body)
        }
    };
    let app = app_name(&t.kind);
    match req.send().await {
        Ok(r) if r.status().is_success() => Ok(()),
        Ok(r) => Err(match r.status().as_u16() {
            401 | 403 => format!("{app} refused the update ({}): its secret doesn't match.", r.status()),
            404 => format!("{app} answered 404: nothing listens at that address. Check the address and that its webhooks are on."),
            _ => format!("{app} answered {}.", r.status()),
        }),
        // The error's URL can carry query parameters: leave it out.
        Err(e) if e.is_timeout() => Err(format!("{app} didn't answer within 15 seconds.")),
        Err(e) if e.is_connect() => Err(format!("Couldn't reach {}: is it running, and reachable from the internet?", app.to_lowercase().replace("the ", ""))),
        Err(e) => Err(format!("Couldn't send to {app}: {}", e.without_url())),
    }
}

/// An address updates may go to: http(s), and unless `private_ok` (a self-hosted
/// control plane, which may well talk to a machine on its own network), one whose
/// name resolves only to public addresses; that address is returned to connect to.
pub async fn check_url(raw: &str, private_ok: bool) -> Result<(reqwest::Url, Option<(String, SocketAddr)>), String> {
    let url = reqwest::Url::parse(raw.trim()).map_err(|_| "That isn't a web address; it should start with https://".to_string())?;
    if !matches!(url.scheme(), "http" | "https") || !url.username().is_empty() || url.password().is_some() {
        return Err("The address must start with https:// or http://, without a user name or password.".into());
    }
    let host = url.host_str().ok_or("The address has no host.")?.to_string();
    if private_ok {
        return Ok((url, None));
    }
    let port = url.port_or_known_default().unwrap_or(443);
    let addrs: Vec<SocketAddr> = tokio::net::lookup_host((host.trim_matches(|c| c == '[' || c == ']'), port))
        .await
        .map_err(|_| format!("Couldn't find {host}. Check the address."))?
        .collect();
    if addrs.is_empty() || addrs.iter().any(|a| !is_public(a.ip())) {
        return Err(format!("{host} is a private or local address, which this Sidefleet can't reach. Use the app's public address."));
    }
    Ok((url, Some((host, addrs[0]))))
}

fn is_public(ip: IpAddr) -> bool {
    match ip {
        IpAddr::V4(v4) => {
            let o = v4.octets();
            !(v4.is_loopback()
                || v4.is_private()
                || v4.is_link_local()
                || v4.is_unspecified()
                || v4.is_broadcast()
                || v4.is_documentation()
                || v4.is_multicast()
                || o[0] == 0
                || (o[0] == 100 && (64..128).contains(&o[1])) // carrier-grade NAT, Tailscale
                || (o[0] == 198 && (18..20).contains(&o[1]))) // benchmarking
        }
        IpAddr::V6(v6) => {
            if let Some(v4) = v6.to_ipv4_mapped() {
                return is_public(IpAddr::V4(v4));
            }
            let s = v6.segments();
            !(v6.is_loopback() || v6.is_unspecified() || v6.is_multicast() || (s[0] & 0xfe00) == 0xfc00 || (s[0] & 0xffc0) == 0xfe80)
        }
    }
}

pub fn hmac_sha256(key: &[u8], msg: &[u8]) -> [u8; 32] {
    let mut k = [0u8; 64];
    if key.len() > 64 {
        k[..32].copy_from_slice(&Sha256::digest(key));
    } else {
        k[..key.len()].copy_from_slice(key);
    }
    let pad = |b: u8| k.iter().map(|x| x ^ b).collect::<Vec<u8>>();
    let inner = Sha256::new().chain_update(pad(0x36)).chain_update(msg).finalize();
    Sha256::new().chain_update(pad(0x5c)).chain_update(inner).finalize().into()
}

pub fn hex(b: &[u8]) -> String {
    b.iter().map(|x| format!("{x:02x}")).collect()
}

// ---- what the assistant is told ----
// Each update is written for the assistant, which turns it into a message in the
// user's words: what happened, then what it can do next with Sidefleet's tools.

fn title(task: &Value) -> String {
    let t = task["task"].as_str().unwrap_or("").lines().map(str::trim).find(|l| !l.is_empty()).unwrap_or("");
    if t.chars().count() > 100 { format!("{}…", t.chars().take(100).collect::<String>()) } else { t.to_string() }
}

fn repo(task: &Value) -> String {
    let r = task["repo"].as_str().unwrap_or("");
    let parts: Vec<&str> = r.split('/').collect();
    if parts.len() >= 3 && parts[0].contains('.') { parts[1..].join("/") } else { r.to_string() }
}

fn link(task: &Value) -> String {
    match task["url"].as_str() {
        Some(u) if u.starts_with("http") => format!(" Page: {u}"),
        _ => String::new(),
    }
}

fn files(task: &Value) -> String {
    let c = &task["changed_files"];
    let n = c["count"].as_i64().unwrap_or(0);
    if n == 0 {
        return "No files changed.".into();
    }
    let names: Vec<&str> = c["files"].as_array().into_iter().flatten().take(5).filter_map(|f| f["path"].as_str()).collect();
    let more = if n as usize > names.len() { format!(" and {} more", n as usize - names.len()) } else { String::new() };
    format!("Changed {n} file{} (+{} -{}): {}{more}.", if n == 1 { "" } else { "s" }, c["additions"], c["deletions"], names.join(", "))
}

fn reply(task: &Value) -> String {
    match task["reply"].as_str() {
        Some(r) => {
            let r = if r.chars().count() > 1500 { format!("…{}", r.chars().skip(r.chars().count() - 1500).collect::<String>()) } else { r.to_string() };
            format!(" The coding agent's last message:\n\"\"\"\n{r}\n\"\"\"\n")
        }
        None => " ".into(),
    }
}

fn finished_text(task: &Value) -> String {
    let next = if task["pr"].is_null() && task["changed_files"]["count"].as_i64().unwrap_or(0) > 0 {
        "Offer to open a pull request (Sidefleet's open_pull_request tool) or to reply to the agent with changes."
    } else {
        "Offer to reply to the agent if they want more."
    };
    format!(
        "Sidefleet update: the task \"{}\" in {} is done.{}{} Tell the user in a short message, in your own words. {next} Task id: {}.{}",
        title(task),
        repo(task),
        reply(task),
        files(task),
        task["id"].as_str().unwrap_or(""),
        link(task)
    )
}

fn failed_text(task: &Value, what: &str) -> String {
    let err = task["error"].as_str().map(|e| format!(" Error: {e}.")).unwrap_or_default();
    format!(
        "Sidefleet update: the task \"{}\" in {} {what}.{err}{} Tell the user briefly and offer to try again (reply to the task) if it makes sense. Task id: {}.{}",
        title(task),
        repo(task),
        reply(task),
        task["id"].as_str().unwrap_or(""),
        link(task)
    )
}

fn pr_text(task: &Value) -> String {
    format!(
        "Sidefleet update: pull request #{} is open for the task \"{}\" in {}: {}. Tell the user and share the link.",
        task["pr"]["number"],
        title(task),
        repo(task),
        task["pr"]["url"].as_str().unwrap_or("")
    )
}

fn approval_text(task: &Value, a: &Approval) -> String {
    let input = ["command", "file_path", "url", "pattern", "path"]
        .iter()
        .find_map(|k| a.input[*k].as_str())
        .map(|v| format!(": `{}`", v.lines().next().unwrap_or("").chars().take(300).collect::<String>()))
        .unwrap_or_default();
    let mins = (a.expires_at - a.created_at).num_minutes().max(1);
    format!(
        "Sidefleet update: the task \"{}\" in {} is waiting for the user's OK to use {}{input}. Ask the user whether to allow it. \
         If they say yes, use Sidefleet's approve tool; if no, deny (with their reason). Task id: {}, approval_id: {}. \
         It waits about {mins} minutes, then goes on without it.{}",
        title(task),
        repo(task),
        a.tool_name,
        task["id"].as_str().unwrap_or(""),
        a.id,
        link(task)
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hmac_matches_rfc_4231() {
        assert_eq!(hex(&hmac_sha256(b"Jefe", b"what do ya want for nothing?")), "5bdcc146bf60754e6a042426089575c75a003f089d2739839dec58b964ec3843");
        // A key longer than the block is hashed first (test case 6).
        assert_eq!(
            hex(&hmac_sha256(&[0xaa; 131], b"Test Using Larger Than Block-Size Key - Hash Key First")),
            "60e431591ee0b67f0d8a26aacbf5b77f8e0bc6213728c5140546040f0ee37f54"
        );
    }

    #[test]
    fn only_public_addresses_when_hosted() {
        for ip in ["127.0.0.1", "10.0.0.5", "192.168.1.2", "172.16.0.1", "169.254.169.254", "100.101.102.103", "0.0.0.0", "::1", "fd00::1", "fe80::1", "::ffff:127.0.0.1"] {
            assert!(!is_public(ip.parse().unwrap()), "{ip}");
        }
        for ip in ["1.1.1.1", "93.184.216.34", "2606:4700::1111"] {
            assert!(is_public(ip.parse().unwrap()), "{ip}");
        }
    }

    #[tokio::test]
    async fn addresses_are_checked() {
        assert!(check_url("ftp://example.com", true).await.is_err());
        assert!(check_url("https://user:pw@example.com", true).await.is_err());
        assert!(check_url("not a url", true).await.is_err());
        assert!(check_url("http://127.0.0.1:8644/webhooks/sidefleet", true).await.is_ok());
        let e = check_url("http://127.0.0.1:8644/webhooks/sidefleet", false).await.unwrap_err();
        assert!(e.contains("private or local"), "{e}");
        assert!(check_url("http://localhost:18789/hooks/agent", false).await.is_err());
    }
}
