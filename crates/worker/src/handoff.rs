//! Sending a thread's files to a fork that runs on another machine. The machine
//! with the files pushes the snapshot the fork starts from to a short-lived
//! branch on `origin` (`sidefleet/handoff-<fork>`); the fork's machine fetches
//! it, sets its worktree up from it and deletes the branch.
//!
//! Nothing that looks like a secret is pushed: every file the push would send
//! that `origin` doesn't already have is checked, by name and by content, and
//! the push is refused if any looks like a credential. The person's own pre-push
//! hooks run as well.

use std::collections::BTreeMap;
use std::path::Path;
use std::process::Stdio;
use std::time::Duration;

use anyhow::{Context, Result};
use tokio::io::AsyncWriteExt;
use uuid::Uuid;

use crate::checkpoint::{self, git};
use crate::snapshot;

const NETWORK_TIMEOUT: Duration = Duration::from_secs(120);
/// Larger files are only checked by name (they are rarely text anyone typed a key into).
const MAX_SCANNED_FILE: u64 = 8 * 1024 * 1024;
/// More new content than this is not checked at all: the push is refused instead.
const MAX_SCANNED_TOTAL: u64 = 64 * 1024 * 1024;

/// A file the push would send that looks like it holds a secret.
#[derive(Debug, Clone, PartialEq)]
pub struct Finding {
    pub path: String,
    pub why: &'static str,
}

/// Push `snapshot` (a commit in `repo`) to the fork's handoff branch on `origin`,
/// unless something it would send looks like a secret. Returns the branch.
pub async fn send(repo: &Path, snapshot: &str, fork_id: Uuid) -> Result<String> {
    if !snapshot::is_commit_id(snapshot) {
        anyhow::bail!("handoff run without a snapshot to send");
    }
    git(repo, &["cat-file", "-e", &format!("{snapshot}^{{commit}}")], None)
        .await
        .context("the files after that message are no longer kept on this machine")?;
    let branch = sidefleet_core::handoff_branch(fork_id);
    // Learn what origin already has, so only what is new to it is checked (and sent).
    if let Err(e) = git(repo, &["fetch", "--quiet", "--no-tags", "origin"], Some(NETWORK_TIMEOUT)).await {
        tracing::warn!(error = %e, "fetching origin before a handoff failed; checking everything not known to be there");
    }
    let found = secrets_in_new_files(repo, snapshot).await?;
    if !found.is_empty() {
        anyhow::bail!("not sending the files: {}", describe(&found));
    }
    // Not --no-verify: the person's own pre-push checks get their say too.
    git(repo, &["push", "--quiet", "origin", &format!("{snapshot}:refs/heads/{branch}")], Some(NETWORK_TIMEOUT))
        .await
        .context("pushing the files to origin")?;
    Ok(branch)
}

/// Fetch a fork's files from its handoff branch: the snapshot commit to start from.
pub async fn receive(repo: &Path, branch: &str) -> Result<String> {
    if !branch.starts_with("sidefleet/handoff-") {
        anyhow::bail!("{branch} is not a handoff branch");
    }
    let r = checkpoint::fetch_branch(repo, branch).await?;
    let sha = git(repo, &["rev-parse", "--verify", &format!("{r}^{{commit}}")], None).await?;
    Ok(sha)
}

/// Delete the handoff branch from `origin` once the fork has its files.
pub async fn clean_up(repo: &Path, branch: &str) {
    if !branch.starts_with("sidefleet/handoff-") {
        return;
    }
    if let Err(e) = git(repo, &["push", "--quiet", "--no-verify", "origin", "--delete", branch], Some(NETWORK_TIMEOUT)).await {
        tracing::warn!(branch, error = %e, "could not delete the handoff branch from origin");
    }
    let _ = git(repo, &["update-ref", "-d", &format!("refs/remotes/origin/{branch}")], None).await;
}

fn describe(found: &[Finding]) -> String {
    let mut parts: Vec<String> = found.iter().take(5).map(|f| format!("{} looks like {}", f.path, f.why)).collect();
    if found.len() > 5 {
        parts.push(format!("and {} more", found.len() - 5));
    }
    parts.join("; ")
}

