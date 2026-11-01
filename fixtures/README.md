# Fixtures

M0 has no real sources or transcript status rules. The per-agent directories are
empty placeholders; `list.json` is the golden output of an empty registry.
M1 will add sanitized real transcripts for Claude Code and Codex and expand the
golden file. Never copy raw session text or credentials into this directory.

`tests/harness.rs` supplies fake sources to exercise grouping and state without
reading any live agent store. CLI tests set both SBOARD directory overrides to
temporary directories; the production registry is empty until M1.
