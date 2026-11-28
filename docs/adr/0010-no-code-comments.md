# No code comments

Comments in this repo drifted the way agent-written comments do: some repeated the code, some
named milestones that had shipped, and some excused a shortcut that later code copied. In
November 2026 every comment was sorted into tautology, stale, excuse or decision record. The
first two were deleted, excuses became issues, and decision records moved into ADRs 0003–0009.

## Rule
- No comments in `.rs`, `.py` or `.sql` files under `src/`, `tests/` and `fixtures/`. Doc
  comments count. Put the reason in a name, a test, or an ADR.
- Exempt: `// SAFETY:` blocks, which `clippy::undocumented_unsafe_blocks` requires on every
  `unsafe` block, and a `#!` shebang on line 1. A SAFETY block is an own-line `// SAFETY:`
  plus the plain `//` lines directly under it; a doc comment, a block comment or a blank line
  ends it. Keep those continuation lines about the unsafe call, since the lint cannot tell.
- clap help text uses `#[command(about = "...")]`, not doc comments.

## Enforcement
- `tests/comments.rs` scans those files and fails when a file has more comment lines than
  `tests/comment-baseline.txt` allows. It also fails when a file has fewer, so the ratchet only
  tightens: run `UPDATE_COMMENT_BASELINE=1 cargo test --test comments` and commit the new
  baseline. Update mode still fails on any increase and leaves the baseline alone.
- The baseline is empty after the first pass. A branch that adds comments, including one opened
  before this rule, fails CI until it moves them out. Raising the baseline is a decision for an
  ADR, not a fix.
- The `/delete-comment` skill (`.agents/skills/delete-comment/`) repeats the sort-and-move pass.
