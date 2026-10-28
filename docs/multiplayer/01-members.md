# 01 · Members, roles and invites

Phase 1.

## Today

- `org_members.role` is `owner` or `member`.
- Owners add people with `POST /v1/orgs/:id/members {login}`, and only people who have already
  signed in once (`AddMember::NotFound` otherwise). Most people you want to invite have never
  heard of Sidefleet, so the flow fails at the first step.
- There is no way to remove a member, change a role or leave an org.

## 1. Roles

| Role | Who it's for |
|---|---|
| `owner` | Billing, deleting the org, everything an admin can do. At least one per org. |
| `admin` | Manages members, invites and org settings, and can stop runs or remove machines of members who left. |
| `member` | Starts tasks on their own machines and drives their threads. The default for invites. |
| `guest` | Sees only the threads explicitly shared with them. Phase 3. |

What each role may do (machines run only their owner's tasks whatever the role, see [02](02-machines.md)):

| Action | owner | admin | member | guest |
|---|---|---|---|---|
| See team threads, diffs, PRs | ✓ | ✓ | ✓ | shared threads only |
| Start tasks (on their own machines) | ✓ | ✓ | ✓ | ✗ |
| Connect their own machines | ✓ | ✓ | ✓ | ✗ |
| Stop any run, remove offline machines | ✓ | ✓ | ✗ | ✗ |
| Invite, remove, change roles below their own | ✓ | ✓ | ✗ | ✗ |
| Org worker tokens | ✓ | ✓ | ✗ | ✗ |
| Plan, delete org, make owners | ✓ | ✗ | ✗ | ✗ |

`Caller::is_owner()` checks that are about management become `can_manage()` (owner or admin).

Admins can never start anything on **another member's machine**. Removing a member is how an
admin cuts off their machines (§4).

## 2. Invite links

An owner or admin opens **Team → Invite** and gets a link like
`https://app.sidefleet.xyz/join/K7QD-M2XP-9RTA`, with:
- **Role**: member (default) or admin.
- **Expiry**: 7 days by default, or 1 day or 30 days.
- **Uses**: one person, or up to N people (a link for the whole team chat).
- Optionally, **restricted to an email or a GitHub login**.

Opening the link:
1. Not signed in: the page shows "Mark invited you to **Acme** on Sidefleet" and a button,
   **Continue with GitHub**. After GitHub sign-in it comes back to the link. First-time users get
   their personal org as today, plus the membership.
2. Signed in: **Join Acme** → membership is created, the org switcher selects Acme, and the
   [first-run screen](#5-first-run-in-a-team) appears.
3. Expired, revoked or used up: "This invite has expired. Ask Mark for a new one."

Also from the invite panel:
- **Pending and recent invites**, with who made each, uses left, and a Revoke button.
- **Add by GitHub login** stays for people who have signed in before. It becomes "invite
  @login", which adds them at once and puts a "You were added to Acme" item in their inbox.

### Data

```sql
CREATE TABLE org_invites (
    id          uuid PRIMARY KEY,
    org_id      uuid NOT NULL REFERENCES orgs(id) ON DELETE CASCADE,
    code_hash   text NOT NULL UNIQUE,        -- SHA-256 of the code; the code is shown once
    role        text NOT NULL CHECK (role IN ('admin', 'member', 'guest')),
    created_by  uuid REFERENCES users(id) ON DELETE SET NULL,
    restrict_to text,                         -- email or GitHub login, or NULL
    max_uses    integer,                      -- NULL = unlimited
    uses        integer NOT NULL DEFAULT 0,
    expires_at  timestamptz NOT NULL,
    revoked_at  timestamptz,
    created_at  timestamptz NOT NULL DEFAULT now()
);
ALTER TABLE org_members DROP CONSTRAINT org_members_role_check;
ALTER TABLE org_members ADD CONSTRAINT org_members_role_check CHECK (role IN ('owner', 'admin', 'member', 'guest'));
ALTER TABLE org_members ADD COLUMN invited_by uuid REFERENCES users(id) ON DELETE SET NULL;
```

The code follows the enrollment-code style (`K7QD-M2XP-9RTA`, 12 characters from the same
unambiguous alphabet), and only its SHA-256 is stored, like worker and API tokens.

### API

| Method | Path | Who | |
|---|---|---|---|
| `POST` | `/v1/orgs/:org/invites` | admin+ | `{role, expires_in_days, max_uses, restrict_to}` → `{invite, url}` (the URL is returned once) |
| `GET` | `/v1/orgs/:org/invites` | admin+ | pending invites |
| `DELETE` | `/v1/orgs/:org/invites/:id` | admin+ | revoke |
| `GET` | `/v1/invites/:code` | anyone | `{org_name, inviter_login, role, expired}`, for the landing page. Rate-limited per IP. |
| `POST` | `/v1/invites/:code/accept` | signed-in user | join. Idempotent if already a member. |

`accept` runs in one transaction: lock the invite row, check expiry, revocation, uses and
`restrict_to`, then insert the membership, increment `uses`, add an `inbox_cleared` row for the new
member (as migration 0017 did, so their inbox starts empty), and write an audit event.

## 3. Domain auto-join (phase 2)

`orgs.join_domain text`: anyone signing in with a verified primary GitHub email at that domain
sees "Join Acme" on their first screen. Admin-only setting. Shown only when GitHub reports the
email as verified, which is what `users.email` stores today.

## 4. Removing, leaving, changing roles

- `PATCH /v1/orgs/:org/members/:user {role}` (admin+, never to or from `owner` unless the caller
  is an owner, and never removing the last owner).
- `DELETE /v1/orgs/:org/members/:user` removes someone (admin+), and `DELETE …/members/me` leaves.

Removing a member, in one transaction:
1. Their machine assignments in this org are disabled. Their personal connections and assignments
   in other orgs stay active. Their `workers` rows stay for history.
2. Their **queued** runs in this org: stopped, with reason `member removed`.
3. Their **running** runs in this org: stop requested.
4. Their API tokens for this org: revoked.
5. Their threads stay visible to the team, and anyone can carry one on with **Continue on my
   machine** ([03](03-shared-threads.md) §5). Their private threads stay private. Owners see that
   they exist (03 §2) and can delete them, but can't read them.
6. Audit event `member.removed` with the counts above.

The page confirms with these numbers before it acts: "Remove Sam? Their 2 machine assignments are disabled and
1 running task stops."

## 5. First run in a team

After joining, a short checklist replaces the empty home page (built on the existing first-run
checklist):
1. **See what the team is working on**: links to the fleet board, or the thread list until
   [05](05-fleet-board.md) ships.
2. **Connect a machine**: the usual connect flow, landing in this org. It explains that teammates
   will see your tasks but can never run anything on it ([02](02-machines.md)).
3. **Turn on notifications** for mentions and handoffs.

## Tests

- An invite works for a first-time GitHub user end to end (oauth test support).
- Expired, revoked and used-up invites are refused. `restrict_to` mismatches are refused.
- Two concurrent accepts of a `max_uses = 1` link: exactly one wins.
- A member cannot create invites. An admin cannot create owners or remove the last owner.
- Removing a member revokes their worker and API tokens and stops their queued runs.
