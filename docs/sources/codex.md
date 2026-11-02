# Codex source

Store: `${CODEX_HOME:-~/.codex}/sessions/**/rollout-*.jsonl`; title index:
`${CODEX_HOME:-~/.codex}/session_index.jsonl`. Empty overrides fall back to HOME.
Everything is opened read-only; directory symlinks below the store are not traversed.

Only the **first session_meta** supplies identity, provenance and cwd. Keep explicit
`thread_source: "user"`, or absent thread_source with a string source (cli, vscode,
exec). Reject object sources and every explicit non-user thread_source. Later metas
are ignored, including a parent's meta embedded in a subagent rollout. App-server
sessions omit thread_source; this is current live data, not an older format to drop.
Native IDs are cleaned like titles and skipped/countable when the result is empty;
index ID lookup uses the same cleanup. Resume hints use the cleaned native ID.
Turn_context cwd changes never move the session. Empty/relative first cwds skip with
a count. IDs are `codex:<native-id>`; resume hints use the native ID. Project identity
uses shared path normalization; worktrees remain separate projects.

Read event_msg task_started, task_complete, turn_aborted, error, user_message and
item_completed; response_item user/assistant message and tool call/output records.
Ignore developer messages, reasoning, token_count, token_usage_record,
thread_settings_applied, world_state, additional_tools and unknown types.
Tool output exit codes are not API errors.

Titles prefer a nonempty cleaned session_index thread_name (last index entry wins), then the first
user_message or item_completed UserMessage, then the first user response_item whose
text does not start with `<` or `#`. Last user text uses the same precedence with
the latest eligible text. Fallback titles are cut to 60 Unicode characters. Titles
collapse whitespace and remove controls and escape commands before reaching text
output; last user/assistant text retains transcript prose for the future detail pane.

Read 256 KiB of head and a nominal 64 KiB tail, aligned to complete records,
including the record crossing the nominal boundary. When the tail has no meaningful
activity, extend backwards in 64 KiB steps until activity is found or the head is
reached. Alignment and recovery share a hard 1 MiB total tail cap. At the cap,
retain the source's fallback status and report one skipped line plus a path-specific
`1 MiB tail cap` warning in doctor. Never parse the left seam fragment. Oversized
unrecoverable transcripts beyond the old 64 MiB fallback threshold still skip
the session with a reason, without reading that middle.
Sampled malformed/truncated/invalid-UTF-8 records increment skipped lines; seam
fragments do not. Missing/unreadable stores increment skipped stores. Empty files,
invalid cwds, excluded provenance and unreadable transcripts increment skipped
sessions. Corruption in an omitted middle is not counted. Malformed title-index
records increment skipped lines. The optional index must be a regular file, checked
with symlink_metadata before open; FIFOs, symlinks and other nonregular paths produce
a skipped-store count and a path-specific warning. Missing index is optional.
Doctor scans all ages; list applies its window.

A head task_started survives the gap until a tail task event overrides it. Pending
tasks or unanswered calls mean working below 1800 seconds, error at/after 1800.
Task_complete clears calls and supplies ended text. Turn_aborted clears calls and
is idle, or an ended turn when last_agent_message is present. A completed turn
ending in `?` or containing `<question>` / `<request_user_input>` is waiting; other
ended turns are done below 3600 seconds, else idle. An event_msg error is an API
error; the next task_started clears it. Error events terminate the pending task.
The first-match derivation and thresholds live in `src/status.rs`. Stale-call
reasons appear only in doctor, never as list warnings.

Base fixtures under `fixtures/codex/sessions/2026/11/01/`, fixed clock 12:00 UTC:

| Fixture | Rule / behavior |
|---|---|
| rollout-working.jsonl | 1: pending task, 1 minute old |
| rollout-stuck.jsonl | 2: pending task, 40 minutes old; doctor reason |
| rollout-waiting.jsonl | 3: completed question |
| rollout-done.jsonl | 4: recent completion, indexed title |
| rollout-idle.jsonl | 4: completion 61 minutes old |
| rollout-error.jsonl | 5: event_msg API error |
| rollout-malformed.jsonl | Truncated final JSON after valid records |
| rollout-empty.jsonl | Zero-byte transcript |
| rollout-guardian.jsonl | Explicit guardian excluded |
| rollout-subagent.jsonl | Subagent object excluded |

`fixtures/codex/session_index.jsonl` covers lookup; `fixtures/list.json` is the
combined fixed-clock golden. `tests/round2.rs` creates bounded temporary fixtures:
issue21 covers absent provenance/string and object sources and embedded metas;
issue22 covers a 70 KB first meta, an omitted middle, pending/complete/aborted task
state at 20 seconds and 40 minutes; issue25 covers API error, recovery and nonzero
tool exits; issue27 covers aborts with/without text; issue28 covers event/index
priority, injected instructions, the 256 KiB head and sanitized titles; issue32
covers cwd changes. The same tests exercise the public Source interface.

Round 2 live review confirmed that strict thread_source filtering hid this week's
sessions and resetting task state made active/stuck tasks idle. Those bugs are
fixed. The 30-minute threshold is the agreed calibration for long tools/subagents;
transcripts still cannot prove process liveness. The waiting heuristic remains as
specified, with no prior-turn text reused for completion.

Round 3 tests in `tests/round2.rs` also cover printable/empty native IDs, signed
numeric timestamp fields, a FIFO index (both list and doctor complete within the
test deadline), FIFO config/state paths, and control-only index titles falling
through to actual owner text. Numeric timestamp fields accept ASCII digits only.

Retained last user and assistant text is capped at the latest 8 KiB each,
on a UTF-8 boundary. Status derivation still sees the full sampled text.

Issue #118 fixtures in `fixtures/tail-sampling/` and `tests/tail_sampling.rs` cover
completed turns hidden behind large ignored records, small transcripts, recovery
to the head and the 1 MiB cap. The ignored release performance test scans 10,000
fixture-shaped Claude sessions through the public Source seam.
