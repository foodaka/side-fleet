//! What a task changed. The working tree (tracked and untracked files, minus
//! ignored ones) is snapshotted into a git tree before and after each turn,
//! using a throwaway index so the user's own index is never touched. The diff
//! between the two is the turn's changes; the diff from the tree recorded when
//! the thread started is everything the task has changed so far.

use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use serde_json::{json, Value};
use sidefleet_core::redact;
use uuid::Uuid;

/// Patches past this are sent as stats only. Lock files never carry a patch.
const MAX_FILE_PATCH: usize = 96 * 1024;
/// Total patch text per diff, so one event stays well under the request limit.
const MAX_PATCH: usize = 384 * 1024;
/// More files than this are listed by name only.
const MAX_FILES: usize = 300;

/// The tree id of the working tree as it is now.
pub async fn snapshot(cwd: &Path) -> Result<String> {
    let real_index = PathBuf::from(git(cwd, &["rev-parse", "--path-format=absolute", "--git-path", "index"], &[]).await?);
    let tmp = std::env::temp_dir().join(format!("sidefleet-index-{}", Uuid::new_v4().simple()));
    // Starting from the real index lets git reuse its stat cache instead of rehashing every file.
    if std::fs::copy(&real_index, &tmp).is_err() {
        let _ = std::fs::remove_file(&tmp);
    }
    let env = [("GIT_INDEX_FILE", tmp.to_string_lossy().to_string())];
    let res = async {
        git(cwd, &["add", "-A", "--", "."], &env).await?;
        git(cwd, &["write-tree"], &env).await
    }
    .await;
    let _ = std::fs::remove_file(&tmp);
    res
}

/// Where the tree a thread started from is kept: one file per root run in the
/// repo's git dir. Nothing references the tree, so git may prune it after
/// a couple of weeks; a thread that old just shows per-turn changes.
async fn base_file(cwd: &Path, root_id: Uuid) -> Result<PathBuf> {
    let common = git(cwd, &["rev-parse", "--path-format=absolute", "--git-common-dir"], &[]).await?;
    Ok(PathBuf::from(common).join("sidefleet").join("bases").join(root_id.to_string()))
}

/// The tree the thread started from, recording `before` as that tree on the first turn.
pub async fn thread_base(cwd: &Path, root_id: Uuid, before: &str) -> Option<String> {
    let path = base_file(cwd, root_id).await.ok()?;
    if let Ok(s) = std::fs::read_to_string(&path) {
        let tree = s.trim().to_string();
        if git(cwd, &["cat-file", "-e", &format!("{tree}^{{tree}}")], &[]).await.is_ok() {
            return Some(tree);
        }
    }
    std::fs::create_dir_all(path.parent()?).ok()?;
    std::fs::write(&path, format!("{before}\n")).ok()?;
    Some(before.to_string())
}

/// The `diff` event payload, or None when nothing changed in this turn.
/// `turn` is this turn's changes; `total` is everything since the thread
/// started, left out when it is the same as `turn`.
pub async fn diff_event(cwd: &Path, base: Option<&str>, before: &str, after: &str) -> Option<Value> {
    if before == after {
        return None;
    }
    let turn = diff(cwd, before, after).await.ok()?;
    let total = match base {
        Some(b) if b != before => diff(cwd, b, after).await.ok(),
        _ => None,
    };
    Some(json!({ "turn": turn, "total": total }))
}

async fn diff(cwd: &Path, from: &str, to: &str) -> Result<Value> {
    let patch = git(
        cwd,
        &[
            "-c", "core.quotePath=false", "diff", "--no-color", "--no-ext-diff", "--no-textconv", "-M",
            "--src-prefix=a/", "--dst-prefix=b/", from, to,
        ],
        &[],
    )
    .await?;
    Ok(summarize(&patch))
}

