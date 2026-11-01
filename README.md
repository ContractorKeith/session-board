# session-board

`sboard`: a multi-project agent session organizer for a side terminal. One Rust binary that
reads the sessions Claude Code, Codex, Grok and OpenCode already write to disk, groups them by
project, shows working / waiting / idle / error / done, jumps to the tmux pane or resumes the
session, and launches new sessions in a project or a fresh git worktree. Observes agents; never
owns them. MIT.

M0 provides the model, source registry, scan pipeline, state/config persistence,
fixture harness, and CI. Real sources land in M1 and the TUI in M2; the registry
is empty, so `list --json` currently returns `[]` with default configuration.
See `docs/PLAN.md` for the remaining milestones.

```bash
cargo build --release
./target/release/sboard version
cargo run -- list --json
cargo run -- doctor
cargo fmt --all --check
cargo clippy --all-targets -- -D warnings
cargo test
```

The Cargo package is `session-board`; the binary is `sboard`. Running without a
subcommand prints `TUI lands in M2`. Doctor checks existence of the four known
store paths without opening their contents. M0 returns success for an empty
list or missing stores; the full CLI filters and diagnostics arrive later.

State is `~/.local/state/sboard/state.toml`, overridden by `SBOARD_STATE_DIR`.
Config is `~/.config/sboard/config.toml`, overridden by `SBOARD_CONFIG_DIR`.
Both paths use these defaults on Linux and macOS. Missing files use defaults;
commands do not create them automatically. Library save methods write a sibling
temporary file and rename it over the destination. Corrupt state warns and loads
empty; invalid config reports an error; unknown config keys warn and are ignored.
Every config key is optional:

```toml
roots = []
window_days = 7
notify = false

[agents]
claude_code = true
codex = true
grok = true
opencode = true

[tmux]
socket = ""
```

Timestamps in the model and read marks are integer Unix seconds in UTC. Scan
resolves each session cwd through a caller-supplied resolver, groups by project,
and applies project pins, tags, archive flags, and unread counts. Git discovery
lives in `project`, cached per cwd with cwd fallback, keeping `scan` free of I/O.
Session archive view filtering, scan-window filtering, attention ordering, and
interactive state changes belong to later milestones.

Tests use temporary state/config directories and fake sources; no live agent
store is needed. `fixtures/list.json` is the empty-registry golden output.
Per-agent fixture directories await sanitized real transcripts in M1/M6.
