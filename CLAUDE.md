# Sidefleet

## Always work in a new worktree

Before changing any file, make sure you are in a git worktree of your own, not the main checkout:

- **Already in a worktree?** If Sidefleet started the task with worktree isolation (the default in
  the composer), you are already in one under `~/.sidefleet/worktrees/<repo>/<run id>` on a
  `sidefleet/<id>` branch. Work there; don't create another.
- **Otherwise** create one first and do all the work in it:

  ```sh
  git worktree add -b <short-branch-name> ../side-fleet-<short-branch-name> origin/main
  cd ../side-fleet-<short-branch-name>
  ```

To check: `git rev-parse --git-dir` differs from `git rev-parse --git-common-dir` inside a linked
worktree and is the same in the main checkout.

Other sessions share the stash stack across worktrees, so never use bare `git stash`/`git stash pop`.

## The web app's build output

Never commit `crates/server/web-dist`: change `web/` only, and CI rebuilds and commits web-dist on
main after merge. If you ran `pnpm build` to try a change, `git checkout crates/server/web-dist`
before committing.