/// Split a unified diff into files with their stats, keeping each patch while
/// the budget lasts. Secrets in changed lines are redacted like any other output.
pub fn summarize(patch: &str) -> Value {
    let mut files = Vec::new();
    let (mut adds, mut dels, mut budget) = (0u64, 0u64, MAX_PATCH);
    for chunk in split_files(patch) {
        let f = FileHeader::parse(chunk);
        let (a, d) = count(chunk);
        adds += a;
        dels += d;
        if files.len() >= MAX_FILES {
            continue;
        }
        let omitted = if f.binary {
            Some("binary")
        } else if is_generated(&f.path) {
            Some("generated")
        } else if chunk.len() > MAX_FILE_PATCH {
            Some("large")
        } else if chunk.len() > budget {
            Some("budget")
        } else {
            None
        };
        let body = match omitted {
            None => {
                budget -= chunk.len();
                Value::String(redact(hunks(chunk)))
            }
            Some(_) => Value::Null,
        };
        files.push(json!({
            "path": f.path,
            "old_path": f.old_path,
            "status": f.status,
            "additions": a,
            "deletions": d,
            "patch": body,
            "omitted": omitted,
        }));
    }
    let count = split_files(patch).count();
    json!({ "files": files, "file_count": count, "additions": adds, "deletions": dels })
}

fn split_files(patch: &str) -> impl Iterator<Item = &str> {
    let mut starts: Vec<usize> = patch.match_indices("diff --git ").filter(|(i, _)| *i == 0 || patch.as_bytes()[i - 1] == b'\n').map(|(i, _)| i).collect();
    starts.push(patch.len());
    (0..starts.len().saturating_sub(1)).map(move |i| &patch[starts[i]..starts[i + 1]])
}

/// The chunk from its first hunk on; the header lines are summarised separately.
fn hunks(chunk: &str) -> &str {
    match chunk.find("\n@@") {
        Some(i) => &chunk[i + 1..],
        None => "",
    }
}

fn count(chunk: &str) -> (u64, u64) {
    let (mut a, mut d) = (0, 0);
    for l in hunks(chunk).lines() {
        if l.starts_with('+') {
            a += 1;
        } else if l.starts_with('-') {
            d += 1;
        }
    }
    (a, d)
}

fn is_generated(path: &str) -> bool {
    let name = path.rsplit('/').next().unwrap_or(path);
    matches!(
        name,
        "Cargo.lock" | "package-lock.json" | "pnpm-lock.yaml" | "yarn.lock" | "bun.lockb" | "bun.lock" | "poetry.lock"
            | "uv.lock" | "Gemfile.lock" | "composer.lock" | "go.sum" | "flake.lock" | "Pipfile.lock"
    ) || name.ends_with(".min.js")
        || name.ends_with(".min.css")
        || name.ends_with(".map")
}

struct FileHeader {
    path: String,
    old_path: Option<String>,
    status: &'static str,
    binary: bool,
}

impl FileHeader {
    fn parse(chunk: &str) -> FileHeader {
        let (mut minus, mut plus, mut rename_from, mut rename_to) = (None, None, None, None);
        let (mut status, mut binary) = ("modified", false);
        for l in chunk.lines() {
            if l.starts_with("@@") {
                break;
            }
            if let Some(p) = l.strip_prefix("--- ") {
                minus = path_of(p, "a/");
            } else if let Some(p) = l.strip_prefix("+++ ") {
                plus = path_of(p, "b/");
            } else if let Some(p) = l.strip_prefix("rename from ") {
                rename_from = Some(unquote(p));
            } else if let Some(p) = l.strip_prefix("rename to ") {
                rename_to = Some(unquote(p));
            } else if l.starts_with("new file mode") {
                status = "added";
            } else if l.starts_with("deleted file mode") {
                status = "deleted";
            } else if l.starts_with("Binary files ") || l == "GIT binary patch" {
                binary = true;
            }
        }
        if rename_to.is_some() {
            status = "renamed";
        }
        // Binary files and pure renames have no ---/+++ lines; fall back to the `diff --git` line.
        let from_header = || {
            let first = chunk.lines().next().unwrap_or("");
            let rest = first.strip_prefix("diff --git a/").unwrap_or(first);
            match rest.find(" b/") {
                Some(i) => (rest[..i].to_string(), rest[i + 3..].to_string()),
                None => (rest.to_string(), rest.to_string()),
            }
        };
        let path = rename_to.clone().or(plus.clone()).or(minus.clone()).unwrap_or_else(|| from_header().1);
        let old_path = rename_from.or_else(|| match (&minus, &plus) {
            (Some(m), Some(p)) if m != p => Some(m.clone()),
            _ => None,
        });
        FileHeader { path, old_path, status, binary }
    }
}

