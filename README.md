# session-board

`sboard`: a multi-project agent session organizer for a side terminal. One Rust binary that
reads the sessions Claude Code, Codex, Grok and OpenCode already write to disk, groups them by
project, shows working / waiting / idle / error / done, jumps to the tmux pane or resumes the
session, and launches new sessions in a project or a fresh git worktree. Observes agents; never
owns them. MIT.

Status: M1 in review: Claude Code and Codex sources, transcript status rules, filtered
`list` and doctor counts. TUI arrives in M2. See `docs/PLAN.md` for milestones and
`AGENTS.md` for commands. The package is `session-board`; the binary is `sboard`.

```bash
sboard list --json
sboard list --status attention --agent codex
sboard list --project ~/Projects/example --all
sboard doctor
```

List uses a seven-day window by default. `--all` also includes archived sessions and
projects. Attention sorts waiting, error, then done-unread within each project;
projects sort by their highest-priority session, then its activity. Normal lists
sort projects by pin and activity, sessions by activity. No read mark still means
unread in M1; first-run semantics remain M4's decision (#16). IDs include the agent
prefix. With no enabled implemented store found, list emits its tree and exits 2;
a found but empty store succeeds. Doctor is diagnostic and succeeds with missing
stores. Broken output pipes exit 0.


State: `~/.local/state/sboard/state.toml` (override `SBOARD_STATE_DIR`). Config:
`~/.config/sboard/config.toml` (override `SBOARD_CONFIG_DIR`); every key is optional and the
defaults are in `docs/PLAN.md` §6.

Config, state and the optional Codex title index are checked as regular files before
reading. FIFOs, devices, directories and symlink files are rejected without opening
them; bad config is an error, bad state warns and uses empty state, and a bad index
warns/counts a skipped store while falling back to prompt titles.
