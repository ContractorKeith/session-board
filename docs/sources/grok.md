# Grok source

Store: `${GROK_HOME:-~/.grok}/sessions/session_search.sqlite`. Empty overrides
fall back to HOME. The shared `source::database_paths` resolver supplies this
path to the registry, doctor and watcher; relative overrides resolve against the
current directory before use. The watcher watches the parent directory and
reattaches after it appears or is recreated.

Read `session_docs` columns `session_id`, `cwd`, `updated_at`, `title`, `content`;
ignore `content_hash` and the FTS5 mirror. IDs must match `[A-Za-z0-9._:-]+` and
must not start with `-`; invalid IDs skip/count sessions, without sanitizing them
into a different native identity. IDs become `grok:<session_id>`. Empty, relative
or NUL-bearing cwd skips/counts sessions; shared normalization handles absolute
paths. Titles use the shared printable-line sanitizer.

`updated_at` is Unix seconds, or milliseconds when **at least** 10^12. It supplies
both start and activity time because the index has no creation time. Timestamps
more than one hour in the future are clamped to scan time with a path/id warning;
negative or invalid values skip/count rows. NULL and non-integer timestamps
are included for conversion/counting even when the window predicate would exclude
them. The SQL window predicate compares seconds and milliseconds separately
before reading content. Doctor scans all ages.

SQL selects `substr(CAST(content AS BLOB), -8192)` and a bounded title candidate
from the first non-blank content line. The full content never enters Rust. Prefer the cleaned
`title`, then the first non-blank content line (60 characters), then the native ID.
`last_assistant_text` is the latest 8 KiB of the flat content, decoded lossily
and bounded again on a UTF-8 boundary; embedded NUL bytes are retained. It can
contain tool names and is not guaranteed assistant prose. `last_user_text` is
empty: the search index has no reliable per-turn roles, so duplicating it would
imply knowledge we do not have.

Status is recency only: **done** below 3600 seconds, otherwise **idle**, including
exactly 3600 seconds. Cannot derive working/waiting (or explicit errors) from this
flat search index, even if content ends in a question. Resume is
`grok --resume=<session_id>`; the installed Grok parser accepts `=` (verified with
`grok --resume= --help`, which exits without resuming). Transcript path is the DB.

SQLite opens READ_ONLY / NO_MUTEX with a 200 ms busy timeout. An existing `-wal`
selects plain `mode=ro`, reading committed frames with SQLite's normal locking.
**Creating or updating `-shm` alongside an existing WAL is permitted SQLite WAL
protocol. Creating a `-wal` or `-journal` is not permitted.** A zero-byte stale
`-journal` does not select normal access. With no WAL, use `mode=ro&immutable=1`,
even if an orphan SHM or zero-byte journal exists. Nonempty rollback journals
without WAL are conservatively skipped with a locked-store warning, rather than
risk reading spilled uncommitted pages or performing recovery.

Both sources use one scan wrapper. Before immutable access it records database
size and nanosecond mtime/ctime, Unix device/inode identity, SQLite header change
counter/version signals (bytes 24–27 and 92–99), and the WAL/SHM/journal set and
metadata. After the complete scan it
compares them again: any change or metadata error discards all sessions, counts
one skipped store and warns with the path. There is no retry through a writer's
lock. This catches a writer starting after the initial sidecar decision. Normal
stable scans print no SQLite warning. Missing stores increment skipped stores
silently. Busy/locked/corrupt/schema failures warn and discard partial sessions.
Row conversion failures skip/count the row; SQLite step/I/O errors fail the store.

After opening the connection and before the schema read, re-snapshot database
and sidecar identities/set; a changed target closes/discards the connection with
a warning. The private `sqlite_vfs` module also removes CREATE, EXCLUSIVE and
DELETEONCLOSE from WAL/main-journal/super-journal xOpen flags before delegating to
SQLite's platform VFS. The OS open therefore cannot recreate a WAL removed in
the later window between that check and the actual WAL open. SQLite's platform
locking and approved SHM protocol remain in use; no cleanup deletes agent files.

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

Round-3 regressions: `tests/sqlite_round3.rs` covers a stale zero-byte journal,
orphan committed WAL with and without SHM, full byte snapshots with the explicit
SHM exception, NULL/non-integer timestamp counts, relative OPENCODE_DB discovery,
and embedded-NUL/invalid-UTF-8 byte tails. `source::sqlite::tests` starts a
pre-opened spilling writer after the first schema query, confirms the candidate
contains an uncommitted title, and proves the scan wrapper discards it. Separate
checks change only database size, only mtime, or only the sidecar set. The watcher
probe also verifies bare relative `OPENCODE_DB=opencode.db` refreshes automatically.

Round-4 regressions remove WAL both before connection open and at the actual VFS
file-open boundary, asserting no WAL/SHM/journal is left. Re-enabling CREATE makes
the latter test fail. Snapshot tests reject same-size committed rewrites, rename
replacement with matching size/mtime, and a spilling MEMORY-journal writer with
restored mtime and an observed uncommitted candidate. Header tests independently
verify that all requested counter/version ranges are read from the file.

Remaining limits: relative overrides are resolved again from cwd on each registry
rebuild; only an in-process chdir can change that result. SQLite may still
materialize very large values internally: the attack's 100 MB content case peaked
near 310 MB RSS despite bounded Rust previews. Neither is a one-line fix, and the
100,000-row historical benchmark is not a bound for a single huge content value.
