# Watcher, background scans and notifications

The TUI watches stores and rescans in the background. No terminal work runs on these threads.

## Watching (`src/watch.rs`)
- Debounce is a fixed window from the first event, so a streaming transcript cannot starve it.
- A failed attachment never discards the watches that succeeded.
- Watched roots are canonicalized because notify reports canonical paths (`/private/var` on
  macOS). FSEvents may deliver a coalesced batch after the first window.
- Watch status is its own channel, so recovery updates the bar without a scan. File and manual
  requests reset the quiet-store refresh deadline.

## Scans (`src/scan.rs`, `src/cli/json.rs`)
- A scan carries the archive entries in force when it ran; queued scans keep their own mute
  (#116).
- Tag groups keep each session's real `project_root` for organizing and launch.
- CLI JSON uses typed projections so domain records stay free of output-only fields.

## Notifications (`src/notify.rs`)
- A bounded queue keeps desktop processes off the event-loop thread.
- The osascript script is constant; transcript text is passed only as argv.
- Sessions archived either now or when a queued scan ran are muted (#116).
- Raw status pairs deduplicate across clock steps; floors belong to one clock interval.
- Missing stores and rolled-back transcripts never erase known activity.
