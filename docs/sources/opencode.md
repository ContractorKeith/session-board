# OpenCode source

**Fixture-only, unverified against a live install.** Neither Keith machine has
OpenCode installed. Store resolution is `OPENCODE_DB` first (including a
channel-specific filename), then `$XDG_DATA_HOME/opencode/opencode.db`, then
`~/.local/share/opencode/opencode.db`. Empty overrides are unset. The shared
`source::database_paths` resolver supplies the registry, doctor and the watcher's
parent directory, with missing-root warnings and reattachment. Relative store
paths resolve against the current directory in that shared resolver. Legacy JSON storage
is unsupported.

Schema references retrieved for M6:

- [agenthud schema](https://github.com/neochoon/agenthud/blob/d8c4ff13260dfaae1d2b8c5492b1d21aa4534b62/docs/schemas/opencode-session.md),
  an empirical OpenCode 1.17.3 schema.
- [agent-trail SQLite reader](https://github.com/camtrik/agent-trail/blob/6e53f21f292955974ac21a082edb4ac216e2ad92/ingest/parser/opencode.ts),
  confirming the session/message/part row fields and JSON parsing.
- [OpenCode CLI](https://opencode.ai/docs/cli/#tui) documents `--session` / `-s`:
  resume hint is `opencode --session=<id>`. OpenCode's string flag parser accepts
  attached `=` values; this was checked against the upstream CLI command and flag
  definitions, not a local install:
  [session string flag](https://github.com/anomalyco/opencode/blob/dev/packages/cli/src/commands/commands.ts)
  and [attached-value convention](https://github.com/anomalyco/opencode/blob/dev/packages/cli/src/framework/flag.ts).

Selected tables/columns (all timestamps are epoch milliseconds):

| Table | Columns used |
|---|---|
| `session` | `id`, `parent_id`, `directory`, `title`, `time_created`, `time_updated` |
| `message` | `id`, `session_id`, `time_created`, `data` |
| `part` | `id`, `message_id`, `session_id`, `time_created`, `data` |

Only `parent_id IS NULL` owner sessions are read; children are excluded. IDs are
`opencode:<id>`. Native IDs must match `[A-Za-z0-9._:-]+` without a leading `-`;
invalid IDs and relative/empty/NUL-bearing `directory` skip/count sessions. Normalize cwd. Start/activity come from
`session.time_created` / `time_updated`, divided by 1000; negative times skip/count.
Title uses cleaned `session.title`, falling back to the first user message's text
(60 characters). Each role's latest message supplies a UTF-8 bounded 8 KiB preview.
Only `part.data.type = text` contributes prose; ignore reasoning and step parts.
Transcript path is the database file. Reads use one transaction snapshot.

Status maps into the same `status::derive` rules as Codex:

- `message.data.role = user` starts pending generation. An assistant without a
  terminal completion is pending too.
- A terminal `data.finish` is a string outside `tool-calls`, `unknown`, and the
  empty string. `data.time.completed` counts only when numeric and `finish` is
  absent. A tool-calls/unknown/empty finish overrides a numeric completion time.
- `part.data.type = tool` with `state.status = pending|running` is pending even
  if the message has a completion marker. Under 1800 seconds: working; at/after
  1800: error with a doctor reason.
- A terminal assistant's current text ending in `?`, or containing `<question>`
  or `<request_user_input>`, gives waiting. Otherwise done below 3600 seconds,
  idle at/after 3600. This is a heuristic, not a permission-table query.
- `data.error` counts only when an object. `error.name = MessageAbortedError`
  is an interruption: idle, regardless of completion markers or tool parts.
  Other error objects or a tool `state.status = error` supply explicit error
  evidence unless another tool is still pending. Later success clears earlier
  errors and questions.
- Non-string finish, non-number/non-null completed time, and non-object/non-null
  error markers (including booleans) are malformed records: skip the message and
  increment skipped lines. Missing/null completed or error markers are absent.

Messages/parts order by `time_created`, then `id`. Unknown roles/part types are
ignored. Malformed JSON or data types increment skipped lines (SQLite records,
not literal lines); malformed session rows increment skipped sessions. Step/I/O or
schema failures warn, count a skipped store and discard all partial sessions.
The `permission`, `project`, migrations, model, tokens and agent-owned archive
fields are unused. sboard owns its own archive.

SQLite uses the shared read-only scan wrapper described in [grok.md](grok.md).
Every on-disk store uses normal `mode=ro` locking with a 200 ms busy timeout;
**no immutable or nolock fallback**. A MEMORY/OFF-journal transaction can spill
before an observer snapshot, so the SHARED lock is required to exclude its pages.
Busy/locked/hot-journal failures skip the store and warn rather than recover it.

**SHM creation/updates beside an existing WAL are allowed SQLite protocol.
Creating WAL or journal files is forbidden.** The private VFS enforces no CREATE
on WAL/journal OS opens. A WAL-format database without its WAL can therefore be
found but skipped when SQLite would need to create that file; the agent must
supply it. No immutable fallback is used even for that closed-store case.

Before opening, symlink_metadata checks the database and each WAL/SHM/journal
entry. Existing entries must be regular files: directories, FIFOs, sockets,
devices and final symlinks (including dangling ones) skip/warn before SQLite.
The identities/set are rechecked after open and before the first schema read.
Non-WAL scans also compare device/inode, size, nanosecond mtime/ctime, header
counter/version bytes and sidecar metadata after reading. SQLite locks provide
transaction isolation; these checks supplement it for replacement/raw file edits.
Normal WAL scans see committed frames. Schema drift fails soft; live verification
is still required before claiming compatibility with an installed OpenCode version.

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

`tests/sqlite_round2.rs` varies the `done-assistant` fixture message through:
`tool-calls`/`unknown`/empty finish plus numeric completion (working), absent finish
plus numeric completion (done), stop finish (done), `MessageAbortedError` (idle),
APIError (error), boolean error/completed/finish and string completed markers
(skipped line; preceding user remains pending). The same regressions cover
invalid IDs/NUL cwds, overrides and a corrupt table leaf; `tests/watch_sources.rs`
checks OpenCode directory updates and reattachment through `Scans::start`.

Shared-wrapper regressions cover writer locks, WAL removal before/inside the OS
open, same-size restored-mtime edits, rename replacement and header fingerprints.
Full byte snapshots capture SHM with its explicit exception only beside WAL.
Relative `OPENCODE_DB=opencode.db` is tested through doctor and `Scans::start`.
Round-5 tests reject pre-existing spilled MEMORY transactions and non-regular
database/sidecar entries, and stress 20 concurrent lists during WAL commits.
Relative overrides still follow cwd if the process itself changes it; child
launches do not change the app's cwd.
