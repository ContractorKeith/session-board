use rusqlite::Connection;
use session_board::{
    config::Config,
    source::{Source, opencode::Opencode},
};
const NOW: i64 = 1793534400;

#[test]
fn issue113_opencode_future_timestamps_clamp_once_per_session() {
    let temp = tempfile::tempdir().unwrap();
    let path = temp.path().join("opencode.db");
    let writer = Connection::open(&path).unwrap();
    writer
        .execute_batch(include_str!("../fixtures/opencode/opencode.sql"))
        .unwrap();
    writer
        .execute_batch(include_str!("../fixtures/opencode/future.sql"))
        .unwrap();
    for (started, updated) in [
        (2108894400, 2108894400),
        (NOW + 3600, NOW + 3600),
        (NOW - 60, NOW + 3601),
        (NOW + 3601, NOW),
    ] {
        writer
            .execute(
                "UPDATE session SET time_created=?1, time_updated=?2 WHERE id='future'",
                [started * 1000, updated * 1000],
            )
            .unwrap();
        let scan = Opencode {
            path: path.clone(),
            now: NOW,
        }
        .scan(&Config::default());
        let row = scan
            .sessions
            .iter()
            .find(|s| s.id == "opencode:future")
            .unwrap();
        let clamp = |time| if time > NOW + 3600 { NOW } else { time };
        assert_eq!(row.started_at, clamp(started));
        assert_eq!(row.last_activity_at, clamp(updated));
        let warnings = usize::from(started > NOW + 3600 || updated > NOW + 3600);
        assert_eq!(scan.warnings.len(), warnings);
        if warnings > 0 {
            assert!(scan.warnings[0].contains(path.to_str().unwrap()));
            assert!(scan.warnings[0].contains("opencode:future"));
            assert!(scan.warnings[0].contains("clamped to now"));
        }
    }
}