fn path_of(p: &str, prefix: &str) -> Option<String> {
    let p = unquote(p.trim_end_matches('\t'));
    if p == "/dev/null" {
        return None;
    }
    Some(p.strip_prefix(prefix).unwrap_or(&p).to_string())
}

/// Git quotes names with control characters or quotes in them.
fn unquote(p: &str) -> String {
    match p.strip_prefix('"').and_then(|s| s.strip_suffix('"')) {
        Some(inner) => inner.replace("\\\"", "\"").replace("\\\\", "\\").replace("\\t", "\t").replace("\\n", "\n"),
        None => p.to_string(),
    }
}

async fn git(cwd: &Path, args: &[&str], env: &[(&str, String)]) -> Result<String> {
    let out = tokio::process::Command::new("git")
        .arg("-C")
        .arg(cwd)
        .args(args)
        .envs(env.iter().map(|(k, v)| (*k, v.as_str())))
        .stdin(std::process::Stdio::null())
        .output()
        .await
        .context("running git")?;
    if !out.status.success() {
        anyhow::bail!("git {} failed: {}", args.join(" "), String::from_utf8_lossy(&out.stderr).trim());
    }
    let s = String::from_utf8_lossy(&out.stdout);
    // `diff` output is kept as is; everything else is a single trimmed value.
    Ok(if args.contains(&"diff") { s.into_owned() } else { s.trim().to_string() })
}

#[cfg(test)]
mod tests {
    use super::*;

