# Transcript tails and status evidence

Status is derived from what a transcript says, never from pane scraping (PLAN §6). Transcripts
can be hundreds of megabytes, so sboard reads a head and a tail and must not mistake what it did
not read for evidence.

## Reading (`src/source/read.rs`)
- `records` keeps the head and recovers a meaningful tail without reading the omitted middle.
  Alignment and recovery share a hard 1 MiB tail budget. The record crossing the nominal 64 KiB
  window is resolved before deciding to stop.
- `files` never follows directory symlinks while walking a store (cycles and subagent trees).
- `preview` keeps the most recent 8 KiB on a UTF-8 boundary; status still uses the full text.
- `title` keeps titles to one printable line: strip CSI commands and OSC wrappers, keep OSC
  payload text after the numeric command, never execute it.
- `timestamp` reads RFC3339 with UTC or numeric offsets; fractional seconds are ignored.
- Times are Unix seconds in UTC. Status is derived by a source and never persisted in state.
- Window filtering follows transcript activity, not file mtime.

## Rules per source
- Claude: a tool result or any non-text block is never a native command record. Local commands
  supply metadata but do not refresh the previous turn's clock. The omitted middle may contain
  completions, so head status is not tail evidence.
- Codex: task lifecycle survives omitted history; tail task events override it.
- Grok: in the flat session index a question cannot imply waiting.
- OpenCode: an open assistant message is still generating until its completion marker.
- Environment overrides (`CLAUDE_CONFIG_DIR` and friends) point at the agent config homes, not
  their stores.

## Duplicates
- A copied transcript repeats its session id. `open` picks the most recent copy and says so
  (#59).
