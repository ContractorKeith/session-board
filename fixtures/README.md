# Fixtures

Claude Code and Codex record envelopes reduced from the live stores, with prompt
and assistant prose, IDs, dates, tool input/output and paths replaced by short
placeholders. Status/age/error variants replay those shapes at deterministic times.
No raw prompt text, credentials, emails or paths outside `/home/user/...` belong here.

`docs/sources/claude-code.md` and `docs/sources/codex.md` map every transcript to its
rule. Each source includes a truncated last line and a tracked zero-byte file.
The Codex title index and excluded guardian/subagent sessions are fixtures too.

`tests/sources.rs` injects 2026-11-01 12:00 UTC into the sources and runs the same
scan, filtering and list JSON writer as the CLI. `list.json` is the combined golden
with relative transcript paths, independent of the checkout and current clock.
Tests also use temporary stores for malformed bytes, normalization, tool results,
large-file tail reads and CLI integration. Every CLI subprocess overrides both
sboard directories and both agent homes; tests never read the owner's stores.
