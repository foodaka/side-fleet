//! What a task changed. The working tree (tracked and untracked files, minus
//! ignored ones) is snapshotted into a git tree before and after each turn,
//! using a throwaway index so the user's own index is never touched. The diff
//! between the two is the turn's changes; the diff from the tree recorded when
//! the thread started is everything the task has changed so far.
//!
//! When a task catches up with the main branch (a merge or rebase), the
//! commits it brings in are not the task's work: both diffs then count from
//! where the task's branch now meets the main branch, as a pull request would.

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

/// A working tree and the commit checked out when it was taken.
#[derive(Clone, Debug, PartialEq)]
pub struct Point {
    pub tree: String,
    pub commit: Option<String>,
}

/// The working tree as it is now, with the commit it sits on.
pub async fn point(cwd: &Path) -> Result<Point> {
    let tree = snapshot(cwd).await?;
    let commit = git(cwd, &["rev-parse", "--verify", "-q", "HEAD"], &[]).await.ok();
    Ok(Point { tree, commit })
}

/// Where the tree a thread started from is kept: one file per root run in the
/// repo's git dir. Nothing references the tree, so git may prune it after
/// a couple of weeks; a thread that old just shows per-turn changes.
async fn base_file(cwd: &Path, root_id: Uuid) -> Result<PathBuf> {
    let common = git(cwd, &["rev-parse", "--path-format=absolute", "--git-common-dir"], &[]).await?;
    Ok(PathBuf::from(common).join("sidefleet").join("bases").join(root_id.to_string()))
}

/// Where the thread started, recording `before` as that point on the first turn.
/// Bases written before commits were recorded hold only the tree.
pub async fn thread_base(cwd: &Path, root_id: Uuid, before: &Point) -> Option<Point> {
    let path = base_file(cwd, root_id).await.ok()?;
    if let Ok(s) = std::fs::read_to_string(&path) {
        let mut parts = s.split_whitespace();
        if let Some(tree) = parts.next() {
            if git(cwd, &["cat-file", "-e", &format!("{tree}^{{tree}}")], &[]).await.is_ok() {
                return Some(Point { tree: tree.to_string(), commit: parts.next().map(str::to_string) });
            }
        }
    }
    std::fs::create_dir_all(path.parent()?).ok()?;
    let line = match &before.commit {
        Some(c) => format!("{} {c}\n", before.tree),
        None => format!("{}\n", before.tree),
    };
    std::fs::write(&path, line).ok()?;
    Some(before.clone())
}

/// The tree to diff from, starting at `p`: `p`'s own tree, or, when the task
/// has since caught up with the main branch, that branch where the task now
/// meets it, with how many commits came in and from where.
/// The task's work in `p` is carried over with a three-way merge; if that
/// conflicts, `p` stays where it was.
async fn caught_up(cwd: &Path, p: &Point) -> (String, Option<Value>) {
    let unchanged = (p.tree.clone(), None);
    let Some(start) = &p.commit else { return unchanged };
    let Some(up) = upstream(cwd).await else { return unchanged };
    let (Ok(old), Ok(new)) = (
        git(cwd, &["merge-base", start, &up], &[]).await,
        git(cwd, &["merge-base", "HEAD", &up], &[]).await,
    ) else {
        return unchanged;
    };
    if old == new || git(cwd, &["merge-base", "--is-ancestor", &old, &new], &[]).await.is_err() {
        return unchanged;
    }
    let (Ok(tree), Ok(n)) = (
        moved_tree(cwd, &old, &p.tree, &new).await,
        git(cwd, &["rev-list", "--count", &format!("{old}..{new}")], &[]).await,
    ) else {
        return unchanged;
    };
    let commits: u64 = n.parse().unwrap_or(0);
    (tree, Some(json!({ "commits": commits, "upstream": up })))
}

