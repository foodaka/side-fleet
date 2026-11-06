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

## Before and after shots for UI changes

A pull request that changes what someone sees in `web/` shows a before and an after screenshot in
its description. Skip it for changes with nothing visible, like a refactor.

- **Before:** run `origin/main` from its own worktree (`git worktree add`), never by switching or
  stashing. For a bug fix, capture the bug before writing the fix.
- **Same conditions:** same page, viewport, theme, data and scroll position in both, cropped to
  what changed. Add a phone-width pair (375x812) if the layout is affected.
- **Look at both images** before posting. A shot of a loading state or an error page isn't evidence.
- **Say what the data is.** If the API was mocked, say so in the description. Mocked or seeded data
  doesn't test the server, so name anything that only ran that way.
- **No private data:** this repo is public, so use made-up tasks, machines and logins. No real
  tokens, emails or customer data, and block avatar requests for made-up logins.
- **Attach them with `gh`,** so nothing lands in the repo or on a public file host. Reference the
  files by relative path in the body and `gh` replaces each path with its uploaded URL:

  ```sh
  # body.md has: | ![Before](./before.png) | ![After](./after.png) |
  gh pr edit <number> --body-file body.md --attach './before.png#Before' --attach './after.png#After'
  ```

  `gh pr create` takes `--attach` too. Run it from inside the repo, or pass `-R foodaka/side-fleet`.
  Afterwards, check the description shows the images.

If you can't capture one of the two, say why in the description instead of leaving it out.
