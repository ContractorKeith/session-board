---
name: skeptic-env
description: "How a skeptical reviewer builds, runs and attacks session-board (sboard). Read by the skeptic-review skill; not a workflow on its own."
---

# skeptic-env — session-board

One Rust binary, `sboard`, that reads the sessions Claude Code, Codex, Grok and OpenCode write
to disk, groups them by project, derives status from the transcript tail, jumps or resumes, and
launches new sessions. It observes agents and never owns them (ADR-0001). "Done" for v1 is
every v1 row in `docs/PLAN.md` §4 passing §10 on Omarchy, the Mac Studio and CI.

## Stack and commands

```bash
cargo build --release && ./target/release/sboard version
cargo fmt --all --check
cargo clippy --locked --all-targets -- -D warnings   # exactly as CI
cargo test --locked                                  # fixtures only; no live agent, tmux or network
cargo run -- doctor                            # which real stores this box has
cargo run -- list --json | head
```

Scratch state: point `SBOARD_STATE_DIR` and `SBOARD_CONFIG_DIR` at temp dirs so a review never
touches `~/.local/state/sboard` or `~/.config/sboard`. The real agent stores on this box are
read-only inputs; sboard must never write inside `~/.claude`, `~/.codex`, `~/.grok`.

## Platforms

| Platform | Ships | Review on | Notes |
|---|---|---|---|
| Linux | yes | Omarchy | live Claude Code, Codex and Grok stores; tmux installed, usually no server running |
| macOS | yes | Mac Studio | live Claude Code and Codex stores; `osascript` notifier |
| Windows | no | | |

## What to attack

- **Sources**: truncated last line (an agent mid-write), a 0-byte file, a file with only
  `queue-operation` lines, a Codex rollout with no `session_meta`, a `thread_source` that is a
  subagent, a session whose `cwd` no longer exists, a cwd that is a worktree, thousands of
  files in one store, non-UTF-8 bytes, a store that is a symlink, an unreadable file.
- **Status rules**: each rule in PLAN §6 against a fixture and against a live store; a tool
  call whose result arrives in the next line; `end_turn` whose text ends in `?` inside a code
  block; a session edited while sboard is watching.
- **State file**: concurrent writers (two sboard instances), a corrupt TOML, a read-only state
  dir, a pin for a project that no longer exists.
- **tmux and launch**: no tmux server; tmux on a custom socket; two panes with the same cwd;
  a pane whose process is a shell that will later run claude; `w` with a branch name that
  already exists, has spaces or `..`; a non-git project.
- **Test honesty**: which acceptance claim has no test; a test that still passes with its
  assertion removed; CI green on macOS while skipping something.
- **Docs as typed**: every command in `AGENTS.md` from a fresh shell in the worktree.

## Traps

- If /tmp is full, set TMPDIR to a scratch directory outside every git checkout.
  The attention CLI tests resolve fixture projects through git; a TMPDIR inside a
  worktree incorrectly groups them under that enclosing repository.
- Status from the transcript is intended; pane contents are never read. If the pane shows a
  prompt and the transcript says working, the transcript wins and that is a status-rule bug,
  not a design bug.
- Subagent and guardian Codex threads and Claude sidechains are dropped on purpose.
- `.claude/skills` is a symlink to `.agents/skills`; `CLAUDE.md` is one line (`@AGENTS.md`).

## Earlier rounds

- Round 1 on PR #9 head 96d50d9 (Linux, Omarchy; macOS, Mac Studio): bugs #10–#15 fixed on the
  branch; tracked weaknesses #16–#18. Report: worklog/reports/session-board-review-2026-11-01.md.
