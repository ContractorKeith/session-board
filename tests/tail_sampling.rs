use session_board::{
    config::Config,
    model::Status,
    source::{Source, claude::Claude, codex::Codex},
};
use std::{fs, time::Instant};
const NOW: i64 = 1793534400;

#[test]
fn completed_turn_behind_large_ignored_records() {
    for name in ["claude.jsonl", "rollout-codex.jsonl"] {
        let temp = tempfile::tempdir().unwrap();
        fs::copy(
            format!("fixtures/tail-sampling/{name}"),
            temp.path().join(name),
        )
        .unwrap();
        let source: Box<dyn Source> = if name == "claude.jsonl" {
            Box::new(Claude {
                root: temp.path().into(),
                now: NOW,
            })
        } else {
            Box::new(Codex {
                root: temp.path().into(),
                index: temp.path().join("absent"),
                now: NOW,
            })
        };
        let scan = source.scan(&Config::default());
        assert_eq!(scan.sessions.len(), 1);
        assert_eq!(scan.sessions[0].status, Status::Done, "{name}");
        assert_eq!(scan.sessions[0].last_assistant_text, "Ready.", "{name}");
        assert_eq!(scan.skipped_lines, 0);
    }
}

#[test]
#[ignore = "manual release performance measurement; 10,000 fixture-shaped sessions"]
fn ten_thousand_attachment_sessions() {
    let temp = tempfile::tempdir().unwrap();
    let fixture = fs::canonicalize("fixtures/tail-sampling/claude.jsonl").unwrap();
    for i in 0..10000 {
        fs::hard_link(&fixture, temp.path().join(format!("{i}.jsonl"))).unwrap();
    }
    let source = Claude {
        root: temp.path().into(),
        now: NOW,
    };
    for run in 1..=3 {
        let start = Instant::now();
        let scan = source.scan(&Config::default());
        let elapsed = start.elapsed();
        assert_eq!(scan.sessions.len(), 10000);
        eprintln!(
            "10,000 Claude attachment sessions run {run}: {elapsed:?}; done={}",
            scan.sessions
                .iter()
                .filter(|s| s.status == Status::Done)
                .count()
        );
    }
}

#[test]
fn attachment_cap_is_reported_without_reusing_head_status() {
    for single_record in [false, true] {
        let fixture = fs::read_to_string("fixtures/tail-sampling/claude.jsonl").unwrap();
        let mut rows: Vec<serde_json::Value> = fixture
            .lines()
            .map(|l| serde_json::from_str(l).unwrap())
            .collect();
        rows.truncate(3);
        let mut text: String = rows.iter().map(|v| format!("{v}\n")).collect();
        let attachment = if single_record {
            serde_json::json!({"type":"attachment", "text":"x".repeat(2 * 1024 * 1024)}).to_string()
                + "\n"
        } else {
            "{\"type\":\"attachment\"}\n".repeat(100_000)
        };
        text.push_str(&attachment);
        let temp = tempfile::tempdir().unwrap();
        fs::write(temp.path().join("cap.jsonl"), text).unwrap();
        let scan = Claude {
            root: temp.path().into(),
            now: NOW,
        }
        .scan(&Config::default());
        assert_eq!(scan.sessions.len(), 1);
        assert_eq!(scan.sessions[0].status, Status::Idle);
        assert!(scan.sessions[0].last_assistant_text.is_empty());
        assert_eq!(scan.skipped_lines, 1);
        assert!(scan.warnings.iter().any(|w| w.contains("1 MiB tail cap")));
    }
}

#[test]
fn small_transcript_and_recovery_to_head_keep_completed_turn() {
    let fixture = fs::read_to_string("fixtures/tail-sampling/claude.jsonl").unwrap();
    let rows: Vec<serde_json::Value> = fixture
        .lines()
        .map(|l| serde_json::from_str(l).unwrap())
        .collect();
    for padding in [0, 60_000, 270_000] {
        let text = format!(
            "{}\n{}\n{}\n",
            rows[0],
            rows[2],
            serde_json::json!({"type":"attachment", "text":"x".repeat(padding)})
        );
        let temp = tempfile::tempdir().unwrap();
        fs::write(temp.path().join("small.jsonl"), text).unwrap();
        let scan = Claude {
            root: temp.path().into(),
            now: NOW,
        }
        .scan(&Config::default());
        assert_eq!(scan.sessions[0].status, Status::Done);
        assert_eq!(scan.sessions[0].last_assistant_text, "Ready.");
        assert_eq!(scan.skipped_lines, 0);
        assert!(scan.warnings.is_empty());
    }
}

#[test]
fn complete_record_at_exact_tail_cap_is_retained() {
    let fixture = fs::read_to_string("fixtures/tail-sampling/claude.jsonl").unwrap();
    let rows: Vec<serde_json::Value> = fixture
        .lines()
        .map(|l| serde_json::from_str(l).unwrap())
        .collect();
    let complete = format!("{}\n", rows[2]);
    let mut attachment = serde_json::json!({"type":"attachment", "text":""});
    let overhead = format!("{attachment}\n").len();
    attachment["text"] = serde_json::json!("x".repeat(1024 * 1024 - complete.len() - overhead));
    let text = format!("{}\n{}\n{complete}{attachment}\n", rows[0], rows[1]);
    let temp = tempfile::tempdir().unwrap();
    fs::write(temp.path().join("boundary.jsonl"), text).unwrap();
    let scan = Claude {
        root: temp.path().into(),
        now: NOW,
    }
    .scan(&Config::default());
    assert_eq!(scan.sessions[0].status, Status::Done);
    assert_eq!(scan.sessions[0].last_assistant_text, "Ready.");
    assert_eq!(scan.skipped_lines, 0);
    assert!(scan.warnings.is_empty());
}

#[test]
fn meaningful_record_one_byte_beyond_one_mib_cap_is_not_recovered() {
    let fixture = fs::read_to_string("fixtures/tail-sampling/claude.jsonl").unwrap();
    let rows: Vec<serde_json::Value> = fixture
        .lines()
        .map(|line| serde_json::from_str(line).unwrap())
        .collect();
    let complete = format!("{}\n", rows[2]);
    let mut attachment = serde_json::json!({"type":"attachment", "text":""});
    let overhead = format!("{attachment}\n").len();
    // Pin the contract independently of the reader's MAX_TAIL_BYTES constant.
    let distance = 1024 * 1024 + 1;
    attachment["text"] = serde_json::json!("x".repeat(distance - complete.len() - overhead));
    let prefix = format!("{}\n{}\n", rows[0], rows[1]);
    let text = format!("{prefix}{complete}{attachment}\n");
    assert_eq!(text.len() - prefix.len(), distance);
    let temp = tempfile::tempdir().unwrap();
    fs::write(temp.path().join("beyond-cap.jsonl"), text).unwrap();
    let scan = Claude {
        root: temp.path().into(),
        now: NOW,
    }
    .scan(&Config::default());
    assert_eq!(scan.sessions.len(), 1);
    assert_eq!(scan.sessions[0].status, Status::Idle);
    assert!(scan.sessions[0].last_assistant_text.is_empty());
    assert_eq!(scan.skipped_sessions, 0);
    assert_eq!(scan.skipped_lines, 1);
    assert!(
        scan.warnings
            .iter()
            .any(|warning| warning.contains("1 MiB tail cap"))
    );
}
