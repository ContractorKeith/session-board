# Grok source

Store: `~/.grok/sessions/session_search.sqlite`. Read `session_docs` columns
`session_id`, `cwd`, `updated_at`, `title`, `content`; ignore `content_hash` and
the FTS5 mirror. IDs are `grok:<session_id>`. Clean IDs and titles with the shared
printable-line sanitizer; an empty cleaned ID skips/counts the session. Normalize
absolute cwd paths; empty/relative cwd skips/counts the session.

`updated_at` is Unix seconds, or milliseconds when greater than 10^12. It supplies
both start and activity time because the index has no creation time. Negative or
invalid timestamps skip/count the row. Prefer the cleaned title, falling back to
the first content line (60 characters). Both text previews contain the same latest
8 KiB UTF-8 tail of `content`: the index has no reliable per-turn roles.

Status is recency only: **done** below 3600 seconds, otherwise **idle**, including
exactly 3600 seconds. Cannot derive working/waiting (or explicit errors) from this
flat search index, even if content ends in a question. Respect the configured scan
window; doctor scans all ages. Resume is `grok --resume <session_id>`, verified with
`grok --help` on Omarchy. Transcript path is the database file.

SQLite access uses READ_ONLY / NO_MUTEX with `mode=ro&nolock=1`. `nolock` avoids
locks on the agent database and refuses WAL access rather than creating sidecars.
A lock/WAL open or schema-probe error retries with `immutable=1`. That fallback
reads only the checkpointed database: uncheckpointed WAL changes are invisible,
and doctor/list warn about this limitation. No copies, journal files, WAL files,
shared-memory files, or writes to the agent store. A missing store increments
skipped stores without warning; special files, corrupt databases and schema errors
increment skipped stores with a warning. Invalid rows increment skipped sessions.

Fixtures are committed **SQL**, not binary databases: `fixtures/grok/session_search.sql`.
`tests/sqlite_sources.rs` builds fresh databases in temporary directories with a
fixed clock of 2026-11-01 12:00 UTC:

| Row | Rule |
|---|---|
| recent | Millisecond timestamp, normalized cwd, title, recent done despite question text |
| old | Seconds timestamp, first-line fallback title, 61-minute idle |
| relative | Relative cwd skipped and counted |

Temporary regression cases cover UTF-8 8 KiB tails, zero-day scan windows, URI
metacharacters in paths, missing/directory/FIFO/corrupt/schema-mismatch stores,
WAL-mode immutable fallback without sidecars, doctor counts, agent selection and
disabled sources. CLI tests isolate HOME, config and state. The real Omarchy store
exists but is empty; live status evidence remains unavailable.
