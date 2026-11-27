# Config, state files and git discovery

sboard's own files are often dotfiles-managed symlinks, and it runs inside shells, hooks and
editors that export git variables. These rules keep both safe.

## Environment
- An exported-but-empty override or `HOME` counts as unset (#12), for doctor too.
- Errors are prefixed with the file they concern so two override directories stay tellable
  apart (#15).
- Config: a missing file gives defaults, unknown keys give warnings, and a bad value or an
  unreadable file is an error that names the file.

## Files (`src/storage.rs`, `src/state.rs`)
- Reject FIFOs, devices and directories before opening optional text paths. Symlinks are
  followed on purpose (#13).
- Writes persist beside the target so rename stays on one filesystem. A symlinked target is
  written through and its mode kept. A dangling final symlink is resolved without replacing the
  link.
- State loading is lenient; saving re-reads strictly before replacing anything, reads fully
  before creating a backup, and applies only the edits made since this instance's last
  successful load or save.
- Dangling links in state keys resolve too, so their spelling keeps the target's identity.
- `PathBuf` treats `/p` and `/p/` as equal; tag keys are unioned raw before that comparison can
  lose data.

## git (`src/project.rs`, `src/launch.rs`)
- Inherited git environment (hooks, `rebase --exec`, editor terminals) would make `rev-parse`
  answer for the wrong repository (#14); it is cleared for discovery and for launched agents
  (#59).
- Root lookups cache failures too: old sessions point at directories that no longer exist.
- `rev-parse` output goes to a file to avoid pipe-buffer deadlocks with very long paths. Strip
  exactly one line terminator; whitespace and newlines may be part of a path.
- Paths share one identity for existing symlinks and missing paths. Sources reject relative
  session cwds; config paths may expand a leading home component.
- Worktree creation: successful branch creation, not the earlier lookup, establishes ownership.
  On failure only what this attempt created is removed; pre-existing branches stay. Checkouts
  are not bounded by the metadata timeout.
