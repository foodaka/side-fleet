//! Freeing the disk that finished tasks hold. Someone asks from the Machines page and
//! this removes every task worktree with nothing left to lose: no uncommitted or
//! untracked files, and its commits already on the default branch or merged as its
//! pull request (squash merges leave no ancestry to check).
//!
//! The task branch and snapshots stay, so a follow-up rebuilds the worktree where it
//! was. Files git ignores go with the worktree.

use std::collections::HashMap;
use std::future::Future;
use std::os::unix::fs::MetadataExt;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

use anyhow::Result;
use sidefleet_core::WorktreeCleanup;
use tokio::sync::Mutex;
use uuid::Uuid;

use crate::checkpoint::git;
use crate::pr;

/// Runs in flight: run id -> (repo, isolation, the thread's root run id).
pub type ActiveRuns = Arc<Mutex<HashMap<Uuid, (String, String, Uuid)>>>;

const TRASH: &str = ".trash";
const GIT_TIMEOUT: Option<Duration> = Some(Duration::from_secs(120));

enum Verdict {
    Remove { common_dir: String },
    InUse,
    Changes,
    Unmerged,
}

pub async fn clean(worktrees_dir: &Path, active: &ActiveRuns) -> WorktreeCleanup {
    sweep(worktrees_dir, active, |dir, branch, head| async move { pr::merged_at(&dir, &branch, &head).await }).await
}

async fn sweep<F, Fut>(worktrees_dir: &Path, active: &ActiveRuns, pr_merged: F) -> WorktreeCleanup
where
    F: Fn(PathBuf, String, String) -> Fut,
    Fut: Future<Output = bool>,
{
    let mut report = WorktreeCleanup::default();
    // Left by a sweep that was cut off.
    let leftovers = worktrees_dir.join(TRASH);
    report.freed_bytes += delete(leftovers).await.unwrap_or(0);

    for (root, path) in task_worktrees(worktrees_dir) {
        if in_use(active, root).await {
            report.kept_in_use += 1;
            continue;
        }
        let common_dir = match inspect(&path, root, &pr_merged).await {
            Ok(Verdict::Remove { common_dir }) => common_dir,
            Ok(Verdict::InUse) => {
                report.kept_in_use += 1;
                continue;
            }
            Ok(Verdict::Changes) => {
                report.kept_changes += 1;
                continue;
            }
            Ok(Verdict::Unmerged) => {
                report.kept_unmerged += 1;
                continue;
            }
            Err(e) => {
                tracing::warn!(worktree = %path.display(), error = %format!("{e:#}"), "could not check a worktree; keeping it");
                report.failed += 1;
                continue;
            }
        };
        let trash = worktrees_dir.join(TRASH).join(root.to_string());
        {
            // Held until git has forgotten the worktree: a follow-up claimed now either
            // shows up here, or arrives afterwards and rebuilds it cleanly.
            let runs = active.lock().await;
            if runs.values().any(|(_, _, r)| *r == root) {
                report.kept_in_use += 1;
                continue;
            }
            let moved = std::fs::create_dir_all(worktrees_dir.join(TRASH)).and_then(|()| std::fs::rename(&path, &trash));
            if let Err(e) = moved {
                tracing::warn!(worktree = %path.display(), error = %e, "could not remove a worktree");
                report.failed += 1;
                continue;
            }
            if let Err(e) = git(Path::new(&common_dir), &["worktree", "prune"], GIT_TIMEOUT).await {
                tracing::warn!(error = %format!("{e:#}"), "git worktree prune failed");
            }
        }
        match delete(trash).await {
            Ok(bytes) => {
                report.removed += 1;
                report.freed_bytes += bytes;
            }
            Err(e) => {
                tracing::warn!(worktree = %path.display(), error = %format!("{e:#}"), "could not delete a removed worktree's files");
                report.failed += 1;
            }
        }
    }
    report
}

async fn in_use(active: &ActiveRuns, root: Uuid) -> bool {
    active.lock().await.values().any(|(_, _, r)| *r == root)
}

