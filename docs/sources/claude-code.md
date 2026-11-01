# Claude Code source

Store: `${CLAUDE_CONFIG_DIR:-~/.claude}/projects/*/*.jsonl`. Overrides name the
config home, not `projects`; empty overrides fall back to HOME. Opened read-only.
Only this level is scanned: nested subagent directories are not traversed.

Read `user`, `assistant`, `ai-title` (`aiTitle`, with `title` compatibility).
Ignore every `isSidechain: true` record and all other types, including attachment,
api-request*, queue-operation, atis-latch, last-prompt, pr-link, cost-state,
mode, permission-mode and file-history records. Unknown types are ignored.
Text may be a string or text blocks; thinking and tool payloads are not prose.
The first user text, cut to 60 Unicode characters, is the fallback title.

Read at most 64 KiB of head and 64 KiB of tail, with seek and without parsing the
omitted middle. Head supplies identity, first prompt and start time; tail updates
cwd, title, activity and status. Boundary fragments are discarded, not counted as
malformed lines. A complete malformed/invalid-UTF-8 record or truncated final
record in the sampled regions increments skipped lines. An empty, unreadable or
metadata-less transcript increments skipped sessions. Counts describe sampled
regions, not a whole-file validation. A missing/unreadable store increments
skipped stores. Doctor scans all ages and includes stale-tool reasons.

Session IDs are `claude_code:<native-id>`, preventing read-mark collisions with
other agents. Resume hints retain the native ID. Empty/relative cwds skip the
session; absolute cwds share project normalization (canonicalize existing paths,
otherwise resolve `..` and trailing separators). Git roots are resolved outside
the source; a cwd inside `.git` resolves to the containing project.

Status follows PLAN §6 in order. Pending tool_use blocks (matched to tool_result
IDs), or a `tool_use` stop reason without a result, are working below 600 seconds
and error at/after 600 seconds. An ended assistant turn is waiting when its text
ends in `?` or contains `<question>` / `<request_user_input>` markers. Other
ended turns are done below 3600 seconds and idle at/after 3600 seconds. Explicit
`isApiErrorMessage`, non-null error, or tool_result `is_error` records yield error
only after those higher-priority rules. Unended, nonpending tails default to idle.
Threshold constants live only in `src/status.rs`.

Fixtures are reduced transcript record envelopes from the live store, with text,
IDs, dates and paths replaced; status/age/error variants are deterministic
replays. Tests fix time at 2026-11-01 12:00 UTC. Under `fixtures/claude_code/project/`:

| Fixture | Rule / behavior |
|---|---|
| working.jsonl | 1: pending tool, 1 minute old |
| stuck.jsonl | 2: pending tool, 11 minutes old; doctor reason |
| waiting.jsonl | 3: end_turn with a trailing question |
| done.jsonl | 4: recent end_turn; aiTitle; sidechain and queue line ignored |
| idle.jsonl | 4: end_turn 61 minutes old |
| error.jsonl | 5: explicit API error |
| malformed.jsonl | Valid user followed by a truncated final JSON record |
| empty.jsonl | Zero-byte transcript, counted as skipped session |

Live inspection (Omarchy, 2026-11-01) confirmed tool_use/end_turn stop reasons,
including separate thinking blocks. The question heuristic is implemented as
written; code-block punctuation and indirect requests can still misclassify it.
Transcript status cannot prove whether a process is alive. No change to the plan's
heuristic was made. Last nonempty assistant text is retained across thinking blocks.
