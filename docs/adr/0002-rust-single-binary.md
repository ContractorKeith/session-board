# Rust single binary with Ratatui

sboard ships as one static binary built with cargo: Ratatui for the TUI, clap for the CLI,
`notify` for the file watcher, `rusqlite` with the bundled feature for the SQLite stores, and
the system `git` and `tmux` called as processes. Chosen over Go and Bubble Tea (agent-deck) to
diversify from punch and mdkb, and over Python and Textual because this tool stays open all day
in a side terminal and must start in well under a second on Apple Silicon and Linux. The cost is
slower iteration on UI than Textual and a stricter compile loop; the mitigation is that the UI is
thin over a pure `scan` result that is fully tested without a terminal.
