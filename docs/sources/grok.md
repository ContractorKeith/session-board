# Grok source

Store: `${GROK_HOME:-~/.grok}/sessions/session_search.sqlite`. Empty overrides
fall back to HOME. The shared `source::database_paths` resolver supplies this
path to the registry, doctor and watcher; the watcher watches its parent directory
and reattaches after it appears or is recreated.

Read `session_docs` columns `session_id`, `cwd`, `updated_at`, `title`, `content`;
ignore `content_hash` and the FTS5 mirror. IDs must match `[A-Za-z0-9._:-]+` and
must not start with `-`; invalid IDs skip/count sessions, without sanitizing them
into a different native identity. IDs become `grok:<session_id>`. Empty, relative
or NUL-bearing cwd skips/counts sessions; shared normalization handles absolute
paths. Titles use the shared printable-line sanitizer.

`updated_at` is Unix seconds, or milliseconds when **at least** 10^12. It supplies
both start and activity time because the index has no creation time. Timestamps
more than one hour in the future are clamped to scan time with a path/id warning;
negative or invalid values skip/count rows. The SQL window predicate compares
seconds and milliseconds separately before reading content. Doctor scans all ages.

SQL selects `substr(content, -8192)` and a bounded title candidate from the first
non-blank content line. The full content never enters Rust. Prefer the cleaned
`title`, then the first non-blank content line (60 characters), then the native ID.
`last_assistant_text` is the UTF-8 bounded latest 8 KiB of the flat content; it can
contain tool names and is not guaranteed assistant prose. `last_user_text` is
empty: the search index has no reliable per-turn roles, so duplicating it would
imply knowledge we do not have.

Status is recency only: **done** below 3600 seconds, otherwise **idle**, including
exactly 3600 seconds. Cannot derive working/waiting (or explicit errors) from this
flat search index, even if content ends in a question. Resume is
`grok --resume=<session_id>`; the installed Grok parser accepts `=` (verified with
`grok --resume= --help`, which exits without resuming). Transcript path is the DB.

SQLite opens READ_ONLY / NO_MUTEX with a 200 ms busy timeout. Existing `-wal` or
`-shm` selects plain `mode=ro`, reading committed WAL frames with SQLite's normal
locking; a rollback `-journal` also selects normal locking so spilled uncommitted
pages cannot leak through. With no sidecars, `mode=ro&immutable=1` avoids creating
WAL/SHM files on a stopped store. No `nolock`, copies or immutable retry after a
busy/locked read. Normal scans print no SQLite warning. Missing stores increment
skipped stores silently. Busy/locked/corrupt/schema failures warn with the path,
count one skipped store and discard partial sessions. Row conversion failures
skip/count the row; SQLite step/I/O errors fail the store. No new sidecars or
journal files are created on either normal path.

Fixtures are committed **SQL**, generated in temp directories by
`tests/sqlite_sources.rs`, clock 2026-11-01 12:00 UTC:

| Row in `fixtures/grok/session_search.sql` | Rule |
|---|---|
| recent | Millisecond timestamp, normalized cwd, title, recent done despite question text |
| old | Seconds timestamp, first-line fallback title, 61-minute idle |
| relative | Relative cwd skipped and counted |

`tests/sqlite_round2.rs` adds committed uncheckpointed WAL rows, a 3,000-row
spilled uncommitted transaction, a corrupt middle leaf page (both sources),
conversion failures, ID/NUL-cwd validation, blank/id fallback titles, future clamp,
the exact millisecond boundary, and an old BLOB excluded before conversion.
It polls directory contents throughout a 5,002-session scan to catch transient
sidecars. `source::sqlite::tests` checks database fd access flags on Linux and a
failed SQL write on both platforms. Read-write opener and sidecar-producing
mutations are caught by these oracles. `tests/watch_sources.rs` drives
`Scans::start` in isolated subprocesses for Grok/OpenCode overrides, update requests,
missing-root warnings and reattachment. CLI tests isolate HOME/config/state and
all agent-store overrides.

The ignored `issue67_large_store_memory_probe` measures a 100,000-row history
(8 KiB content each, outside the seven-day window) plus the three base rows. On
Omarchy the full fixture-generation/scan process peaked at **12,220 KiB RSS** and
took 1.480 seconds. Retained preview memory still scales with in-window sessions;
this measurement does not claim a constant memory bound for 100,000 active rows.
The real Omarchy store exists but is empty; macOS skeptic sampling supplied the
two-row live evidence for this round.
