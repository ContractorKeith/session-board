# Observe the agents, never own them

sboard reads the session files the agent CLIs already write and derives status from the shape
of the transcript. It does not install hooks into Claude or Codex settings, does not wrap the
agent process, and does not require sessions to be started through it or inside a private tmux
server. Chosen over agent-deck's model (hooks plus owned tmux sessions) because hooks break on
agent upgrades, owned sessions exclude T3 threads and plain terminals, and the whole point is a
companion that sits beside Ködade or a bare shell. The cost is that "waiting" is a heuristic on
the transcript tail rather than a signal the agent sends, and jump only works when a tmux pane
can be matched by process and directory. Reversing this later means adding an optional hook as a
second status input, not replacing the readers.
