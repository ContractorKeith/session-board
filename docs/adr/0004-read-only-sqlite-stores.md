# Read-only SQLite stores

Grok and OpenCode keep sessions in SQLite databases owned by a live agent. ADR-0001 says sboard
never writes to an agent store; for SQLite that takes more than `mode=ro`, because SQLite's
stock WAL opener requests CREATE even for read-only databases.

## Opening (`src/source/sqlite.rs`, `src/source/sqlite_vfs.rs`)
- A custom VFS keeps the platform VFS and its locking and SHM protocol but forbids WAL and
  journal creation. Its callbacks never panic across the C ABI.
- `regular_entry` inspects the path itself: even a symlink to a regular file is not a regular
  store or sidecar entry. Missing sidecars are normal; missing stores are counted.
- `open`: only a WAL-format store without a WAL file may bypass locks, and only with snapshot
  validation. Otherwise SQLite decides when reads lock.
- A rollback writer may create or remove its journal while holding only RESERVED, so a journal
  appearing between snapshots is not a changed store.
- URIs percent-encode delimiters, including a literal `%` in project paths.
- `sqlite3_open_v2` defers WAL and SHM access until the schema probe; revalidate before that
  first read. The VFS also guards the later OS open.

## Snapshots
- A snapshot records the header change counter (bytes 24..28), version-valid-for and the SQLite
  version (92..100), plus identity, ctime, size and mtime of the store and its WAL, SHM and
  rollback journal.
- Validation brackets the complete scan, so neither source can return a stale immutable view
  after a writer replaces or changes a closed WAL store.
- OpenCode reads session, message and part rows inside one transaction (one snapshot).

## Rows
- Only conversion failures concern a row; step and I/O errors fail the whole store.
- OpenCode roles and completion belong to `message.data`; prose and tool state to `part.data`.
- `fixtures/opencode/opencode.sql` holds selected columns of the documented schema; omitted
  columns are unused.

## Tests
- Tests exercise the WAL protocol with a genuinely read-only database descriptor, keep a
  pre-opened writer alive to hold its exclusive lock and spilled pages, and compare every
  captured byte of the store, allowing only SQLite's own SHM protocol writes while a WAL exists.
  A DELETE-mode writer's transient journal is the writer's; WAL and SHM never appear.
