# Terminal input and UI layout

The board reads raw bytes and assembles key events itself, because crossterm's decoder blocks
on pastes and buffers bytes that belong to a child after handoff.

## Input (`src/ui/input.rs`, `src/ui/organize.rs`)
- Bytes are assembled before dispatch; UI work cannot change input timing. The reader only does
  bounded raw reads from a verified TTY, never a blocking paste decode.
- `ESCAPE_GAP` is 75 ms: a little scheduling slack around the terminal's 50 ms escape grace.
- A held Escape keeps its byte time so the UI's terminator grace does not delay it twice.
- After `ESC [`, wait to tell a bare pair from Escape followed by a sequence.
- Linux console F1–F5 carry an extra `[` before the final byte.
- Prompts are capped separately; an unterminated paste is bounded at 1 MiB too.
- A partial terminator contains only Escape, `[`, digits and `~`, never action keys. Escape may
  close a prompt, so the paste quarantine is rechecked afterwards.

## Layout
- The detail pane uses separate regions so long prose cannot hide the resume hint.
- Help scrolls on short terminals instead of clipping (#18).
- Footer errors put the reason first so a long state path cannot push it off (#116).
- Read marks can shrink the attention queue; an open keeps its original target.
