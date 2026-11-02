# Claude Code source

Store: `${CLAUDE_CONFIG_DIR:-~/.claude}/projects/*/*.jsonl`. The override names the
config home; empty falls back to HOME. Files are read-only, and nested subagent
paths are not traversed.

Read user, assistant and ai-title (aiTitle, with title compatibility). Ignore
sidechains, isMeta lines and all other types, including attachment, api-request*,
queue-operation, atis-latch, last-prompt, pr-link, cost-state, mode,
permission-mode and file-history records. Text is a string or text blocks;
thinking/tool blocks are not prose. User text starting with `<` does not become
the title or last user text. First eligible prompt is the 60-character fallback
title. All titles collapse whitespace and strip controls/escape commands into one
printable line. Display detail text retains the original eligible prose.

The first message cwd is the launch directory; later changes are ignored. Empty
or relative cwds skip the session with a count. Shared normalization canonicalizes
existing paths and resolves `..`/separators otherwise; project resolution maps
cwds inside `.git` to the project. IDs are `claude_code:<native-id>`; native IDs use the same printable-line cleanup
as titles, and an empty cleaned ID skips/counts the session. Resume hints use the
cleaned native ID, avoiding terminal injection and cross-agent read-mark collisions.

Read 256 KiB of head and 64 KiB of tail. Complete head lines supply metadata and
first prompt; tail records supply current activity/status. A tail starting on a
line boundary retains its first line. Otherwise scan backwards in 64 KiB steps to
the preceding newline (or head end), including the crossing record even if complete
ignored records follow it. Scan/read size is capped at 64 MiB. If no complete tail
record is available, retain the bounded head-end-to-EOF fallback. Beyond the cap,
skip and count the session with a reason. Sampled malformed/truncated/invalid-UTF-8
records count as skipped lines; seam fragments do not. Empty/unreadable or invalid
sessions increment skipped sessions. Missing/unreadable stores increment skipped
stores. Omitted middle records are not validated. Doctor scans all ages and gives
stale-call reasons; list applies its configured window and never emits those reasons.

PLAN §6 first-match rules, thresholds in `src/status.rs`:

- Tool_use blocks or tool_use stop reasons indicate pending generation. A last user
  prompt or tool_result also means generating. Under 1800 seconds is working;
  at/after 1800 is error. A user record **starting with** `[Request interrupted by user`
  is idle; quoting that marker later in a prompt does not interrupt it.
- end_turn, max_tokens and stop_sequence are ended turns. Only this turn's text is
  tested for trailing `?` or `<question>` / `<request_user_input>` markers: waiting.
  Empty/thinking-only text never reuses an earlier question. Otherwise done below
  3600 seconds, idle at/after 3600 seconds.
- An API error on the last meaningful record (isApiErrorMessage or non-null error)
  supplies error evidence. Recompute it per record; tool_result is_error is not an
  API error and cannot remain sticky across later activity.

Base fixtures under `fixtures/claude_code/project/`, fixed clock 12:00 UTC:

| Fixture | Rule / behavior |
|---|---|
| working.jsonl | 1: pending tool, 1 minute old |
| stuck.jsonl | 2: pending tool, 40 minutes old; doctor reason |
| waiting.jsonl | 3: ended question |
| done.jsonl | 4: recent completion; aiTitle, ignored sidechain/queue line |
| idle.jsonl | 4: completion 61 minutes old |
| error.jsonl | 5: last API error |
| malformed.jsonl | Valid prompt then truncated JSON; prompt is working |
| empty.jsonl | Zero-byte transcript |

`tests/round2.rs` creates temporary regression fixtures through the Source seam:
issue23 covers large final records (including an 8-day-old head), aligned tail
starts, 64 KiB/128 KiB ±1 records and the 64 MiB fallback cap; issue24 covers prompt,
tool result and interruption tails; issue25 covers tool failure/recovery and last
API errors; issue28 covers injected metadata/control characters; issue29 covers an
answered question followed by thinking-only completion; issue32 covers cwd changes.
Additional tests cover ended stop reasons and the 30-minute threshold.

Round 2 live sampling found mid-turn idle, sticky tool failures and stale question
reuse. These were bugs in transcript interpretation and are fixed. Long calls
observed on both machines motivated the agreed threshold increase to 30 minutes.
Question punctuation is still a heuristic; no pane/process inference is used.

Round 3 regressions in `tests/round2.rs` cover a 100 KB fresh prompt crossing the
nominal tail before an ignored attachment, printable/empty IDs, signed timestamp
fields (skipped lines), file-based project filters and quoted interruption prose.
Removing the backward scan fails the straddling-record test. Timestamp numeric
fields must contain ASCII digits, including year/month/day and offset fields.

Retained last user and assistant text is capped at the latest 8 KiB each,
on a UTF-8 boundary. Status derivation still sees the full sampled text.
