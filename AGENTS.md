# session-board

A multi-project agent session organizer for a side terminal: one Rust binary, `sboard`, that
reads the sessions Claude Code, Codex, Grok and OpenCode already write to disk, groups them by
project, derives working / waiting / idle / error / done from the transcript tail, jumps to the
tmux pane or resumes the session, launches new sessions in a project or a fresh git worktree,
and keeps its own pins, tags, read marks and archive. It observes agents and never owns them.
MIT. "Done" for v1 is every row marked v1 in `docs/PLAN.md` §4 passing the acceptance checks in
§10 on Omarchy, the Mac Studio and GitHub Actions.

## Status
M5 in review — attention pane and optional desktop notifications. M0–M3 merged;
M4 persisted read marks integrate before M5 lands.

## Commands
```bash
cargo build --release          # target/release/sboard
cargo test --locked            # unit tests on fixtures, no live agent needed
cargo fmt --all --check
cargo clippy --locked --all-targets -- -D warnings
cargo run -- list --json       # cold scan of the real stores on this machine
cargo run -- doctor
```
(Keep this block in step with `Cargo.toml` and the CI workflow.)

## Pointers
- `docs/PLAN.md` — problem, feature matrix, architecture, status rules, CLI contract,
  milestones, testing, acceptance.
- `CONTEXT.md` — glossary. Use its words in code, issues and PRs.
- `docs/adr/` — observe never own; Rust single binary.
- `docs/sources/<agent>.md` — one page per source: paths, line types, status rules, fixtures.
- Session logs: `~/Projects/worklog/projects/session-board.md` (not in this repo).

## Conventions & Gotchas
- Sources are read-only and tolerant. Skip and count malformed lines; never panic on a store.
  Never write under `~/.claude`, `~/.codex`, `~/.grok` or any agent directory (ADR-0001).
- Status comes from the transcript, not from pane scraping. tmux is for jump and launch only.
- Codex stores contain subagent and guardian sessions (object `source` or explicit non-user
  `thread_source`); drop them. Missing `thread_source` with string `source` is an owner session. Claude stores contain `api-request*`, `attachment`, `queue-operation` and sidechain
  lines; skip them.
- `model`, `status`, `state`, `scan` and `ui` do no I/O beyond the state and config files.
- Fixtures are real transcripts with text replaced by placeholders. Every status rule has one.
- One process, no daemon. The watcher runs inside the TUI; `list` is a cold scan.
- Tooling is cargo, rustfmt, clippy. No nightly, no build scripts.
- One milestone = one issue = one branch = one PR = one T3 thread in its own worktree. M0 lands
  alone; M1 then M2 sequential; M3–M6 may run in parallel after M2 (merge order M3, M4, M5,
  M6); M7 last.
- A PR needs a completed green CI check and then a `skeptic-review` round before leaving draft.
  Skeptic findings become labelled issues. Keith marks ready; workers do not.
- Commits: `<type>: <description>`, author @ContractorKeith only.

## Tracking
- GitHub issues are the tickets, one per milestone, label `milestone`.
- Basecamp project "github" (card table) has one card per milestone titled
  `session-board Mn: ...` linking the issue and the PLAN section. Move the card to In progress
  when the branch opens and to Done when the PR merges. No per-issue cards.

## Out of Scope
- Editor, persona builder, chat surface, sub-agent trees, Docker sandbox, conductor bots. Never.
- Cost rollup, worktree cleanup, search, GitHub overlay: v0.2. Routines, MCP, digest, web view:
  v0.3. Not before v0.1.0 is tagged.