/// Linked worktrees at `<dir>/<repo-slug>/<root run id>`.
fn task_worktrees(dir: &Path) -> Vec<(Uuid, PathBuf)> {
    let children = |d: &Path| std::fs::read_dir(d).into_iter().flatten().filter_map(|e| e.ok()).map(|e| e.path()).collect::<Vec<_>>();
    children(dir)
        .into_iter()
        .filter(|slug| slug.file_name().is_some_and(|n| n != TRASH))
        .flat_map(|slug| children(&slug))
        .filter(|p| p.join(".git").is_file())
        .filter_map(|p| Some((p.file_name()?.to_str()?.parse().ok()?, p)))
        .collect()
}

async fn inspect<F, Fut>(path: &Path, root: Uuid, pr_merged: &F) -> Result<Verdict>
where
    F: Fn(PathBuf, String, String) -> Fut,
    Fut: Future<Output = bool>,
{
    // Moved to another branch by hand or by the agent: not ours to judge.
    let branch = git(path, &["symbolic-ref", "--quiet", "--short", "HEAD"], GIT_TIMEOUT).await.unwrap_or_default();
    if branch != crate::worker::task_branch(root) {
        return Ok(Verdict::Unmerged);
    }
    let git_dir = git(path, &["rev-parse", "--absolute-git-dir"], GIT_TIMEOUT).await?;
    if Path::new(&git_dir).join("locked").exists() {
        return Ok(Verdict::InUse);
    }
    if !git(path, &["status", "--porcelain"], GIT_TIMEOUT).await?.is_empty() {
        return Ok(Verdict::Changes);
    }
    let head = git(path, &["rev-parse", "HEAD"], GIT_TIMEOUT).await?;
    let on_default = match git(path, &["rev-parse", "--abbrev-ref", "origin/HEAD"], GIT_TIMEOUT).await {
        Ok(default) => git(path, &["merge-base", "--is-ancestor", &head, &default], GIT_TIMEOUT).await.is_ok(),
        Err(_) => false,
    };
    if !on_default && !pr_merged(path.to_path_buf(), branch, head).await {
        return Ok(Verdict::Unmerged);
    }
    let common_dir = git(path, &["rev-parse", "--path-format=absolute", "--git-common-dir"], GIT_TIMEOUT).await?;
    Ok(Verdict::Remove { common_dir })
}

/// Delete a folder, returning the disk it held. Off the async threads: this can be gigabytes.
async fn delete(dir: PathBuf) -> Result<u64> {
    if !dir.exists() {
        return Ok(0);
    }
    Ok(tokio::task::spawn_blocking(move || -> std::io::Result<u64> {
        let bytes = disk_usage(&dir);
        std::fs::remove_dir_all(&dir)?;
        Ok(bytes)
    })
    .await??)
}

