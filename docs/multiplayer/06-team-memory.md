# 06 · Team memory

Phase 3. Every thread the team runs should make the next one better, without anyone writing
documentation. Inspired by Buzz's "ask the project a question and get an answer with receipts".

## 1. Search

**Cmd+K** opens search across the org's threads, and the sidebar has a search field.

**Indexed:** the task text, replies and notes; the agent's assistant text (not tool output, which is
noisy and may contain secrets); PR titles and review bodies (04); changed file paths (05 §2); repo, agent,
people and dates.

**Results:** grouped by thread. Each shows the matching passage with the hit highlighted, the
creator's and driver's avatars, the repo, and the state (merged with the PR number, open, failed).
Tapping one opens the thread scrolled to the passage.

**Filters:** `repo:acme/shop`, `by:sam`, `is:merged`, `file:src/cart.rs`, `before:` and `after:`.

**Visibility:** results go through the same rules as everything else (03 §2). The query joins
on visibility, so nothing private leaks through counts or snippets.

### Index

```sql
CREATE TABLE search_docs (
    id        uuid PRIMARY KEY,
    org_id    uuid NOT NULL,
    root_id   uuid NOT NULL REFERENCES runs(id) ON DELETE CASCADE,
    source    text NOT NULL,      -- task | reply | note | assistant | review | pr_title
    source_id text NOT NULL,      -- run id + seq, note id, review id…
    author_id uuid,
    body      text NOT NULL,      -- redacted text, capped at 8 KB per doc
    at        timestamptz NOT NULL,
    tsv       tsvector GENERATED ALWAYS AS (to_tsvector('english', body)) STORED,
    UNIQUE (source, source_id)
);
CREATE INDEX search_docs_tsv_idx ON search_docs USING gin (tsv);
CREATE INDEX search_docs_org_idx ON search_docs (org_id, at DESC);
```

- **Writing documents:** assistant text is written when a run *finishes*, one document per
  assistant message (coalesced from streamed text), so live streaming isn't slowed. Notes, replies and reviews
  are written when they're stored.
- **Backfill:** a one-off job over existing `run_events` of kind `assistant`.
- **Query:** `websearch_to_tsquery`, ranked by `ts_rank_cd` and recency. Postgres full-text
  search is plenty at the expected size (thousands of threads per org). Semantic search
  (pgvector embeddings) is a later option behind the same API, and it means sending text to an
  embedding provider, which the hosted service doesn't do today.
- **Redaction:** the same redaction as the worker and control plane already apply, re-run on the
  way in.

`GET /v1/search?q=&repo=&by=&state=&limit=` → `{results: [{thread, passages: [{source, text, at, author}]}]}`.
API tokens with `read` scope may call it (add it to `scope_allows`).

## 2. Agents that read the team's history

The bigger idea: when Ana's agent hits an error, it can look up how Mark's thread solved the same
one last week.

**A `sidefleet` MCP server for the running agent.** When a worker starts a run, it adds an MCP
server to the agent's configuration (Claude Code: `--mcp-config`; Codex and opencode: their
equivalents, which needs checking per agent). It talks to the control plane with a **run-scoped
token**:
- It's minted at claim time and returned with the claimed run, and only its SHA-256 is stored.
- Its scope: read, this org, and the threads visible to the **driver** of this run (03 §2), so an
  agent can never see what its driver couldn't.
- It expires when the run finishes.

Tools:

| Tool | Does |
|---|---|
| `search_threads(query, repo?, limit?)` | the search above, returning passages and thread links |
| `read_thread(id, around?)` | a thread's summary (summary.rs) plus the passage around a hit |
| `team_notes(repo)` | the repo's pinned team notes (§3) |

What the agent says should carry the receipts: "This is the same `ECONNRESET` Mark fixed in
*Retry the payment webhook* (#398): he added a retry with backoff in `webhook.rs`." The
thread view renders those references as links.

This is opt-in per org at first (`orgs.settings.agent_memory`), because it changes what agents see.

## 3. Lessons to CLAUDE.md

When a driver corrects an agent ("no, we use sqlx here, not diesel"), that's knowledge the
whole team's agents should have.

- After a thread **merges**, a small summarizing pass looks at the thread's human replies for
  corrections and preferences. It runs in the background on the same machine, as a short
  `kind = 'lessons'` run with a fixed prompt and read-only mode.
- It proposes zero to three one-line lessons. They show up in the thread as a card: "Lessons from this
  thread: 1. Use sqlx, not diesel. 2. Money is always integer cents."
  With **Add to CLAUDE.md** / **Edit** / **Dismiss**.
- **Add** queues a `pr` run on a new branch that appends the lessons to the repo's CLAUDE.md
  (or AGENTS.md) under "Team notes". It goes through review like any other PR, so the team approves what
  every agent will read.
- **Pinned notes** (no PR): a repo-level list on the board, which the MCP tool `team_notes` serves.
  This is for things that don't belong in the repo.

## 4. The morning digest

Once a day per member, at 8:00 in their time zone (taken from the browser), by push and in chat apps
if they've connected one:

> **While you were away**: 6 merged, 2 waiting on you, 1 failed.
> Waiting on you: *Fix checkout rounding* (Ana suggested a change), *Bump deps* (approval).
> Merged: …

It's built from the board's data and sent only if something happened. Members can turn it off or change the time.

## Tests

- A private thread's text never appears in search results, counts or snippets for others, or in
  `search_threads` for an agent whose driver can't see it.
- The run-scoped token stops working when the run finishes, and it can't call any route except
  the three tools' reads.
- Redaction: a secret-shaped string in assistant text isn't indexed.