    const PATCH: &str = "diff --git a/src/a.rs b/src/a.rs
index 1111111..2222222 100644
--- a/src/a.rs
+++ b/src/a.rs
@@ -1,3 +1,3 @@
 fn main() {
-    println!(\"hi\");
+    println!(\"hello\");
 }
diff --git a/new.txt b/new.txt
new file mode 100644
index 0000000..3333333
--- /dev/null
+++ b/new.txt
@@ -0,0 +1,2 @@
+one
+two
diff --git a/old name.md b/docs/new name.md
similarity 90%
rename from old name.md
rename to docs/new name.md
index 4444444..5555555 100644
--- a/old name.md
+++ b/docs/new name.md
@@ -1 +1 @@
-x
+y
diff --git a/logo.png b/logo.png
new file mode 100644
index 0000000..6666666
Binary files /dev/null and b/logo.png differ
diff --git a/Cargo.lock b/Cargo.lock
index 7777777..8888888 100644
--- a/Cargo.lock
+++ b/Cargo.lock
@@ -1 +1 @@
-a
+b
";

    #[test]
    fn splits_files_with_status_stats_and_patches() {
        let v = summarize(PATCH);
        assert_eq!(v["file_count"], 5);
        assert_eq!(v["additions"], 5);
        assert_eq!(v["deletions"], 3);
        let f = v["files"].as_array().unwrap();
        assert_eq!(f[0]["path"], "src/a.rs");
        assert_eq!(f[0]["status"], "modified");
        assert_eq!((f[0]["additions"].as_u64(), f[0]["deletions"].as_u64()), (Some(1), Some(1)));
        assert!(f[0]["patch"].as_str().unwrap().starts_with("@@ -1,3 +1,3 @@\n fn main() {"));
        assert_eq!(f[1]["status"], "added");
        assert_eq!(f[1]["path"], "new.txt");
        assert!(f[1]["old_path"].is_null());
        assert_eq!(f[2]["status"], "renamed");
        assert_eq!(f[2]["path"], "docs/new name.md");
        assert_eq!(f[2]["old_path"], "old name.md");
        assert_eq!(f[3]["path"], "logo.png");
        assert_eq!(f[3]["omitted"], "binary");
        assert!(f[3]["patch"].is_null());
        assert_eq!(f[4]["omitted"], "generated");
        assert_eq!(f[4]["additions"], 1);
    }

    #[test]
    fn patches_are_redacted() {
        let p = "diff --git a/.env b/.env\n--- a/.env\n+++ b/.env\n@@ -0,0 +1 @@\n+GITHUB_TOKEN=ghp_abcdefghijklmnopqrstuvwxyz123456\n";
        let v = summarize(p);
        let body = v["files"][0]["patch"].as_str().unwrap();
        assert!(!body.contains("ghp_abc"), "{body}");
    }

    fn sh(dir: &Path, args: &[&str]) {
        let out = std::process::Command::new("git")
            .arg("-C").arg(dir).args(args)
            .env("GIT_AUTHOR_NAME", "t").env("GIT_AUTHOR_EMAIL", "t@t").env("GIT_COMMITTER_NAME", "t").env("GIT_COMMITTER_EMAIL", "t@t")
            .output().unwrap();
        assert!(out.status.success(), "git {args:?}: {}", String::from_utf8_lossy(&out.stderr));
    }

    #[tokio::test]
    async fn snapshots_see_untracked_files_and_leave_the_index_alone() {
        let dir = tempfile::tempdir().unwrap();
        let d = dir.path();
        sh(d, &["init", "-q", "-b", "main"]);
        std::fs::write(d.join("a.txt"), "one\n").unwrap();
        std::fs::write(d.join(".gitignore"), "target/\n").unwrap();
        sh(d, &["add", "."]);
        sh(d, &["commit", "-q", "-m", "init"]);

        let before = snapshot(d).await.unwrap();
        let root = Uuid::new_v4();
        assert_eq!(thread_base(d, root, &before).await.as_deref(), Some(before.as_str()));
        assert!(diff_event(d, Some(&before), &before, &snapshot(d).await.unwrap()).await.is_none(), "no change, no event");

        std::fs::write(d.join("a.txt"), "one\ntwo\n").unwrap();
        std::fs::write(d.join("b.txt"), "new\n").unwrap();
        std::fs::create_dir_all(d.join("target")).unwrap();
        std::fs::write(d.join("target/out"), "ignored\n").unwrap();
        let mid = snapshot(d).await.unwrap();
        let ev = diff_event(d, Some(&before), &before, &mid).await.unwrap();
        let paths: Vec<&str> = ev["turn"]["files"].as_array().unwrap().iter().map(|f| f["path"].as_str().unwrap()).collect();
        assert_eq!(paths, vec!["a.txt", "b.txt"]);
        assert!(ev["total"].is_null(), "first turn: total is the turn");

        // The second turn sees only its own change; the total covers both.
        std::fs::remove_file(d.join("b.txt")).unwrap();
        std::fs::write(d.join("c.txt"), "c\n").unwrap();
        let after = snapshot(d).await.unwrap();
        let base = thread_base(d, root, &mid).await.unwrap();
        assert_eq!(base, before, "the base is kept from the first turn");
        let ev = diff_event(d, Some(&base), &mid, &after).await.unwrap();
        assert_eq!(ev["turn"]["file_count"], 2);
        let total: Vec<&str> = ev["total"]["files"].as_array().unwrap().iter().map(|f| f["path"].as_str().unwrap()).collect();
        assert_eq!(total, vec!["a.txt", "c.txt"]);

        let status = std::process::Command::new("git").arg("-C").arg(d).args(["status", "--porcelain"]).output().unwrap();
        let status = String::from_utf8_lossy(&status.stdout);
        assert!(status.contains(" M a.txt") && status.contains("?? c.txt"), "real index untouched: {status}");
    }
}
