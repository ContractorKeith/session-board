# Grok source

Store: `${GROK_HOME:-~/.grok}/sessions/session_search.sqlite`. Empty overrides
fall back to HOME. `source::database_paths` resolves relative overrides against
cwd for the registry, doctor and watcher. The watcher watches the parent directory
and reattaches after it appears or is recreated.

Read `session_docs` columns `session_id`, `cwd`, `updated_at`, `title`, `content`;
ignore `content_hash` and the FTS5 mirror. Native IDs must match `[A-Za-z0-9._:-]+`
without a leading `-`; invalid IDs skip/count sessions rather than being sanitized
into different identities. IDs become `grok:<session_id>`. Empty, relative or
NUL-bearing cwd skips/counts sessions. Normalize absolute cwd and clean titles
with the shared printable-line sanitizer. Resume is `grok --resume=<session_id>`;
the installed parser accepts `grok --resume= --help` without resuming anything.
Transcript path is the database file.

`updated_at` is Unix seconds, or milliseconds when **at least** 10^12. It supplies
both start and activity time. Values more than one hour in the future clamp to
scan time with a path/id warning. Negative or invalid values skip/count rows;
NULL/non-integer timestamps are included for conversion/counting. SQL compares
seconds/milliseconds separately for the scan window; doctor reads all ages.

SQL selects `substr(CAST(content AS BLOB), -8192)` and a bounded first non-blank
line candidate. Full content never enters Rust, though SQLite may materialize a
large value internally. Title prefers the cleaned `title`, then first non-blank
content line (60 characters), then native ID. `last_assistant_text` contains the
latest 8 KiB decoded lossily and capped again on a UTF-8 boundary; embedded NUL
is retained. This flat index can contain tool names, not guaranteed assistant
prose. `last_user_text` is empty because roles cannot be recovered reliably.

Status is recency only: **done** below 3600 seconds, otherwise **idle**, including
exactly 3600 seconds. Cannot derive working/waiting or explicit errors from the
flat index, even when content ends in a question.

## SQLite access

Header bytes 18/19 select the rule: WAL format (`2,2`) with WAL uses normal
`mode=ro`, WAL format without WAL uses `mode=ro&immutable=1` with before/after
identity/header validation, and rollback format (`1,1`) always uses normal
`mode=ro` locking. Unreadable/short headers or other version pairs skip with one
warning. Every connection is READ_ONLY / NO_MUTEX with a 200 ms busy timeout;
`nolock` is never used.

WAL writers write the main file during checkpoints, which require WAL; a WAL
appearing before open changes the sidecar set and discards the connection, and
changes during the immutable scan discard its results. Rollback-format stores
never use immutable: MEMORY/OFF-journal writers can spill pages before a snapshot,
so SQLite's normal SHARED lock must exclude them. A busy/locked store contributes
zero sessions, one skipped store and one path warning. A hot rollback journal
fails read-only rather than being recovered.

**SHM creation/updates beside an existing WAL are permitted SQLite WAL protocol.
Creating a WAL or rollback journal is forbidden.** A private VFS removes CREATE,
EXCLUSIVE and DELETEONCLOSE from WAL/main-journal/super-journal xOpen flags, then
delegates to SQLite's platform VFS. Removing WAL between a check and the actual
OS open cannot make the observer recreate it. No cleanup deletes agent files.
A zero-byte journal beside a rollback-format store is ignored normally.

Before any connection open, `symlink_metadata` checks the database and each
`-wal`, `-shm`, `-journal` entry. Existing entries must be regular files: FIFO,
directory, socket, device and all final symlinks (including dangling ones) skip
the store with one warning. Missing sidecars are normal; a missing database
counts as skipped without warning. The checks repeat during snapshots. A malicious
process with write access to the user's agent directory can atomically alternate
the path between a regular file and FIFO after this check but before SQLite
opens it, hanging the scan. This
accepted race requires an adversary writing into the user's own home directory.
After opening and before the first schema read, database/sidecar identities and
set must still match or the connection is discarded. The VFS independently
prevents file creation in the later OS-open window. Rollback-journal appearance
or removal is normal during commits and is left to SQLite's lock/recovery rules.

The common scan wrapper discards partial results on step/I/O/schema errors;
conversion errors skip/count only the affected row. Only the immutable path uses
a before/after fingerprint of device/inode, size, nanosecond mtime/ctime, header
bytes 24–27 and 92–99, and sidecar set/metadata. This detects replacement or raw
file changes, validating the closed-WAL immutable view. Normal `mode=ro` scans
rely on SQLite locks; a commit between discovery and SELECT is a healthy old/new
view and does not discard the store.

## Fixtures and verification

SQL fixtures are committed and generated only in temp directories. Fixed clock:
2026-11-01 12:00 UTC.

| Row in `fixtures/grok/session_search.sql` | Rule |
|---|---|
| recent | Milliseconds, normalized cwd, explicit title, recent done despite question text |
| old | Seconds, first-line fallback title, 61-minute idle |
| relative | Relative cwd skipped and counted |

`tests/sqlite_sources.rs` covers base rules, UTF-8 bounds, missing/special/corrupt
stores, reads of a closed WAL-format store without sidecars, doctor/filters,
and full byte snapshots with the explicit SHM exception.
`tests/sqlite_round2.rs` covers live committed WAL, spilled locked writes,
corrupt middle leaves in both sources, conversion failures, validation, titles,
future/millisecond boundaries, window filtering and transient-sidecar polling.
`tests/sqlite_round3.rs` covers stale journals, orphan WAL with/without SHM,
NULL timestamp counts, relative overrides and NUL/invalid-UTF-8 byte tails.
`source::sqlite::tests` covers read-only fd/write oracles, writer races, file/header
fingerprints and WAL removal before/inside VFS xOpen. A CREATE mutation fails the
actual-open regression. Header validation and a WAL appearing before an immutable
open have regressions too. Post-query MEMORY writers fail through SQLite locking.

`tests/sqlite_round5.rs` keeps a 2,999-insert MEMORY transaction open **before**
the first snapshot. An independent SQLite process verifies the EXCLUSIVE lock;
both CLI list and doctor must skip/warn, then read committed rows after rollback.
Database and all three sidecars have FIFO/directory/socket/symlink cases; sidecar
CLI probes have a bounded deadline. Its ignored stress test runs 20 concurrent
lists during WAL commits while polling filenames and asserting no new sidecar.
`tests/watch_sources.rs` uses isolated `Scans::start` subprocesses for override
paths, updates, missing-root warnings, reattachment and bare relative OPENCODE_DB.

The ignored 100,000-row fixture measures a historical corpus with 8 KiB per row,
not 100,000 active previews or a single huge cell. The attack's single 100 MB
content value peaked near 310 MB RSS inside SQLite despite bounded Rust previews.
Relative paths are still re-resolved if the process itself changes cwd; current
launches change only child cwd. Neither weakness has been redesigned here.

Round-7 regressions commit between the fingerprint and first table SELECT in
DELETE/MEMORY/OFF modes without discarding rows or warning. Immutable-only raw
size/mtime/sidecar, rewrite/replacement and WAL-appearance checks still apply.

Run `python3 tests/sqlite_delete.py target/release/sboard` for 50 concurrent lists
per source against DELETE commits. It rejects warnings/discards, permits only the
writer's temporary journal, checks the final sidecar set and reaps all children.
