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
