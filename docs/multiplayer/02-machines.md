# 02 · Machines: yours stay yours

Phase 1. This is the section that makes it safe to invite anyone.

## Today

- A machine belongs to an org (`workers (org_id, id)`), and whatever token or enrollment approval
  connected it decides which org.
- `CAN_RUN` and `claim_run` (crates/server/src/db.rs) let **any** run of the org go to **any**
  online machine of the org that has the repo and agent. Any member can pin a run to any machine
  and choose full access (`bypassPermissions`).
- An agent runs as the OS user that runs `sidefleet run`. A git worktree separates changes, but
  it isn't a sandbox. With full access an agent can read `~/.ssh`, shell history, environment variables and the
  agent's own sign-in.

So today, connecting your laptop to a team org gives every member a shell on it.

## 1. The rule

> **A machine only runs its owner's tasks.** Teammates see that your machine exists, whether it's
> online and how busy it is, and they see the threads that run on it. They can never start,
> continue or approve anything on it.

There's no sharing setting in this version. Everything that "uses" a teammate's work (taking
over a thread, a handoff, a suggestion they'd like run) goes through a **fork onto your own
machine**, which Sidefleet already does well: the fork carries the files as they were and a
transcript of the conversation. Sharing machines is deferred (§7).

Why:
- **No pooled agent sign-ins.** Every task runs on its own creator's Claude Code / Codex login,
  so nobody's subscription serves other people.
- **No one runs code as you.** There's no consent screen to get wrong, and no cap to configure.
- **Clear GitHub identity.** PRs and pushes from a thread come from its owner's machine and
  are made as its owner, which is also the person driving the thread.

## 2. Owners

```sql
ALTER TABLE workers       ADD COLUMN owner_id uuid REFERENCES users(id) ON DELETE SET NULL;
ALTER TABLE worker_tokens ADD COLUMN owner_id uuid REFERENCES users(id) ON DELETE SET NULL;
```

How a machine gets its owner:
- **Approve-on-the-page enrollment**: the approver (`enrollments.decided_by`). The minted
  per-machine token gets `owner_id = approver`.
- **`connect` with a token**: the token's owner. The "Connect a machine" panel lets every member
  mint their own machine token (`owner_id = them`). Today only org owners can mint org worker
  tokens. That stays true for org tokens, and personal machine tokens are new.
- **Static tokens and the developer key** (self-hosted): no owner (`NULL`). These machines run
  only owner-less runs, the ones started with the developer key, so a self-hosted install behaves
  exactly as today.
- At `register`, `workers.owner_id` is copied from the token. A heartbeat never changes it. A
  worker can't choose its owner.

### Migrating existing machines

- **Personal orgs**: the org's only member.
- **Team orgs**: the enrollment approver if an enrollment minted the machine's token. Otherwise,
  if the org has exactly one owner, that owner, because org worker tokens can only be minted by
  owners today. Otherwise the machine is **unclaimed**.
- **Unclaimed** machines run nothing. The Machines page shows them with **This is mine**. The
  first member to press it becomes the owner, with an audit event (08) and a notice to the org's
  owners, who can reassign it. That's rare: only team orgs with several owners and token-connected
  machines.
- **Queued runs** whose creator isn't known (`created_by IS NULL` in a team org, see 03 §1) are
  claimed by no one, and they're failed by the migration with the reason "Sidefleet now runs tasks
  only on their creator's machines; start it again." Only a handful are expected, and the release
  notes say so.

## 3. Enforcement

Checks happen in two places, and the second is the one that guarantees the rule.

**At creation** (`create_run`, `follow_up`, `fork_run`, `restore_run`, `open_pr`):
- **Pinned to someone else's machine**: 403 "MacBook-Pro is Mark's. Tasks only run on their
  owner's machines."
- **Unpinned, and none of the caller's own machines has the repo and agent**: 409 with what's
  missing ("None of your machines has `acme/shop` with Codex"), plus a **Connect a machine**
  link. This follows how the page already explains a missing repo or agent.
- **A reply, restore or PR on a thread whose machine isn't the caller's**: 403 with a fork hint
  (03 §5 turns it into a suggestion).

**At claim**: `CAN_RUN` and the `claim_run` query gain one condition, with worker `w` and run `r`:

```sql
AND w.owner_id IS NOT DISTINCT FROM r.created_by
```

- `r.created_by` is the person **the run acts for**. For a reply, that's who sent it, so a reply
  from anyone but the machine's owner can never be claimed.
- A failover (`moved_from`) only moves to the creator's other machines, so the excluded-worker
  logic applies within them.
- Owner-less runs (developer key) land only on owner-less machines (static tokens). That's all of
  a self-hosted install.

Least-busy routing, usage-limit failover and failover when a machine drops all keep working, among
your own machines.

**Stopping** a run on your machine: you, or an org admin (a stop is always safe). Admins can't
start anything on your machine.

## 4. The machine page

For your own machines: as today, plus a **Yours** badge.

For teammates' machines, read-only:
- The owner's avatar, online or offline, CPU and how busy it is, the agents it has, and its repo
  *count* (no paths).
- **Running here now**: the threads running on it, which link to them if you can see them.
- No Remove button, no Update button, no pinning.

The composer's machine picker lists **only your machines**. Grouping by owner on the Machines page:
**Yours** first, then each teammate.

### API

`GET /v1/workers` gains `owner: {id, login} | null` and `mine: bool`. For machines that aren't
yours, the `repos` field is reduced to a count. `DELETE /v1/workers/:id` and `POST
/v1/workers/:id/update` are limited to the owner and org admins (for cleaning up a departed member's
machines).

## 5. A member leaves or is removed

See 01 §4: tokens they own are revoked, their machines go offline, and their queued and running
runs are stopped. Their threads stay visible to the team, and anyone can **fork** one to carry on
from where it was, on their own machine.

## 6. Moving a machine between orgs

People will connect a machine to their personal org, then join a team. They should be able to
move it without reconnecting.

**Move to Acme…**, owner only, and only when nothing is running on the machine:
1. Reassign the machine's worker token to the target org (`worker_tokens.org_id`). The owner must
   be a member of the target.
2. Create a new `workers` row in the target org at the next `register` or heartbeat. The old row
   stays, offline, in the old org, because past runs reference `(org_id, worker_id)`.

Phase 2. Until then the docs say: "To use a machine in a team, connect it from the team."

## 7. Later: sharing machines (deferred)

We may want sharing later, for a team-owned Mac mini or VPS, or to lend a beefy machine. The
design we set aside, kept here so it isn't lost:
- **Sharing modes** per machine: `private` / `ask` (each use approved by the owner) / named people /
  team, plus a **permission cap** for others (read-only, ask, read & edit, full access).
- **Team machines**: owner-less machines connected with a team token, managed by admins.
- A **consent screen** that says plainly that others' tasks run as your OS user, use your agent
  sign-ins and your GitHub identity.
- **Enforcement**: the claim condition above becomes `owner matches OR has_access(...)`,
  with the cap checked against the run's permission mode.
- **Real isolation** before it's offered for personal machines: others' tasks run as a separate
  unprivileged OS user or in a container.

What has to be true before reviving it:
1. A good answer for **agent sign-ins** (per-user credentials for one run, or team or API accounts),
   so personal subscriptions aren't shared.
2. **PRs as the driver** (the GitHub App, 04 §6), not as the machine owner.
3. Isolation, at least for personal machines.

The data model in this version (`owner_id` on workers and tokens, `created_by` on runs) is all
sharing needs to build on, so nothing here has to be undone.

## Tests

- Claim: a member's run never lands on another member's machine, pinned or not, including
  replies, forks, restores, PR runs and failovers.
- A reply from a non-owner into a thread is refused at creation, and is never claimable even if
  inserted directly.
- Owner-less runs land only on owner-less machines. A self-hosted install (dev key and static
  tokens) works unchanged.
- A heartbeat can't change `owner_id`.
- Migration: personal-org machines get their member. Team-org machines get the enrollment
  approver or the only owner, and are otherwise unclaimed and run nothing until claimed.
- `GET /v1/workers` doesn't leak another member's repo paths.
