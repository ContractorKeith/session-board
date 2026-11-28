# Hermetic, event-synchronized tests

Tests run on developer machines that have real agent stores, a real tmux server and git hooks,
and on loaded CI runners. They must never touch the owner's files and must not depend on timing.

## Isolation (`tests/common/mod.rs`, `tests/harness.rs`)
- git runs hermetically: no inherited `GIT_DIR` (a hook running `cargo test` would otherwise
  commit into the developer's repo) and no global config (gpgsign, hooks path, identity).
- Every binary scan goes through `common::isolate`: no tmux server, and no inherited
  `GROK_HOME`, `OPENCODE_DB` or `XDG_DATA_HOME`, which would point at the owner's real stores.
  Every subprocess gets both sboard overrides. Binary tests pin `window_days` so the fixed-date
  fixtures never age out of the scan window.
- Environment-changing probes run in a child process so the change cannot leak into other
  tests.
- Desktop notifiers are fake executables, never a real desktop server.
- tmux tests force UTF-8 (`-u`) so pane paths do not depend on the runner's locale, and fix only
  the agent label: renaming a copied system executable does not give a portable process name.

## Synchronization
- Wait on observable events (a rendered frame, watcher state, a gate the test releases), not
  stopwatches. Where a performance regression matters, count work (process-table snapshots)
  instead of timing it.
- tmux can reply before a new pane's child has entered its cwd; wait for it.
- Timed byte tests write straight to the board's PTY, not through `script(1)`. macOS `sleep`
  coalesces sub-50 ms delays, so those probes busy-wait.
- Linux tests reap forced-exit grandchildren themselves; a killed orphan briefly awaiting
  init's reap counts as not running. macOS may wait for PTY output to drain before session exit.

## Platform and cost
- macOS temp roots have a symlinked prefix; the same case is exercised on Linux.
- The macOS CI filesystem rejects non-UTF-8 names, so that test is Linux-only.
- The RSS probe for large stores is `#[ignore]` and run by hand.
- `clippy::unwrap_used` applies to production code only (`cfg_attr(not(test))` in `lib.rs` and
  `main.rs`), so integration-test helpers can unwrap.