/// `tree`, which differs from `from` by the task's work so far (committed or
/// not), with that work replayed onto `to`.
async fn moved_tree(cwd: &Path, from: &str, tree: &str, to: &str) -> Result<String> {
    if git(cwd, &["rev-parse", &format!("{from}^{{tree}}")], &[]).await? == tree {
        return git(cwd, &["rev-parse", &format!("{to}^{{tree}}")], &[]).await;
    }
    // merge-tree wants commits; this one is never referenced and git prunes it.
    let who = [
        ("GIT_AUTHOR_NAME", "sidefleet".to_string()), ("GIT_AUTHOR_EMAIL", "sidefleet@localhost".to_string()),
        ("GIT_COMMITTER_NAME", "sidefleet".to_string()), ("GIT_COMMITTER_EMAIL", "sidefleet@localhost".to_string()),
    ];
    let wip = git(cwd, &["commit-tree", tree, "-p", from, "-m", "sidefleet snapshot"], &who).await?;
    // `wip` has only `from` as its parent, and `to` descends from `from`, so
    // Git infers exactly that merge base. Avoid --merge-base, which older Git
    // versions (including Debian 12's Git 2.39) do not support.
    // Exits non-zero on conflicts, which the helper turns into an error.
    let out = git(cwd, &["merge-tree", "--write-tree", &wip, to], &[]).await?;
    Ok(out.lines().next().unwrap_or_default().to_string())
}

/// The repo's main branch: what `origin/HEAD` points at, else the usual names.
/// None when the task is on that branch itself, where catching up and the
/// task's own commits cannot be told apart.
async fn upstream(cwd: &Path) -> Option<String> {
    let mut found = None;
    if let Ok(r) = git(cwd, &["rev-parse", "--abbrev-ref", "origin/HEAD"], &[]).await {
        if r != "origin/HEAD" {
            found = Some(r);
        }
    }
    if found.is_none() {
        for r in ["origin/main", "origin/master", "main", "master"] {
            if git(cwd, &["rev-parse", "--verify", "-q", &format!("{r}^{{commit}}")], &[]).await.is_ok() {
                found = Some(r.to_string());
                break;
            }
        }
    }
    let up = found?;
    let branch = git(cwd, &["symbolic-ref", "-q", "--short", "HEAD"], &[]).await.unwrap_or_default();
    let short = up.strip_prefix("origin/").unwrap_or(&up);
    (branch != short).then_some(up)
}