/// Every file reachable from `tip` that `origin` doesn't have yet (as far as this
/// repo knows) and that looks like it holds a secret, by name or by content.
pub async fn secrets_in_new_files(repo: &Path, tip: &str) -> Result<Vec<Finding>> {
    // `<sha> <path>` for every object the push would send; commits and the root tree have no path.
    let listed = git(repo, &["rev-list", "--objects", tip, "--not", "--remotes=origin"], None).await?;
    let mut paths: BTreeMap<String, String> = BTreeMap::new();
    for line in listed.lines() {
        if let Some((sha, path)) = line.split_once(' ') {
            if !path.is_empty() {
                paths.entry(sha.to_string()).or_insert_with(|| path.to_string());
            }
        }
    }
    if paths.is_empty() {
        return Ok(vec![]);
    }
    // Which of them are files, and how big.
    let input: String = paths.keys().map(|s| format!("{s}\n")).collect();
    let checked = git_with_input(repo, &["cat-file", "--batch-check=%(objectname) %(objecttype) %(objectsize)"], input.as_bytes()).await?;
    let mut found = Vec::new();
    let mut to_read = Vec::new();
    let mut total = 0u64;
    for line in String::from_utf8_lossy(&checked).lines() {
        let mut it = line.split(' ');
        let (Some(sha), Some("blob"), Some(size)) = (it.next(), it.next(), it.next()) else { continue };
        let Some(path) = paths.get(sha) else { continue };
        if let Some(why) = secret_file_name(path) {
            found.push(Finding { path: path.clone(), why });
            continue;
        }
        let size: u64 = size.parse().unwrap_or(u64::MAX);
        if size <= MAX_SCANNED_FILE {
            total += size;
            to_read.push(sha.to_string());
        }
    }
    if total > MAX_SCANNED_TOTAL {
        anyhow::bail!(
            "not sending the files: there are {} MB of changes origin doesn't have, too many to check for secrets",
            total / (1024 * 1024)
        );
    }
    if !to_read.is_empty() {
        let input: String = to_read.iter().map(|s| format!("{s}\n")).collect();
        let out = git_with_input(repo, &["cat-file", "--batch"], input.as_bytes()).await?;
        for (sha, content) in parse_batch(&out) {
            // Binary files: not text anyone pasted a key into.
            if content.iter().take(8000).any(|&b| b == 0) {
                continue;
            }
            if let Some(why) = sidefleet_core::find_credential(&String::from_utf8_lossy(content)) {
                found.push(Finding { path: paths[sha].clone(), why });
            }
        }
    }
    found.sort_by(|a, b| a.path.cmp(&b.path));
    found.dedup_by(|a, b| a.path == b.path);
    Ok(found)
}

/// Files whose name says they hold secrets, whatever is in them.
pub fn secret_file_name(path: &str) -> Option<&'static str> {
    let lower = path.to_ascii_lowercase();
    let name = lower.rsplit('/').next().unwrap_or(&lower);
    const EXAMPLES: &[&str] = &[".example", ".sample", ".template", ".tmpl", ".dist", ".default", ".defaults", ".schema"];
    let example = EXAMPLES.iter().any(|e| name.ends_with(e)) || name.contains("example");
    if (name == ".env" || name.ends_with(".env") || name.starts_with(".env.")) && !example {
        return Some("an environment file");
    }
    if ["id_rsa", "id_dsa", "id_ecdsa", "id_ed25519"].contains(&name) {
        return Some("an SSH private key");
    }
    if [".pem", ".key", ".p12", ".pfx", ".jks", ".keystore", ".ppk", ".kdbx"].iter().any(|e| name.ends_with(e)) && !example {
        return Some("a key or certificate store");
    }
    if name.ends_with(".tfstate") || name.ends_with(".tfstate.backup") {
        return Some("Terraform state, which holds secrets");
    }
    if [".netrc", ".git-credentials", ".pgpass", ".htpasswd", ".pypirc", ".dockercfg", "credentials.json"].contains(&name)
        || lower.ends_with(".aws/credentials")
        || lower.ends_with(".docker/config.json")
        || (["secrets.json", "secrets.yml", "secrets.yaml", "secrets.toml"].contains(&name))
    {
        return Some("a credentials file");
    }
    None
}

/// `(sha, content)` for each object in `git cat-file --batch` output.
fn parse_batch(out: &[u8]) -> Vec<(&str, &[u8])> {
    let mut res = Vec::new();
    let mut at = 0;
    while at < out.len() {
        let Some(nl) = out[at..].iter().position(|&b| b == b'\n') else { break };
        let header = std::str::from_utf8(&out[at..at + nl]).unwrap_or("");
        at += nl + 1;
        let mut it = header.split(' ');
        let (Some(sha), Some(_kind), Some(size)) = (it.next(), it.next(), it.next()) else { continue };
        let Ok(size) = size.parse::<usize>() else { continue };
        let end = (at + size).min(out.len());
        res.push((sha, &out[at..end]));
        at = end + 1;
    }
    res
}

async fn git_with_input(dir: &Path, args: &[&str], input: &[u8]) -> Result<Vec<u8>> {
    let mut child = tokio::process::Command::new("git")
        .arg("-C")
        .arg(dir)
        .args(args)
        .env("GIT_TERMINAL_PROMPT", "0")
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .kill_on_drop(true)
        .spawn()
        .context("running git")?;
    let mut stdin = child.stdin.take().context("git stdin")?;
    let input = input.to_vec();
    // Written alongside reading, so a full pipe can't stall either side.
    let writer = tokio::spawn(async move {
        let _ = stdin.write_all(&input).await;
    });
    let out = child.wait_with_output().await.context("running git")?;
    let _ = writer.await;
    if !out.status.success() {
        anyhow::bail!("git {} failed: {}", args.join(" "), String::from_utf8_lossy(&out.stderr).trim());
    }
    Ok(out.stdout)
}

