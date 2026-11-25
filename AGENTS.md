
<!-- BEGIN:openagenticstack -->
# openagenticstack workflow

Every change goes from a clean worktree to a PR that shows what changed (before and after), proves it works (Bugpatrol), and reads like a person wrote it (unslop). Pick the steps that fit the task:

- Read-only questions, reviews, and investigations: no worktree, no PR. If they lead to edits, start at step 1.
- Docs-only edits: steps 1, 4 and 5. Check links and runnable examples instead of running Bugpatrol.
- A request for a plan or a local experiment doesn't mean "open a PR". Ship only when asked for a PR or a finished change.

## Steps

1. **Isolate.** New worktree under `.worktrees/<task>` on a branch named `<agent>/<task>` (`claude/fix-rsvp-dates`), from the latest `origin/main`. Never build on `main`. Before starting, skim open PRs for overlap (`gh pr list`, `gh pr diff <n> --name-only`). Copy the gitignored files the app needs (`.env*`, iOS config plists) into the new worktree and run a real install there. Don't symlink `node_modules`.
2. **Capture before.** Before changing anything a user can see, take the before shots with `/before-and-after`. For bug fixes, reproduce the bug with `/patrol` and note the issue id.
3. **Build.** Follow the project's existing architecture and its own AGENTS.md or CLAUDE.md. Database changes go in new migration files. Never apply a migration to production without asking.
4. **Prove.** Run the project's checks (listed in its own AGENTS.md or CLAUDE.md), then `/patrol`: retest the issue or explore the changed flow, and on web apps the gate must exit 0. Take the after shots.
5. **Ship.** Write the PR body from `.github/pull_request_template.md`: what changed, the before/after table, each test command and its result, risks. Run `/unslop` over the title, body and commit messages. Open the PR, wait for the Bugpatrol comment, and fix every finding marked Introduced. End by giving the PR URL.

`/review` (gstack) before opening the PR, and `/cso` on anything touching auth, payments, RLS policies or secrets, are worth the time on non-trivial changes.

## Before and after is required when

- Any UI changes: layout, copy, color, a new screen, an empty or error state. Capture desktop and mobile for web.
- An API or CLI's output changes: paste an output pair instead of screenshots.
- Nothing visible changed: write "No visible change" in that section. Don't skip the section.

Screenshots upload to the repo's own `pr-assets` branch by default, so they're as private as the repo. Don't switch to the public 0x0.st adapter for screens that show real user data.

## Writing for people

Run `/unslop` over everything a person will read before you commit, post or send it: commit messages, PR title and body, docs, code comments, and the final reply. Apply it to text you wrote or changed, not text you didn't touch.

## Multi-agent rules

- One worktree and one branch per task and per agent. Don't touch another agent's worktree, branch or uncommitted work.
- Never commit to `main`. Never `--force`. Only `--force-with-lease`, only on your own branch.
- Regenerate lockfiles on conflict, never hand-merge them.
- Confirm a dev-server port answers your own process before trusting it. Several projects here use shifted ports.
- Don't run schema experiments against a shared or production database.
- Don't merge a PR unless asked. Keep the worktree until the PR is merged or closed.
- If a conflict can't be resolved with confidence, stop and report.

## Checks in this repo

<!-- Replace with this repo's commands. Example: -->
<!-- - `npm run build` -->
<!-- - `npm test` -->
<!-- - `npx bugpatrol run --no-models` -->
<!-- END:openagenticstack -->

## This repo

Kept outside the openagenticstack block, so running the installer again leaves it alone.

Where this file and `CLAUDE.md` disagree, `CLAUDE.md` wins. That covers where worktrees go
(`../side-fleet-<name>`, not `.worktrees/`), how screenshots get into a pull request, and keeping
the Mac app in step with the web app.

Checks, the same ones CI runs:

- `pnpm --dir web check`
- `cargo clippy --workspace --all-targets --locked -- -D warnings`
- `cargo test --workspace --locked`. The integration tests need Postgres (`docker compose up -d`).
- `SWIFT_DOCKER=1 macos/scripts/test-live.sh` when `macos/` changed.

Bugpatrol isn't set up here yet. It needs a running control plane with a worker and demo data to
test against, so leave the Bugpatrol lines in the pull request template unchecked.
