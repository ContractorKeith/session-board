# session-board — Build Plan

Status: v0.1.0 release candidate; acceptance tracked in M7 (#8). Written from Keith's scope note and a read of
[agent-deck](https://github.com/asheshgoplani/agent-deck) (Go, MIT, tmux-backed, 1.2k stars),
[agenthud](https://github.com/neochoon/agenthud) (TypeScript, MIT, read-only HUD) and
[agent-mux](https://github.com/leonardcser/agent-mux) (Rust, MIT, tmux pane list), with a
glance at agent-trail and opensession. Glossary: `CONTEXT.md`. Decisions that are hard to
reverse: `docs/adr/`.

Scope model: agenthud (observe on-disk sessions, never own them) plus agent-mux (jump, unread,
stash). Process model: hoist and mdkb (one milestone, one issue, one branch, one PR, one thread).

## 1. Problem

Eight repos, each with one or more agent sessions in Claude Code, Codex, Grok or OpenCode, in
T3 threads, tmux panes and plain terminals. The question "which repo has an agent that is stuck,
waiting on me, or finished?" has no single screen. Ködade answers it inside one window with a
full ADE; agent-deck answers it by owning every session in tmux. Neither fits a side terminal
next to whatever is already running.

## 2. Solution

`sboard` is one Rust binary with a TUI and a CLI. It reads the session files the agent CLIs
already write, merges them into one tree grouped by project, derives a status for each session
from the shape of its transcript, and shows what needs you. Enter jumps to the native session
when it can be found (tmux pane) or resumes it in place; one keystroke launches a new session in
the project or in a fresh git worktree. Pins, tags, read marks and archive are its own state.
It never edits an agent's config and never holds the agent process. MIT.

v1 is done when every row marked **v1** in §4 passes the acceptance checks in §10 on Linux
(Omarchy) and macOS (Mac Studio) and in GitHub Actions.

## 3. User stories

1. As the owner, I open `sboard` in a side terminal and see every project that has had an agent
   session in the last 7 days, with its sessions and a status glyph, in under a second.
2. As the owner, I see at the top a "needs you" list: sessions waiting for input, finished since
   I last looked, or errored. Reading one clears its mark.
3. As the owner, I press Enter on a session that is running in a tmux pane and land in that
   pane. On a session with no pane, I get `claude --resume <id>` (or the Codex equivalent)
   started in the directory it belongs to.
4. As the owner, I press `n` on a project and get a new agent session in that project; `w` does
   the same in a fresh git worktree named for the branch I type.
5. As the owner, I pin three projects to the top, tag two as `client`, sort the rest by last
   activity, and archive a project I am done with. That state survives restarts.
6. As the owner, I run `sboard list --json` from another agent or script and get the same tree
   as the TUI, so an agent can ask "what is active?".
7. As the owner, I run `sboard doctor` and see which agent stores were found, how many sessions
   each contributed, and which ones were skipped and why.
8. As a maintainer, I run `cargo test` and the sources are proven against fixture transcripts
   checked into the repo, no live agent needed.

## 4. Feature matrix

**v1** = in scope now. **Later** = after v1 (v0.2, v0.3). **Drop** = deliberately never.

| # | Feature | Decision | Milestone | Notes |
|---|---|---|---|---|
| 1 | `Session`, `Project`, `Status` model and the `Source` trait | v1 | M0 | The one seam every source implements |
| 2 | Fixture harness: `fixtures/<agent>/` transcripts, golden `list --json` output | v1 | M0 | Sources are tested only on fixtures |
| 3 | State file: pins, tags, read marks, archive | v1 | M0 | `~/.local/state/sboard/state.toml`, atomic write |
| 4 | Config file: project roots, scan window, agent toggles | v1 | M0 | `~/.config/sboard/config.toml`, every key optional |
| 5 | CI: `cargo fmt --check`, `clippy -D warnings`, `cargo test` on Linux and macOS | v1 | M0 | Required check on every PR |
| 6 | `sboard version`, `sboard doctor` skeleton | v1 | M0 | Doctor lists stores found |
| 7 | Claude Code source: `~/.claude/projects/*/*.jsonl` | v1 | M1 | Skip sidechains, `api-request*` and `attachment` lines |
| 8 | Codex source: `~/.codex/sessions/**/rollout-*.jsonl` + `session_index.jsonl` | v1 | M1 | Keep `thread_source: user` only; subagent and guardian threads are dropped |
| 9 | Status derivation from transcript tail: working, waiting, idle, error, done | v1 | M1 | Structural first, recency second; see §6 |
| 10 | `sboard list [--json] [--project <path>] [--status <s>]` | v1 | M1 | Same tree the TUI renders |
| 11 | TUI: project tree, session rows, detail pane with last user and assistant text | v1 | M2 | Ratatui; `j k h l Enter q ?` |
| 12 | Live refresh: `notify` watcher on the store roots, 2 s debounce, manual `r` | v1 | M2 | No daemon |
| 13 | Jump: find the tmux pane whose process and cwd match, switch to it | v1 | M3 | `tmux list-panes -a -F` and `switch-client` / `select-window` |
| 14 | Resume: no pane found, run `claude --resume <id>` or `codex resume <id>` in the session cwd | v1 | M3 | Suspends the TUI, returns on exit |
| 15 | Launch: `n` new session in project, `w` new session in a fresh worktree | v1 | M3 | `git worktree add ../<repo>-wt/<branch>` via system git |
| 16 | Grouping by project (auto) and by tag; pin; sort by activity or attention | v1 | M4 | Tags and pins live in the state file |
| 17 | Read and unread; archive project or session; `a` toggles archived view | v1 | M4 | Unread is set when a session changes while not selected |
| 18 | Attention queue: top pane listing waiting, done-unread and error sessions | v1 | M5 | Same ordering rule in `list --status attention` |
| 19 | Desktop notification on status change to waiting, done or error (optional) | v1 | M5 | `notify-send` on Linux, `osascript` on macOS; off by default |
| 20 | Grok source: `~/.grok/sessions/session_search.sqlite` `session_docs` | v1 | M6 | Header WAL + WAL present: mode=ro; header WAL + no WAL: checked immutable; rollback header: mode=ro locks; SHM allowed beside WAL, never create WAL/journal; regular entries only |
| 21 | OpenCode source: `~/.local/share/opencode/opencode.db` | v1 | M6 | Fixture-only until a live install exists |
| 22 | `v0.1.0`: Linux x86_64 and aarch64, macOS aarch64 binaries, checksums, install notes | v1 | M7 | Tag-triggered release workflow; Homebrew tap later |
| 23 | Token and cost rollup per project and day from transcripts | Later | | v0.2; SQLite index, microdollar ledger, `costs sync` |
| 24 | Worktree list, cleanup, "session uses worktree X" | Later | | v0.2 |
| 25 | Fuzzy search across titles and prompts; preview without leaving | Later | | v0.2 |
| 26 | GitHub overlay: open issues and PRs per project via `gh` | Later | | v0.2 |
| 27 | Scheduled routines, MCP query surface, digest, read-only web view | Later | | v0.3 |
| 28 | Gemini CLI, Kiro and other sources | Later | | When Keith uses one |
| 29 | Full editor, persona builder, chat surface, sub-agent trees | Drop | | Ködade's job |
| 30 | Writing hooks into agent settings, owning agent processes, Docker sandbox | Drop | | ADR-0001 |

## 5. Architecture

```
src/main.rs          clap entry: tui (default), list, doctor, version
src/model.rs         Session, Project, Status, Agent; pure data
src/source/mod.rs    Source trait: fn scan(&self, cfg) -> Vec<Session>; registry
src/source/claude.rs claude_code JSONL reader
src/source/codex.rs  codex rollout reader
src/source/grok.rs   grok sqlite reader            (M6)
src/source/opencode.rs                             (M6)
src/status.rs        transcript tail -> Status; the recency rules
src/state.rs         pins, tags, read marks, archive; atomic TOML
src/config.rs        config.toml with defaults
src/scan.rs          run all sources, group into projects, apply state (no I/O; roots come in)
src/project.rs       project root per cwd via `git rev-parse --show-toplevel`, cached
src/watch.rs         notify watcher -> debounced rescan
src/tmux.rs          pane discovery, switch, new-window
src/launch.rs        resume and launch commands, worktree creation
src/ui/              ratatui app: tree, detail, attention pane, keymap, help
src/cli.rs           list and doctor output, json and text
fixtures/<agent>/    checked-in transcripts and the golden list.json
docs/PLAN.md         this file
docs/sources/<agent>.md  one page per source: paths, line types, status rules
```

Rules that hold the shape:

- `model`, `status`, `state`, `scan` and `ui` never touch the filesystem outside the state and
  config files. Sources read agent stores; `project` runs `git rev-parse` to find roots;
  `tmux` and `launch` run processes. Nothing else does.
- Sources are read-only and tolerant: a malformed line is skipped and counted, never fatal.
  `doctor` reports the counts. A missing store is a skipped source, not an error.
- JSONL sources keep a 256 KiB head and a nominal 64 KiB tail. Include the record
  crossing the tail boundary. If the tail contains only ignored or invalid records,
  extend backwards in 64 KiB steps until source-specific meaningful activity is
  found, the head is reached, or the hard 1 MiB total tail cap is reached. Alignment
  shares that cap; no omitted middle is read in full. At the cap, keep the existing
  source fallback status, count one skipped line and give doctor a path-specific
  `1 MiB tail cap` warning. Seam fragments are never parsed as malformed records.
- Status is derived from the transcript, not from screen scraping. The tmux pane, when found,
  is only used to jump. If the two disagree, the transcript wins.
- sboard never writes inside `~/.claude`, `~/.codex`, `~/.grok` or any agent's directory, and
  never installs hooks (ADR-0001).
- One process, no daemon. The watcher lives inside the TUI; `list` does a cold scan.
- Every source has a one-page `docs/sources/<agent>.md` that names the paths, the line types
  read, the ones ignored, and the status rules, with a fixture that exercises each rule.

## 6. Data model and status rules

```
Agent     claude_code | codex | grok | opencode
Session   id, agent, cwd, project_root (git toplevel of cwd or cwd), title,
          started_at, last_activity_at, status, last_user_text, last_assistant_text,
          transcript_path, resume_hint (the command that resumes it), pane (optional)
Project   root, name (basename), sessions, pinned, tags, archived, unread_count
Status    working | waiting | idle | error | done
```

Status from the transcript tail, first match wins:

1. Last meaningful line is an assistant turn that ends in a tool call with no result yet, or a
   `task_started` without `task_complete` (Codex), and `last_activity_at` is under 30 minutes
   old: **working**.
2. Same shape but older than 30 minutes: **error** (an agent that stopped mid-tool is stuck or
   died). `doctor` names the reason.
3. Last line is an assistant text turn with `stop_reason: end_turn` (Claude) or a
   `task_complete` (Codex) whose text ends in a question or an ask for a decision: **waiting**.
   Heuristic: trailing `?`, or the agent's own plan or question markers. Documented per source.
4. Otherwise an ended assistant turn: **done** if under 60 minutes old, else **idle**.
5. An explicit error record in the tail (API error, non-zero exit event): **error**.

Claude local-command user records (`<command-name>`, `<command-message>`,
`<local-command-stdout>` wrappers) never open a pending assistant turn. `/exit`
clears turn/error evidence and makes the session **idle**. Only complete native
envelopes count: trailing prose and tool_result records remain normal user activity,
and stdout contents are opaque. Other local commands
leave the previous turn and its activity clock unchanged. Command-only sessions
are idle. Commands supply neither the title nor last user text; absent a usable
session title or real prompt, use the native session id.

Unread is state, not status: set when a session's `last_activity_at` advances while it is not
the selected row, cleared when selected or marked. Attention = waiting, error, or done-and-unread.
Without a read mark, a session is unread only when its activity is newer than `first_seen`, which is written on the first state save; before that, all sessions are read.

State file (`~/.local/state/sboard/state.toml`): `pins = [root]`, `tags = { root = [tag] }`,
`archived = [root or session id]`, `read = { session_id = last_activity_at }`, `first_seen = <unix seconds>`,
`sort = "activity" | "attention"`, `group = "project" | "tag"`, `show_archived = false`. The last three
preferences apply to the TUI; list defaults to activity and project. Written with a
temp file and rename. Config file (`~/.config/sboard/config.toml`): `roots = []` (extra project
roots to include even with no sessions), `window_days = 7`, `agents = { claude_code = true, ... }`,
`notify = false`, `tmux.socket = ""`.

Archived projects and sessions do not emit desktop notifications; read marks do not suppress transition notifications.
Notification transitions compare observed status/activity pairs, without a scan-time or
activity watermark. First-seen sessions have a five-minute age window against the current
clock. Retain the last 32 announced raw pairs per session and reject activity below the
oldest retained announcement floor. Floor timestamps are capped at announcement time; a
backward clock step starts a new floor interval while raw-pair deduplication is retained.

Desktop notifications are per sboard process: two instances may each announce the same
transition. A notifier that deliberately daemonizes with `setsid()` and forks again leaves
the command process group and is outside the five-second process-group timeout.

## 7. CLI contract

```
sboard                         # TUI
sboard list [--json] [--project <path>] [--status working|waiting|idle|error|done|attention]
            [--agent <a>] [--sort activity|attention] [--group project|tag] [--all]           # --all includes archived and beyond window_days
sboard doctor [--json]         # stores found, sessions per source, skipped lines, tmux, git
sboard open <session-id>       # jump or resume, same as Enter
sboard new <project-path> [--agent <a>] [--worktree <branch>]
sboard version
```

Opening a session with a discovered live pane attempts only a tmux jump. A failed
jump reports the tmux error and never resumes a second copy. Resume is available
when no pane is known. `open` prints one outcome line (`jumped to <target>` or
`resumed <id> in <dir>`); an `error: <reason>` line goes to stderr with exit 1.
The TUI retains an open error in its status bar until the next key.

During a terminal handoff the child owns a separate foreground process group.
SIGHUP or terminal hangup forwards HUP to that group, allows 500 ms for cleanup,
then kills remaining group members and bounds leader reaping to two seconds.
sboard exits 129 without re-entering or restoring the terminal. Ctrl-C goes to
the foreground child; normal child exit returns terminal ownership to sboard.

TUI keys: `j k` move, `h l` fold, `Enter` open, `n` new, `w` new in worktree, `p` pin, `t` tag,
`m` mark read, `a` archive toggle, `A` show archived, `s` cycle sort, `g` group by tag/project, `/` filter, `r` rescan,
`?` help, `q` quit. Exit codes: 0 ok, 1 usage, 2 no stores found.

## 8. Milestones

One milestone = one GitHub issue, one branch, one draft PR, one T3 worker thread in its own
worktree (Codex gpt-6.1-sol, medium). M0 lands alone; M1 and M2 are sequential; M3, M4, M5 and
M6 may run in parallel after M2 and merge in that order; M7 last.

| M | Delivers | Test asserts |
|---|---|---|
| M0 | model, Source trait, fixture harness, state and config files, CI, version, doctor skeleton | `list --json` on an empty fixture dir is `[]`; state round-trips; CI green on both OSes |
| M1 | Claude Code and Codex sources, status rules, `list` | golden `list.json` from the fixtures matches; each status rule has a fixture |
| M2 | Ratatui tree, detail pane, live refresh | TUI snapshot test of the tree on fixtures; touching a fixture triggers a rescan |
| M3 | jump, resume, launch, worktree | pane matcher picks the right pane from a `list-panes` fixture; `new --worktree` creates the worktree in a temp repo |
| M4 | grouping, tags, pins, sort, read marks, archive | state survives a restart in a temp state dir; sort orders are deterministic |
| M5 | attention queue, notifications | `list --status attention` ordering; notifier is called once per transition |
| M6 | Grok and OpenCode sources | fixtures for both; `doctor` reports them |
| M7 | release workflow, binaries, checksums, install notes, v0.1.0 | release job produces three artifacts and `sboard version` prints the package version |

## 9. Testing

- **Unit** (`cargo test`): sources against `fixtures/`, status rules one fixture each, state and
  config round-trips in a temp dir, tmux matcher against captured `list-panes` output, TUI
  rendering with Ratatui's `TestBackend`. No live agent, tmux or network.
- **Fixtures** are real transcripts with content replaced by short placeholder text. Each source
  page in `docs/sources/` lists which fixture covers which rule.
- **CI**: GitHub Actions on `ubuntu-latest` and `macos-latest`: fmt, clippy with `-D warnings`,
  test. A PR needs a completed green check before it leaves draft, then a `skeptic-review`
  round in its own worktree. Skeptic findings become `skeptic` issues.
- **Live checks** are the skeptic's: run `sboard` on Omarchy and the Mac Studio against real
  stores and real tmux, and try to break it.

## 10. Acceptance (v1)

On Omarchy and on the Mac Studio, each with live Claude Code and Codex stores:

1. `sboard` opens in under 1 s with the last 7 days of sessions grouped by project.
2. A Claude session that is mid-tool shows **working**; one that just asked a question shows
   **waiting**; one that ended an hour ago shows **idle**.
3. Enter on a session running in a tmux pane lands in that pane. Enter on one with no pane
   starts the resume command in its directory and returns to sboard on exit.
4. `w` creates a worktree and starts a session in it; the new session appears under the
   worktree path within 5 s.
5. Pin, tag, read and archive survive quitting and reopening.
6. `sboard list --json` is valid JSON with the same sessions and statuses as the screen.
7. `sboard doctor` names every store found and every source skipped, with counts.
8. The release page has three binaries with checksums and `sboard version` prints `sboard 0.1.0`.

## 11. Open questions

- "Waiting" for Claude Code: `end_turn` with a trailing question is a heuristic. If it misfires
  in practice, the fallback is "any ended turn is waiting until read", which makes waiting and
  done-unread the same thing. Decide after M1 on real data.
- Codex `session_index.jsonl` gives titles; Claude's `ai-title` line does the same but is not
  always present. Fallback is the first user prompt, cut to 60 chars.
- macOS paths: Claude and Codex use the same `~/.claude` and `~/.codex` on macOS. OpenCode's
  database location on macOS is unverified until M6.
