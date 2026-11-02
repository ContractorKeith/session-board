# session-board

`sboard`: a multi-project agent session organizer for a side terminal. One Rust binary that
reads the sessions Claude Code, Codex, Grok and OpenCode already write to disk, groups them by
project, shows working / waiting / idle / error / done, jumps to the tmux pane or resumes the
session, and launches new sessions in a project or a fresh git worktree. Observes agents; never
owns them. MIT.

Status: v0.1.0 release candidate. See `docs/PLAN.md` for scope and `AGENTS.md` for commands.
The package is `session-board`; the binary is `sboard`.

## Install

Download `SHA256SUMS` and the archive for your platform from the
[releases page](https://github.com/ContractorKeith/session-board/releases):

| Platform | Target |
|---|---|
| Linux x86_64 | `x86_64-unknown-linux-gnu` |
| Linux aarch64 | `aarch64-unknown-linux-gnu` |
| macOS Apple Silicon | `aarch64-apple-darwin` |

From the download directory, verify the archive, extract it, and put `sboard` on PATH.
For example, on Linux x86_64:

```bash
sha256sum -c SHA256SUMS --ignore-missing
tar -xzf sboard-v0.1.0-x86_64-unknown-linux-gnu.tar.gz
mkdir -p "$HOME/.local/bin"
install -m 755 sboard "$HOME/.local/bin/sboard"
export PATH="$HOME/.local/bin:$PATH" # add to your shell profile for future terminals
sboard version                       # sboard 0.1.0
```

On macOS, verify with `shasum -a 256 -c SHA256SUMS --ignore-missing` and use the
`aarch64-apple-darwin` archive. Each archive includes the README and MIT license.
A Homebrew tap is planned for later. tmux enables jumps and launches; install an
agent CLI to resume or launch its sessions.

## Usage

Run `sboard` in a terminal to see the project tree and session details; stores refresh live; config, state and Codex title-index edits need `r`.
Use `Tab` to switch between attention and the project tree, `j`/`k` or arrows to move, `h`/`l` to fold/unfold, and `r` to rescan.
Use `p` to pin the selected project, `t` to edit comma-separated tags in the status bar
(Enter saves, Esc cancels), `m` to mark the selected session or project read, `a` to archive
or unarchive it, and `A` to include archived rows. `s` cycles activity/attention sorting;
`g` switches between project and tag groups. In tag groups, select a session to pin or tag
its real project; `a` archives the selected session. Pins always sort first.
Ctrl-J acts as Enter and Ctrl-H as Backspace.
Press `?` for help, `q` or Ctrl-C to quit.
Enter jumps or resumes; `n` launches in the selected project; `w` launches in a fresh git worktree.
Use `sboard open <session-id>` or `sboard new <project-path> [--agent claude_code|codex] [--worktree <branch>]`.

```bash
sboard list --json --sort attention
sboard list --group tag
sboard list --status attention --agent codex
sboard list --project ~/Projects/example --all
sboard doctor
```

List uses a seven-day window by default. `--all` also includes archived sessions and
projects. Attention sorts waiting, error, then done-unread within each project;
pinned projects sort first, then by their highest-priority session and latest activity.
Activity mode sorts by pin and activity. Tag groups use `tag:<name>` roots (`%` and `/`
escaped as `%25` and `%2F` to preserve identity) and retain each
session's real `project_root`; projects without tags appear under `untagged:` (displayed as untagged). JSON sessions carry a `group` field in tag view
and appear once per tag; unread counts are per group copy.
Sessions without read marks are read until the first state save establishes `first_seen`;
activity after that timestamp is unread, and selecting a session clears its mark.
IDs include the agent prefix. With no enabled implemented store found, list emits its tree and exits 2;
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

State saves merge read marks by maximum timestamp and apply only additions/removals since
the last-loaded snapshot onto the latest pins, tags, archives and preferences. The directory
lock times out after two seconds. Corrupt
state is copied to `state.toml.corrupt-<unix seconds>` before saving. Invalid UTF-8 state
paths and sessions are skipped with a warning. Sort, grouping and archived-view preferences
survive TUI restarts. `list` always defaults to activity sorting and project grouping;
`--sort` and `--group` change only that invocation.

Terminal input is read independently of scans and saves. Paste bursts end only after the
reader observes a quiet gap; pending type-ahead is discarded when transferring the terminal
to or from an agent. Tag prompts merge edits against the tags present when they opened.
The attention pane shows waiting, error and done-unread sessions in the same order as
`sboard list --status attention --json`. With attention focused, `h` collapses it and `l`
expands it; collapsed panes ignore movement. Navigation keeps projects folded; Enter unfolds
the selected project and uses the same jump/resume handler as the tree.
The pane uses at most one third of the terminal, with a three-line minimum when non-empty;
`+N above` and `+N below` show hidden rows.

Desktop notifications are off by default. Add `notify = true` at the top level of
`config.toml` to notify on transitions into waiting, done or error. The first scan
is silent. Sessions first discovered later notify only if their activity is at most five
minutes old; known sessions notify when their status/activity pair changes. Linux uses
`notify-send --app-name sboard`; macOS uses `osascript`.
Delivery runs off the UI thread through a bounded queue. More than five transitions in one
scan produce one summary; commands exceeding five seconds are killed. Missing or failing
commands warn once in the status bar, and a missing command becomes a no-op.

Desktop notifications are per sboard process: two instances may each announce the same
transition. A notifier that deliberately daemonizes with `setsid()` and forks again leaves
the command process group and is outside the five-second process-group timeout.

Attention JSON is a flat queue ordered globally by waiting, error and done-unread.
