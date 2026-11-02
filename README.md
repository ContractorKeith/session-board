# session-board

`sboard`: a multi-project agent session organizer for a side terminal. One Rust binary that
reads the sessions Claude Code, Codex, Grok and OpenCode already write to disk, groups them by
project, shows working / waiting / idle / error / done, jumps to the tmux pane or resumes the
session, and launches new sessions in a project or a fresh git worktree. Observes agents; never
owns them. MIT.

Status: M5 in review: attention queue and optional desktop notifications over the Claude Code
and Codex sources; M3 launch is integrated; M4 read marks follow its merge. See `docs/PLAN.md` for milestones and
`AGENTS.md` for commands. The package is `session-board`; the binary is `sboard`.

Run `sboard` in a terminal to see attention, the project tree and session details. Stores refresh live.
`notify` is re-read on every successful scan; `r` requests an immediate scan after config, state
or Codex title-index edits. Quiet stores also refresh every minute.
Use `Tab` to switch between attention and the project tree, `j`/`k` or arrows to move, `h`/`l` to fold/unfold, and `r` to rescan.
Press `?` for help, `q` or Ctrl-C to quit.
Enter jumps or resumes; `n` launches in the selected project; `w` launches in a fresh git worktree.
Use `sboard open <session-id>` or `sboard new <project-path> [--agent claude_code|codex] [--worktree <branch>]`.

```bash
sboard list --json
sboard list --status attention --agent codex
sboard list --project ~/Projects/example --all
sboard doctor
```

List uses a seven-day window by default. `--all` also includes archived sessions and
projects. The pane and attention JSON sort sessions globally: waiting, error, then
done-unread, followed by newest activity and ID. Attention text output keeps the
project tree, ordering projects by their best attention item. Normal lists
sort projects by pin and activity, sessions by activity. No read mark still means
unread in M1; first-run semantics remain M4's decision (#16). IDs include the agent
prefix. With no enabled implemented store found, list emits its tree and exits 2;
a found but empty store succeeds. Doctor is diagnostic and succeeds with missing
stores; `doctor --json` returns `{ "stores": [...], "tmux": "..." }`, distinguishing
the tmux version, no server, and a missing executable. Broken output pipes exit 0.


State: `~/.local/state/sboard/state.toml` (override `SBOARD_STATE_DIR`). Config:
`~/.config/sboard/config.toml` (override `SBOARD_CONFIG_DIR`); every key is optional and the
defaults are in `docs/PLAN.md` §6.

Config, state and the optional Codex title index are checked as regular files before
reading. FIFOs, devices, directories and symlink files are rejected without opening
them; bad config is an error, bad state warns and uses empty state, and a bad index
warns/counts a skipped store while falling back to prompt titles.

The attention pane shows waiting, error and done-unread sessions in the same order as
`sboard list --status attention --json`. With attention focused, `h` collapses it and `l`
expands it; collapsed panes ignore movement. Navigation keeps projects folded; Enter unfolds
the selected project and uses the same jump/resume handler as the tree.
The pane uses at most one third of the terminal, with a three-line minimum when non-empty;
`+N above` and `+N below` show hidden rows.

Desktop notifications are off by default. Add `notify = true` at the top level of
`config.toml` to notify on transitions into waiting, done or error. The first scan
is silent. Linux uses `notify-send --app-name sboard`; macOS uses `osascript`.
Delivery runs off the UI thread through a bounded queue. More than five transitions in one
scan produce one summary; commands exceeding five seconds are killed. Missing or failing
commands warn once in the status bar, and a missing command becomes a no-op.

Desktop notifications are per sboard process: two instances may each announce the same
transition. A notifier that deliberately daemonizes with `setsid()` and forks again leaves
the command process group and is outside the five-second process-group timeout.