#[cfg(test)]
mod tests {
    use super::*;

    async fn sh(dir: &Path, args: &[&str]) -> String {
        git(dir, args, None).await.unwrap()
    }

    #[test]
    fn secret_file_names() {
        for p in [".env", "app/.env.production", "deploy/prod.env", "certs/server.pem", "id_ed25519", "infra/terraform.tfstate", "home/.aws/credentials", "config/secrets.yml"] {
            assert!(secret_file_name(p).is_some(), "{p}");
        }
        for p in [".env.example", ".env.sample", "src/env.rs", "README.md", "id_ed25519.pub", "docs/keys.md", "example.pem"] {
            assert!(secret_file_name(p).is_none(), "{p}");
        }
    }

    /// A repo with an `origin` that has `main`, and a snapshot on top with `files`.
    async fn setup(files: &[(&str, &str)]) -> (tempfile::TempDir, std::path::PathBuf, std::path::PathBuf, String) {
        let tmp = tempfile::tempdir().unwrap();
        let origin = tmp.path().join("origin.git");
        let repo = tmp.path().join("repo");
        std::fs::create_dir_all(&repo).unwrap();
        sh(tmp.path(), &["init", "--quiet", "--bare", origin.to_str().unwrap()]).await;
        sh(&repo, &["init", "--quiet", "-b", "main"]).await;
        // Already on origin: never counted, whatever it holds.
        std::fs::write(repo.join("old.txt"), "AKIAABCDEFGHIJKLMNOP\n").unwrap();
        sh(&repo, &["add", "."]).await;
        sh(&repo, &["-c", "user.name=t", "-c", "user.email=t@t", "commit", "--quiet", "-m", "init"]).await;
        sh(&repo, &["remote", "add", "origin", origin.to_str().unwrap()]).await;
        sh(&repo, &["push", "--quiet", "origin", "main"]).await;
        for (p, c) in files {
            let path = repo.join(p);
            std::fs::create_dir_all(path.parent().unwrap()).unwrap();
            std::fs::write(path, c).unwrap();
        }
        let tree = crate::diff::snapshot(&repo).await.unwrap();
        let snap = snapshot::keep(&repo, &tree, &snapshot::ref_name(Uuid::new_v4(), Uuid::new_v4(), "after")).await.unwrap();
        (tmp, origin, repo, snap)
    }

    #[tokio::test]
    async fn sends_clean_files_and_the_other_machine_gets_them() {
        let (tmp, origin, repo, snap) = setup(&[("src/lib.rs", "fn main() { let password = read(); }\n")]).await;
        let fork = Uuid::new_v4();
        let branch = send(&repo, &snap, fork).await.unwrap();
        assert_eq!(branch, sidefleet_core::handoff_branch(fork));
        assert_eq!(sh(&origin, &["rev-parse", &format!("refs/heads/{branch}")]).await, snap);

        let other = tmp.path().join("other");
        sh(tmp.path(), &["clone", "--quiet", origin.to_str().unwrap(), other.to_str().unwrap()]).await;
        assert_eq!(receive(&other, &branch).await.unwrap(), snap);
        clean_up(&other, &branch).await;
        assert!(git(&origin, &["rev-parse", "--verify", &format!("refs/heads/{branch}")], None).await.is_err(), "the branch is gone from origin");
    }

    #[tokio::test]
    async fn refuses_to_send_secrets_by_name_or_content() {
        let (_tmp, origin, repo, snap) = setup(&[
            (".env", "HELLO=1\n"),
            ("src/config.ts", "export const key = 'sk-ant-api03-abcdefghijklmnopqrstuvwx';\n"),
            ("src/ok.ts", "export const ok = 1;\n"),
        ])
        .await;
        let found = secrets_in_new_files(&repo, &snap).await.unwrap();
        let paths: Vec<&str> = found.iter().map(|f| f.path.as_str()).collect();
        assert_eq!(paths, [".env", "src/config.ts"], "{found:?}");
        let err = send(&repo, &snap, Uuid::new_v4()).await.unwrap_err().to_string();
        assert!(err.contains(".env looks like an environment file") && err.contains("src/config.ts"), "{err}");
        assert!(!err.contains("sk-ant"), "the secret itself is never repeated: {err}");
        assert_eq!(sh(&origin, &["for-each-ref", "--format=%(refname)"]).await, "refs/heads/main", "nothing was pushed");
    }

    #[test]
    fn parses_batch_output() {
        let out = b"aaa blob 3\nabc\nbbb blob 0\n\nccc blob 2\nx\n\n";
        let parsed = parse_batch(out);
        assert_eq!(parsed, vec![("aaa", &b"abc"[..]), ("bbb", &b""[..]), ("ccc", &b"x\n"[..])]);
    }
}
