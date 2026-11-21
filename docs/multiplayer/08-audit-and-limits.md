# 08 · Audit log, plans and limits

## 1. Audit log (phase 1)

When several people work in one org, "who did that?" must always have an answer.

```sql
CREATE TABLE audit_events (
    id        bigserial PRIMARY KEY,
    org_id    uuid NOT NULL REFERENCES orgs(id) ON DELETE CASCADE,
    actor_id  uuid REFERENCES users(id) ON DELETE SET NULL,  -- NULL: developer key, a worker, the system
    actor     text NOT NULL,                                 -- login, 'developer key', 'worker:<id>', 'automation:<id>'
    action    text NOT NULL,
    target    text,                                          -- run id, worker id, user id, invite id
    detail    jsonb NOT NULL DEFAULT '{}',
    at        timestamptz NOT NULL DEFAULT now()
);
CREATE INDEX audit_events_org_idx ON audit_events (org_id, at DESC);
```

**What's logged** (at least):

| Area | Actions |
|---|---|
| Members | `invite.created`, `invite.revoked`, `member.joined`, `member.role_changed`, `member.removed`, `member.left` |
| Machines | `machine.enrolled` (with owner), `machine.claimed`, `machine.reassigned`, `machine.moved`, `machine.removed` |
| Control | `run.stopped_by_other` (whenever an admin stops someone else's run), `handoff.created`, `.accepted`, `.declined` |
| Tokens | `worker_token.created` and `.revoked`, `api_token.created` and `.revoked` |
| Visibility | `thread.made_private`, `thread.made_team` |

It never logs task text, agent output or tool input beyond the one-line redacted summary the
approval already stores.

**Who sees it**:
- Admins and owners see the whole org at **Team → Activity log**.
- Every machine owner sees their own machine's entries on its card ("History").

`GET /v1/orgs/:org/audit?actor=&action=&target=&before=` (admin+).
`GET /v1/workers/:id/audit` (owner, admin+). Kept for 1 year on hosted, and indefinitely when
self-hosted unless pruned.

### What v1 built

Migration `0030_audit.sql` and `crates/server/src/audit.rs`; the API is in
[API.md](../API.md#audit-log).

- **Schema.** As above, plus `run_id uuid` and `worker_id text` columns (copies, not foreign keys,
  so a run's or a removed machine's history stays) with partial indexes for "this run" and "this
  machine", and indexes on `(org_id, actor_id)` and `(org_id, action)`. `detail` must be a JSON
  object. A trigger rejects every `UPDATE` except the database clearing `actor_id` when a user is
  deleted (`actor` keeps their login). Deletes happen only by deleting the org (or an operator
  pruning).
- **Written where the change happens.** State changes the control plane makes (claims, finishes,
  requeues, failover moves, the reaper, approvals being asked, expiring or cancelled) are written in
  the same transaction. Actions people take are written by the route once they succeed; a failed
  write is logged and never fails the action.
- **Actions.** Tasks: `run.created` (`via` task/reply/fork/restore/pr/integrate), `run.claimed`,
  `run.started`, `run.finished`, `run.stopped`, `run.stopped_by_other`, `run.requeued` (usage
  limit), `run.moved` (failover), `run.permission_mode_changed`. Approvals: `approval.requested`,
  `.approved`, `.denied`, `.timed_out`, `.cancelled`. Machines: `machine.enrolled` (first
  registration in the org, with its owner), `machine.enrollment_approved`, `.enrollment_denied`,
  `machine.removed`. Tokens: `worker_token.created`/`.revoked`, `api_token.created`/`.revoked`.
  People: `org.created`, `org.renamed`, `invite.created`, `invite.revoked`, `member.added`,
  `member.joined`, `member.role_changed`, `member.removed`, `member.left`.
- **Reserved names**, accepted by the filter and written later without a schema change (their
  amounts, currency, payee and decision go in `detail`): `budget.authorized`, `budget.exceeded`,
  `payment.requested`, `payment.made`, `payment.refused`, `payment.refunded`, and the not yet built
  `handoff.*`, `machine.claimed`/`.reassigned`/`.moved`, `thread.made_private`/`.made_team`.
- **Redaction.** `detail`, `target` and `actor` go through `sidefleet_core::redact` (and
  `redact_tool_input`, which clips strings to 500 bytes) before they are stored, and `detail` again
  on the way out. Task text, agent output, tool input, a denial's message, invite codes and token
  values are never put in it; an approval carries its one-line redacted summary.
- **Who reads it.** Org owners (and the developer key, which owns the default org) read the whole
  log. Members read the entries about their own machines and the ones they did; machine owners
  read their machine's full history at `GET /v1/workers/:id/audit`. Another org's entries read as
  `404`, and API tokens can't reach the routes. Enforced in the query, not the page.
- **API.** `GET /v1/orgs/:org/audit` filters by `run`, `thread`, `machine`, `actor`, `action`
  (name or `group.*`), `target`, `since`/`until`, keyset-paged with `before` (`{events, next}`).
  `GET /v1/orgs/:org/audit/export?format=csv|json` downloads the same filtered set (at most
  10,000; CSV cells that could be read as a spreadsheet formula are prefixed with `'`).
- **Web.** Team → Activity log (`#/team/activity`): filters by kind, person or machine, machine
  and dates, older pages, CSV/JSON export. A thread's header has **Activity**, the same log for
  that thread.

### Deferred

- **Mac app**: the Activity log and a thread's Activity aren't in `macos/` yet; they use only the
  routes above.
- **Machine card "History"** on the Machines page (the route exists).
- **Retention**: nothing prunes yet. Hosted should keep a year (or the plan's retention, §3); a
  daily `DELETE FROM audit_events WHERE at < now() - interval '1 year'` is all it takes.
- Spec'd actions whose features don't exist yet: `handoff.*`, `thread.made_private`/`made_team`,
  `machine.claimed`/`reassigned`/`moved` (reserved above). Archive, snooze and pin aren't logged.
- Spend: budgets, payments and wallets (reserved names only), §2's per-member caps and §3's plans
  and billing.

## 2. Per-member limits (phase 2)

Today's plan limits are per org: `free` allows 2 machines and 2 runs at once, and `pro` allows 10 and 10
(tenancy.rs). In a team, one person's race of 3 agents shouldn't take every slot.

- `orgs.settings.member_max_running` (default: none) caps how many runs one member may have running
  at once. It's enforced in `claim_run` next to the org's limit, using `created_by`.
- `orgs.settings.member_max_queued` makes a new run beyond the cap fail fast with 429 "You have 10 tasks
  queued", instead of quietly waiting forever.

## 3. Plans for teams (phase 2, a product decision)

A sketch to decide on before charging:

| | Free | Pro (one person) | Team |
|---|---|---|---|
| Members | 1 (personal) | 1 | per seat |
| Team orgs | join any, create 1 with up to 3 members | same | ✓ |
| Machines | 2 | 10 | 5 per seat, pooled |
| Runs at once | 2 | 10 | 5 per seat, pooled, plus member caps |
| Search, board | ✓ | ✓ | ✓ |
| Agents read team history (06 §2), automations (07) | ✗ | ✓ | ✓ |
| Audit log retention | 7 days | 30 days | 1 year |
| GitHub App, guests | ✗ | ✗ | ✓ |

The free team tier (up to 3 members) is what makes "invite your friend" frictionless, which is
the growth loop multiplayer exists for. A machine counts against the limit of the org it's
connected to.

There's no billing integration yet (`orgs.plan` is set by an operator). This section needs one
before "Team" can be sold.
