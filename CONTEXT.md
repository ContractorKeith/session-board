# session-board

The words sboard uses for what the agent CLIs leave on disk and what you do with it. One
definition each.

## Language

**Session**:
One conversation an agent CLI recorded on disk: one Claude Code JSONL file, one Codex rollout,
one Grok or OpenCode row. It has a working directory, a last activity time, and a status.
_Avoid_: thread, conversation, chat, run

**Agent**:
The CLI that wrote the session: `claude_code`, `codex`, `grok`, `opencode`. Not a persona and
not the model.
_Avoid_: tool, runtime, provider, model

**Source**:
The reader for one agent's store. It scans, never writes, and skips what it cannot parse.
_Avoid_: adapter, plugin, backend, collector

**Store**:
The directory or database an agent writes sessions into (`~/.claude/projects`,
`~/.codex/sessions`, ...). Found or skipped by `doctor`.
_Avoid_: log dir, history, cache

**Project**:
The git top-level of a session's working directory, or the directory itself when it is not in
a repo. Sessions group under it. A worktree is its own project.
_Avoid_: repo, workspace, folder

**Status**:
What the transcript tail says the session is doing: `working`, `waiting`, `idle`, `error`,
`done`. Derived, never stored.
_Avoid_: state, health, mode

**Attention**:
The subset that needs you: `waiting`, `error`, or `done` and unread. Shown in the top pane and
by `list --status attention`.
_Avoid_: alerts, inbox, notifications (that word is for the desktop popup)

**Unread**:
A session changed since you last looked at it. sboard's own mark, kept in the state file.
_Avoid_: new, dirty, pending

**Jump**:
Switch the terminal to the tmux pane where the session is running.
_Avoid_: attach, focus, open (open is the key that does jump-or-resume)

**Resume**:
Start the agent's own resume command for a session that has no live pane, in its directory.
_Avoid_: reopen, restore, continue

**Launch**:
Start a new session in a project, or in a fresh worktree of it.
_Avoid_: spawn, create, start

**State file**:
sboard's own pins, tags, read marks and archive list in `~/.local/state/sboard/state.toml`.
The only file sboard writes besides its config.
_Avoid_: database, cache, index

**Owner**:
The one person running sboard on their own machine. There are no other users.
_Avoid_: user, admin, operator