fn disk_usage(dir: &Path) -> u64 {
    let mut total = 0;
    let mut stack = vec![dir.to_path_buf()];
    while let Some(d) = stack.pop() {
        for entry in std::fs::read_dir(&d).into_iter().flatten().flatten() {
            let Ok(meta) = entry.metadata() else { continue };
            total += meta.blocks() * 512;
            if meta.is_dir() {
                stack.push(entry.path());
            }
        }
    }
    total
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::worker::{prepare_worktree, task_branch};

    struct Fixture {
        _tmp: tempfile::TempDir,
        repo: PathBuf,
        wts: PathBuf,
        active: ActiveRuns,
    }

    const COMMIT: [&str; 4] = ["-c", "user.name=t", "-c", "user.email=t@t"];

    async fn commit_all(dir: &Path, msg: &str) {
        git(dir, &["add", "-A"], None).await.unwrap();
        git(dir, &[&COMMIT[..], &["commit", "--quiet", "-m", msg]].concat(), None).await.unwrap();
    }

    async fn fixture() -> Fixture {
        let tmp = tempfile::tempdir().unwrap();
        let (origin, repo, wts) = (tmp.path().join("origin.git"), tmp.path().join("repo"), tmp.path().join("wts"));
        std::fs::create_dir_all(&repo).unwrap();
        git(tmp.path(), &["init", "--quiet", "--bare", origin.to_str().unwrap()], None).await.unwrap();
        git(&repo, &["init", "--quiet", "-b", "main"], None).await.unwrap();
        std::fs::write(repo.join(".gitignore"), "ignored/\n").unwrap();
        commit_all(&repo, "init").await;
        git(&repo, &["remote", "add", "origin", origin.to_str().unwrap()], None).await.unwrap();
        git(&repo, &["push", "--quiet", "origin", "main"], None).await.unwrap();
        git(&repo, &["remote", "set-head", "origin", "main"], None).await.unwrap();
        Fixture { _tmp: tmp, repo, wts, active: Default::default() }
    }

    impl Fixture {
        async fn worktree(&self) -> (Uuid, PathBuf) {
            let root = Uuid::new_v4();
            let wt = prepare_worktree(&self.repo, "r", root, &self.wts, None, None).await.unwrap();
            (root, wt.path)
        }

        async fn sweep(&self, merged: bool) -> WorktreeCleanup {
            sweep(&self.wts, &self.active, move |_, _, _| async move { merged }).await
        }
    }

    #[tokio::test]
    async fn a_worktree_with_nothing_of_its_own_is_removed_but_its_branch_stays() {
        let f = fixture().await;
        let (root, path) = f.worktree().await;
        std::fs::create_dir_all(path.join("ignored")).unwrap();
        std::fs::write(path.join("ignored/big"), vec![1u8; 64 * 1024]).unwrap();

        let r = f.sweep(false).await;
        assert_eq!((r.removed, r.failed), (1, 0), "{r:?}");
        assert!(r.freed_bytes >= 64 * 1024, "{r:?}");
        assert!(!path.exists());
        assert!(!f.wts.join(TRASH).join(root.to_string()).exists());
        git(&f.repo, &["rev-parse", "--verify", &format!("refs/heads/{}", task_branch(root))], None).await.unwrap();
    }

    #[tokio::test]
    async fn a_merged_pull_request_frees_the_worktree_and_a_follow_up_gets_its_commits_back() {
        let f = fixture().await;
        let (root, path) = f.worktree().await;
        std::fs::write(path.join("work.txt"), "done").unwrap();
        commit_all(&path, "work").await;

        assert_eq!(f.sweep(false).await.kept_unmerged, 1, "not merged: kept");
        assert!(path.exists());
        assert_eq!(f.sweep(true).await.removed, 1);
        assert!(!path.exists());

        let again = prepare_worktree(&f.repo, "r", root, &f.wts, None, None).await.unwrap();
        assert_eq!(again.path, path);
        assert_eq!(std::fs::read_to_string(path.join("work.txt")).unwrap(), "done");
    }

    #[tokio::test]
    async fn uncommitted_or_untracked_work_is_kept_even_when_merged() {
        let f = fixture().await;
        let (_, path) = f.worktree().await;
        std::fs::write(path.join("scratch.txt"), "notes").unwrap();
        let r = f.sweep(true).await;
        assert_eq!((r.removed, r.kept_changes), (0, 1), "{r:?}");
        assert!(path.join("scratch.txt").exists());
    }

    #[tokio::test]
    async fn a_thread_with_a_run_in_flight_is_left_alone_even_on_a_follow_up() {
        let f = fixture().await;
        let (root, path) = f.worktree().await;
        f.active.lock().await.insert(Uuid::new_v4(), ("r".into(), "worktree".into(), root));
        let r = f.sweep(true).await;
        assert_eq!((r.removed, r.kept_in_use), (0, 1), "{r:?}");
        assert!(path.exists());
    }

    #[tokio::test]
    async fn another_branch_a_locked_worktree_and_foreign_folders_are_kept() {
        let f = fixture().await;
        let (_, moved) = f.worktree().await;
        git(&moved, &["switch", "--quiet", "-c", "someones-branch"], None).await.unwrap();
        let (_, locked) = f.worktree().await;
        git(&f.repo, &["worktree", "lock", locked.to_str().unwrap()], None).await.unwrap();
        let foreign = f.wts.join("r").join("not-a-run-id");
        std::fs::create_dir_all(&foreign).unwrap();

        let r = f.sweep(true).await;
        assert_eq!((r.removed, r.kept_unmerged, r.kept_in_use), (0, 1, 1), "{r:?}");
        assert!(moved.exists() && locked.exists() && foreign.exists());
    }
}
