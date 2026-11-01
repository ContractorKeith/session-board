# Codex source

Store: `${CODEX_HOME:-~/.codex}/sessions/**/rollout-*.jsonl`.
Titles: `${CODEX_HOME:-~/.codex}/session_index.jsonl` (`id`, `thread_name`,
`updated_at`); last index entry for an ID wins, matching the append-only store.
Empty overrides fall back to HOME. Everything is opened read-only.

Read session_meta identity/cwd and require `thread_source == "user"`.
Drop subagent objects, subagent/guardian strings and missing provenance. Read
turn_context cwd, event_msg task_started/task_complete/item_completed UserMessage,
response_item user/assistant message text, custom_tool_call/output and
function_call/output, plus explicit event_msg error. Ignore developer messages,
reasoning, token_count, token_usage_record, thread_settings_applied, world_state,
additional_tools and unknown records. Tool output is inspected structurally for
nonzero `Process exited with code N` lines, never included as assistant text.
Titles fall back to the first user text cut to 60 Unicode characters.

At most 64 KiB of head and 64 KiB of tail are read; metadata and first prompt come
from the head, tail records update activity, cwd, text and status. Complete malformed
or invalid-UTF-8 sampled lines and a truncated final line increment skipped lines;
partial lines at seek boundaries do not. Empty/unreadable/missing-metadata files,
invalid cwds and excluded non-user sessions increment skipped sessions. Corruption
in the omitted middle is not counted. Missing/unreadable stores increment skipped
stores; malformed title-index lines increment skipped lines too. Doctor scans all
ages and reports stale-task/tool reasons. No directory symlinks are traversed.

IDs are `codex:<native-id>`; resume hints retain the native ID. Empty/relative cwds
skip the session; absolute cwds use the same normalizer as project roots, pins and
tags. Existing paths canonicalize; missing paths resolve `..` and separators.
Config paths additionally expand a leading `~`. Worktrees remain separate projects.

PLAN §6 precedence: outstanding task_started or unanswered tool calls mean working
below 600 seconds, error at/after 600 seconds. task_complete clears outstanding
calls and supplies the ended text (last_agent_message, falling back to assistant
text). A trailing `?`, `<question>` or `<request_user_input>` marker means waiting.
Other completed tasks are done below 3600 seconds, otherwise idle. Explicit error
records/nonzero exit output yield error only after those rules. Other tails are
idle. Both thresholds are named constants in `src/status.rs`. Status evidence from the
head is reset across an omitted middle: a completion could be in that gap. A long
running task with its start outside the tail may therefore appear idle between
tool calls; the bounded tail cannot establish that missing history.

Fixtures are reduced live transcript envelopes with placeholders and normalized
IDs, dates and paths; deterministic status/age/error replays use a fixed test time
of 2026-11-01 12:00 UTC. Under `fixtures/codex/sessions/2026/11/01/`:

| Fixture | Rule / behavior |
|---|---|
| rollout-working.jsonl | 1: task_started without completion, 1 minute old |
| rollout-stuck.jsonl | 2: task_started without completion, 11 minutes old |
| rollout-waiting.jsonl | 3: task_complete ends in a question |
| rollout-done.jsonl | 4: recent completion; title from session_index.jsonl |
| rollout-idle.jsonl | 4: completion 61 minutes old |
| rollout-error.jsonl | 5: nonzero tool exit |
| rollout-malformed.jsonl | Valid metadata/user followed by truncated JSON |
| rollout-empty.jsonl | Zero-byte transcript, skipped session |
| rollout-guardian.jsonl | Guardian excluded |
| rollout-subagent.jsonl | Subagent object excluded |

`fixtures/codex/session_index.jsonl` covers title lookup. The combined golden
`fixtures/list.json` uses the fixed clock and the same JSON writer as `list`.

Live inspection (Omarchy, 2026-11-01) found 44 older rollouts with `source: vscode`
and no thread_source, alongside 12 explicitly user-owned rollouts and five explicit
non-user rollouts. The older records are intentionally excluded under the strict
user-only rule; no inferred ownership fallback was added. The question heuristic
remains unverified against owner intent; there is no confirmed misclassification
from structural inspection alone. This does not establish live process liveness.
