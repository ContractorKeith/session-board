# Terminal handoff and child process control

sboard hands its terminal to agents, tmux and git and must get it back intact. Every rule here
came from a skeptic round where the shell, a pipe or a process group misbehaved. They are not
visible from the code, so they live here instead of in comments.

## Process groups and signals (`src/launch.rs` `run`, `foreground`)
- Block SIGTTOU only on the calling thread while transferring terminal ownership with
  `tcsetpgrp`.
- sboard may catch SIGINT and SIGHUP while a child runs: caught signals reset to their default
  disposition across the child's exec, so the child still receives them normally.
- On hangup, give the whole child group time to handle HUP and reap its own children before
  escalating. A descendant may ignore HUP after the leader exits.
- Keep the group leader unreaped until escalation is over so its group id cannot be reused.
- `waitpid` observes stops as well as exits: the shell tracks sboard's job group while terminal
  input goes to the separate agent group.
- SIGSTOP also works for an orphaned PTY group. A background resume must stop again instead of
  stealing the shell's terminal.

## Pipes and output
- `launch::output` bounds metadata commands without risking a full stdout or stderr pipe.
- `cli::warn`: diagnostics never prevent successful stdout when stderr is closed or full.
- `src/ui/output.rs`: stop terminal writes after hangup, including Ratatui's cursor-restoring
  destructor.

## TUI lifetime (`src/ui/mod.rs`, `src/ui/open.rs`)
- `TerminalGuard` restores the terminal on setup errors, event-loop errors and unwinding.
- After a child returns, the forwarded panic hook may have scrolled stderr through the frame:
  reset the fullscreen buffers without querying the cursor position.
- Jump failures remain errors even when the target vanished after discovery.

## tmux (`src/tmux.rs`)
- `field`: `n:` is a byte count. Consume exactly six fields and one record newline; never
  resynchronize malformed data at a newline inside an untrusted field.
- `jump`: empty display formats succeed even for missing targets, so require an actual pane id.
- `doctor`: multiple argv elements make tmux exec directly, bypassing shell rc files.