/// The `diff` event payload, or None when nothing changed in this turn.
/// `turn` is this turn's changes; `total` is everything since the thread
/// started, left out when it is the same as `turn`. A turn that caught up
/// with the main branch carries `caught_up` (`commits`, `upstream`).
pub async fn diff_event(cwd: &Path, base: Option<&Point>, before: &Point, after: &str) -> Option<Value> {
    if before.tree == after {
        return None;
    }
    let (from, caught) = caught_up(cwd, before).await;
    let mut turn = diff(cwd, &from, after).await.ok()?;
    if let Some(c) = caught {
        turn["caught_up"] = c;
    }
    let total = match base {
        Some(b) => {
            let (b, _) = caught_up(cwd, b).await;
            if b != from { diff(cwd, &b, after).await.ok() } else { None }
        }
        None => None,
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

        let before = point(d).await.unwrap();
        let root = Uuid::new_v4();
        assert_eq!(thread_base(d, root, &before).await, Some(before.clone()));
        assert!(diff_event(d, Some(&before), &before, &snapshot(d).await.unwrap()).await.is_none(), "no change, no event");

        std::fs::write(d.join("a.txt"), "one\ntwo\n").unwrap();
        std::fs::write(d.join("b.txt"), "new\n").unwrap();
        std::fs::create_dir_all(d.join("target")).unwrap();
        std::fs::write(d.join("target/out"), "ignored\n").unwrap();
        let mid = point(d).await.unwrap();
        let ev = diff_event(d, Some(&before), &before, &mid.tree).await.unwrap();
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

    fn files(set: &Value) -> Vec<&str> {
        set["files"].as_array().unwrap().iter().map(|f| f["path"].as_str().unwrap()).collect()
    }

    #[tokio::test]
    async fn catching_up_with_main_is_not_counted_as_the_task_s_work() {
        let dir = tempfile::tempdir().unwrap();
        let (origin, other, d) = (dir.path().join("origin.git"), dir.path().join("other"), dir.path().join("task"));
        sh(dir.path(), &["init", "-q", "--bare", "-b", "main", origin.to_str().unwrap()]);
        sh(dir.path(), &["clone", "-q", origin.to_str().unwrap(), other.to_str().unwrap()]);
        std::fs::write(other.join("a.txt"), "one\n").unwrap();
        sh(&other, &["add", "."]);
        sh(&other, &["commit", "-q", "-m", "init"]);
        sh(&other, &["push", "-q", "origin", "HEAD:main"]);
        sh(dir.path(), &["clone", "-q", origin.to_str().unwrap(), d.to_str().unwrap()]);
        sh(&d, &["checkout", "-q", "-b", "sidefleet/task"]);
        let land = |name: &str| {
            std::fs::write(other.join(name), "upstream\n").unwrap();
            sh(&other, &["add", "."]);
            sh(&other, &["commit", "-q", "-m", name]);
            sh(&other, &["push", "-q", "origin", "HEAD:main"]);
        };
        let root = Uuid::new_v4();

        // Turn 1 commits its work.
        let p1 = point(&d).await.unwrap();
        let base = thread_base(&d, root, &p1).await.unwrap();
        std::fs::write(d.join("a.txt"), "one\ntwo\n").unwrap();
        sh(&d, &["commit", "-q", "-am", "agent"]);
        let p2 = point(&d).await.unwrap();
        let ev = diff_event(&d, Some(&base), &p1, &p2.tree).await.unwrap();
        assert_eq!(files(&ev["turn"]), vec!["a.txt"]);

        // Turn 2 merges two new commits from main, then edits c.txt.
        land("up1.txt");
        land("up2.txt");
        sh(&d, &["pull", "-q", "--no-rebase", "--no-edit", "origin", "main"]);
        std::fs::write(d.join("c.txt"), "c\n").unwrap();
        let p3 = point(&d).await.unwrap();
        let ev = diff_event(&d, Some(&base), &p2, &p3.tree).await.unwrap();
        assert_eq!(files(&ev["turn"]), vec!["c.txt"]);
        assert_eq!(ev["turn"]["caught_up"], json!({"commits": 2, "upstream": "origin/main"}));
        assert_eq!(files(&ev["total"]), vec!["a.txt", "c.txt"]);

        // Turn 3 starts with c.txt uncommitted, merges again, and adds d.txt.
        land("up3.txt");
        sh(&d, &["pull", "-q", "--no-rebase", "--no-edit", "origin", "main"]);
        std::fs::write(d.join("d.txt"), "d\n").unwrap();
        let p4 = point(&d).await.unwrap();
        let ev = diff_event(&d, Some(&base), &p3, &p4.tree).await.unwrap();
        assert_eq!(files(&ev["turn"]), vec!["d.txt"]);
        assert_eq!(ev["turn"]["caught_up"]["commits"], 1);
        assert_eq!(files(&ev["total"]), vec!["a.txt", "c.txt", "d.txt"]);

        // A later turn that does not catch up is diffed as before.
        std::fs::write(d.join("d.txt"), "d\ne\n").unwrap();
        let p5 = point(&d).await.unwrap();
        let ev = diff_event(&d, Some(&base), &p4, &p5.tree).await.unwrap();
        assert_eq!(files(&ev["turn"]), vec!["d.txt"]);
        assert!(ev["turn"].get("caught_up").is_none());
    }

    #[tokio::test]
    async fn work_on_the_main_branch_itself_is_never_moved() {
        let dir = tempfile::tempdir().unwrap();
        let d = dir.path();
        sh(d, &["init", "-q", "-b", "main"]);
        std::fs::write(d.join("a.txt"), "one\n").unwrap();
        sh(d, &["add", "."]);
        sh(d, &["commit", "-q", "-m", "init"]);
        let p1 = point(d).await.unwrap();
        std::fs::write(d.join("a.txt"), "two\n").unwrap();
        sh(d, &["commit", "-q", "-am", "agent"]);
        let ev = diff_event(d, Some(&p1), &p1, &snapshot(d).await.unwrap()).await.unwrap();
        assert_eq!(files(&ev["turn"]), vec!["a.txt"]);
        assert!(ev["turn"].get("caught_up").is_none());
    }
}
