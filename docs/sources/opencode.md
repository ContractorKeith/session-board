# OpenCode source

**Fixture-only, unverified against a live install.** Neither Keith machine has
OpenCode installed. Store: `~/.local/share/opencode/opencode.db`; legacy JSON
storage is unsupported. This version uses the default HOME path, without an XDG
data-home override.

Schema references retrieved for M6:

- [agenthud schema](https://github.com/neochoon/agenthud/blob/d8c4ff13260dfaae1d2b8c5492b1d21aa4534b62/docs/schemas/opencode-session.md),
  an empirical OpenCode 1.17.3 schema.
- [agent-trail SQLite reader](https://github.com/camtrik/agent-trail/blob/6e53f21f292955974ac21a082edb4ac216e2ad92/ingest/parser/opencode.ts),
  confirming the session/message/part row fields and JSON parsing.
- [OpenCode CLI](https://opencode.ai/docs/cli/#tui) documents `--session` / `-s`:
  resume hint is `opencode --session <id>`.

Selected tables/columns (all timestamps are epoch milliseconds):

| Table | Columns used |
|---|---|
| `session` | `id`, `parent_id`, `directory`, `title`, `time_created`, `time_updated` |
| `message` | `id`, `session_id`, `time_created`, `data` |
| `part` | `id`, `message_id`, `session_id`, `time_created`, `data` |

Only `parent_id IS NULL` owner sessions are read; children are excluded. IDs are
`opencode:<id>`, cleaned to printable lines. Empty cleaned IDs and relative/empty
`directory` skip/count sessions. Normalize cwd. Start/activity come from
`session.time_created` / `time_updated`, divided by 1000; negative times skip/count.
Title uses cleaned `session.title`, falling back to the first user message's text
(60 characters). Each role's latest message supplies a UTF-8 bounded 8 KiB preview.
Only `part.data.type = text` contributes prose; ignore reasoning and step parts.
Transcript path is the database file. Reads use one transaction snapshot.

Status maps into the same `status::derive` rules as Codex:

- `message.data.role = user` starts pending generation. An assistant missing both
  `data.time.completed` and a terminal `data.finish` is pending too. `tool-calls`
  is a continuation finish, not a completed turn.
- `part.data.type = tool` with `state.status = pending|running` is pending even
  if the message has a completion marker. Under 1800 seconds: working; at/after
  1800: error with a doctor reason.
- Assistant completion (`time.completed`, or a nonempty terminal `finish`) maps
  to ended. Its current text ending in `?`, or containing `<question>` or
  `<request_user_input>`, gives waiting. Otherwise done below 3600 seconds, idle
  at/after 3600. Question detection is a heuristic, not a permission-table query.
- Non-null assistant `data.error`, or a tool `state.status = error`, supplies
  explicit error evidence unless another tool is still pending. Evidence is
  recomputed per message: later success clears earlier errors and questions.

Messages/parts order by `time_created`, then `id`. Unknown roles/part types are
ignored. Malformed JSON or data types increment skipped lines (SQLite records,
not literal lines); malformed session rows increment skipped sessions. Query or
schema mismatch warns and skips the affected store/session without crashing.
The `permission`, `project`, migrations, model, tokens and agent-owned archive
fields are unused. sboard owns its own archive.

SQLite uses the shared no-lock read-only helper described in
[grok.md](grok.md). WAL errors fall back to an immutable checkpointed snapshot
with a warning; **uncheckpointed changes are not visible**. No agent-store writes,
locks, copies or new sidecars. Schema drift fails soft; live verification is still
required before claiming compatibility with an installed OpenCode version.

Fixtures are committed **SQL**: `fixtures/opencode/opencode.sql`, generated into
a temporary database by `tests/sqlite_sources.rs`, clock 2026-11-01 12:00 UTC.

| Row | Rule |
|---|---|
| working | Open assistant with running tool, one minute old |
| stuck | Same shape at the 30-minute boundary: error and doctor reason |
| waiting | Completed assistant question |
| done | Recent completed answer; first-user fallback title |
| idle | Completed answer exactly 60 minutes old |
| error | Explicit assistant API error |
| relative | Relative directory skipped and counted |
| child | Non-null parent_id excluded |

The helper adds a user message/text part and assistant message/text part for each
row. Tests also cover store failures, isolated CLI filters/config toggles and
sidecar absence; fixture extensions exercise message completion, tool recovery,
malformed JSON, preview bounds and stale-question handling.
