---
name: delete-comment
description: "Sort every code comment in session-board into tautology, stale, excuse or decision record; delete the first two, file an issue per excuse, move decision records into ADRs, and tighten the comment ratchet. Use for /delete-comment, 'purge comments', or when tests/comments.rs fails."
---

# delete-comment — session-board

The rule and its enforcement are in `docs/adr/0010-no-code-comments.md`. Read it first.

## 1. Find the comments

```bash
cargo test --locked --test comments                 # fails listing file:line for each new comment
```

The test lists every comment line over the baseline. For a full inventory, temporarily empty
`tests/comment-baseline.txt` and run it again. Do not commit that empty file unless you clear
every listed line. `// SAFETY:` blocks and line-1 shebangs are exempt and never listed.

## 2. Sort each comment (read the code around it first)

| Kind | Test | Action |
|---|---|---|
| Tautology | The code, a name, a test name or an existing doc already says it | Delete |
| Stale | It names a milestone, a removed function or a superseded behavior | Delete |
| Excuse | It justifies a shortcut: a magic number, hand-rolled code where a dependency already exists, a sleep, a retry, a "preserve the old" | Delete and file an issue |
| Decision record | A non-obvious why: platform behavior, a race, a security boundary, an issue-numbered rule | Move into an ADR, then delete |

When a comment is both, the stronger kind wins: excuse over decision over tautology.

## 3. Act

- **Excuse:** `gh issue create --label refactor` with title `refactor: <what to fix>`, body
  naming the file and function (not a line number), the deleted comment verbatim, and the fix.
- **Decision record:** add a bullet to the ADR in `docs/adr/` that owns the area (0003 process
  and terminal, 0004 SQLite, 0005 transcripts and status, 0006 files, state and git, 0007 input
  and UI, 0008 watcher and notifications, 0009 tests). Name the file and function, never a line.
  A new area gets the next ADR number in the existing short-prose style.
- **Clap doc comments** are help text: convert them to `#[command(about = "...")]` without
  the trailing period and diff `sboard --help` before and after.
- Delete with care: removing a doc comment must not change `--help`, and removing a SQL fixture
  comment must not break the fixture.

## 4. Tighten and check

```bash
UPDATE_COMMENT_BASELINE=1 cargo test --test comments   # lowers the ratchet; refuses any increase
cargo fmt --all --check
cargo clippy --locked --all-targets -- -D warnings
cargo test --locked
```

Commit as `refactor: delete comments in <scope>` with the ADR edits and the lower baseline in
the same commit. The PR body lists issues filed per excuse.
