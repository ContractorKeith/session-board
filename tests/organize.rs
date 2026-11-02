use session_board::{
    cli,
    config::Config,
    model::{Agent, Session, Status},
    scan::{self, Group, Sort},
    source::{Registry, Source, SourceScan},
    state::State,
};
use std::{collections::BTreeMap, fs, path::Path};

fn session(id: &str, root: &str, status: Status, time: i64) -> Session {
    Session {
        id: format!("codex:{id}"),
        agent: Agent::Codex,
        cwd: root.into(),
        project_root: root.into(),
        title: id.into(),
        started_at: 0,
        last_activity_at: time,
        status,
        last_user_text: String::new(),
        last_assistant_text: String::new(),
        transcript_path: format!("{id}.jsonl").into(),
        resume_hint: String::new(),
        pane: None,
    }
}
struct Fixture(Vec<Session>);
impl Source for Fixture {
    fn agent(&self) -> Agent {
        Agent::Codex
    }
    fn scan(&self, _: &Config) -> SourceScan {
        SourceScan {
            sessions: self.0.clone(),
            ..Default::default()
        }
    }
}
fn projects(state: &State) -> Vec<session_board::model::Project> {
    let registry: Registry = vec![Box::new(Fixture(vec![
        session("waiting", "/wait", Status::Waiting, 20),
        session("error", "/error", Status::Error, 30),
        session("done", "/done", Status::Done, 40),
        session("idle", "/pinned", Status::Idle, 10),
    ]))];
    scan::scan(&registry, &Config::default(), state, Path::to_path_buf).projects
}
#[test]
fn first_run_read_marks_and_deterministic_projection() {
    let mut state = State::default();
    let tree = projects(&state);
    assert!(tree.iter().all(|p| p.unread_count == 0));
    state.first_seen = Some(0);
    state.pins.push("/pinned".into());
    state
        .tags
        .insert("/wait".into(), vec!["client".into(), "urgent".into()]);
    for (sort, expected) in [
        (Sort::Activity, vec!["pinned", "done", "error", "wait"]),
        (Sort::Attention, vec!["pinned", "wait", "error", "done"]),
    ] {
        let mut tree = projects(&state);
        scan::organize(&mut tree, &state, sort, Group::Project);
        assert_eq!(
            tree.iter().map(|p| p.name.as_str()).collect::<Vec<_>>(),
            expected
        );
        tree.reverse();
        scan::organize(&mut tree, &state, sort, Group::Project);
        assert_eq!(
            tree.iter().map(|p| p.name.as_str()).collect::<Vec<_>>(),
            expected
        );
    }
    state.archived = vec!["/error".into(), "codex:done".into()];
    let mut tree = projects(&state);
    cli::filter(&mut tree, &state, None, None, None, false);
    assert_eq!(tree.iter().flat_map(|p| &p.sessions).count(), 2);
    let mut tree = projects(&state);
    cli::filter(&mut tree, &state, None, None, None, true);
    assert_eq!(tree.iter().flat_map(|p| &p.sessions).count(), 4);
    scan::organize(&mut tree, &state, Sort::Attention, Group::Tag);
    assert_eq!(
        tree.iter().map(|p| p.name.as_str()).collect::<Vec<_>>(),
        ["untagged", "client", "urgent"]
    );
    let mut json = vec![];
    cli::list_with_state(&mut json, &tree, true, &state).unwrap();
    let value: serde_json::Value = serde_json::from_slice(&json).unwrap();
    assert_eq!(value[1]["sessions"][0]["project_root"], "/wait");
    assert_eq!(value[1]["sessions"][0]["unread"], true);
    assert!(
        value[0]["sessions"]
            .as_array()
            .unwrap()
            .iter()
            .any(|s| s["archived"] == true)
    );
    state.read.insert("codex:waiting".into(), 20);
    assert!(!state.unread(&session("waiting", "/wait", Status::Waiting, 20)));
    assert!(state.unread(&session("waiting", "/wait", Status::Waiting, 21)));
    state.pins.push("/gone".into());
    assert!(
        projects(&state)
            .iter()
            .any(|p| p.root == Path::new("/gone") && p.pinned && p.sessions.is_empty())
    );
}
#[test]
fn concurrent_saves_merge_and_explicit_removals_survive() {
    let temp = tempfile::tempdir().unwrap();
    let path = temp.path().join("state.toml");
    std::thread::scope(|scope| {
        for side in ["left", "right"] {
            let path = &path;
            scope.spawn(move || {
                let mut state = State::default();
                state.pins.push(format!("/{side}").into());
                for tick in 0..30 {
                    state.read.insert(format!("codex:{side}"), tick);
                    state.save(path).unwrap();
                }
            });
        }
    });
    let old = State::load(&path).unwrap().0;
    assert_eq!(
        old.read,
        BTreeMap::from([("codex:left".into(), 29), ("codex:right".into(), 29)])
    );
    assert_eq!(old.pins.len(), 2);
    assert!(old.first_seen.is_some());
    let mut edited = old.clone();
    edited.pins.retain(|p| p != Path::new("/left"));
    edited.save_changes(&path, Some(&old)).unwrap();
    assert_eq!(
        State::load(&path).unwrap().0.pins,
        [std::path::PathBuf::from("/right")]
    );
    assert_eq!(fs::read_dir(temp.path()).unwrap().count(), 1);
}
#[test]
fn corrupt_input_is_preserved_before_first_save() {
    let temp = tempfile::tempdir().unwrap();
    let path = temp.path().join("state.toml");
    let corrupt = b"pins = [\"/important\"]\nread = { broken = \"x\" }";
    fs::write(&path, corrupt).unwrap();
    let (state, warnings) = State::load(&path).unwrap();
    assert_eq!(warnings.len(), 1);
    state.save(&path).unwrap();
    let backup = fs::read_dir(temp.path())
        .unwrap()
        .map(|e| e.unwrap().path())
        .find(|p| {
            p.file_name()
                .unwrap()
                .to_string_lossy()
                .contains(".corrupt-")
        })
        .unwrap();
    assert_eq!(fs::read(backup).unwrap(), corrupt);
    assert!(State::load(&path).unwrap().0.first_seen.is_some());
}
#[cfg(unix)]
#[test]
fn invalid_utf8_pin_is_skipped_without_losing_other_state() {
    use std::{ffi::OsString, os::unix::ffi::OsStringExt};
    let temp = tempfile::tempdir().unwrap();
    let path = temp.path().join("state.toml");
    let state = State {
        pins: vec![
            OsString::from_vec(b"/bad\xff".to_vec()).into(),
            "/good".into(),
        ],
        ..Default::default()
    };
    state.save(&path).unwrap();
    assert_eq!(
        State::load(&path).unwrap().0.pins,
        [std::path::PathBuf::from("/good")]
    );
}

#[test]
fn cli_flags_json_archive_and_tag_shape_use_saved_state() {
    use std::process::Command;
    let temp = tempfile::tempdir().unwrap();
    let state = State {
        first_seen: Some(0),
        group: Group::Tag,
        sort: Sort::Attention,
        pins: vec!["/home/user/project".into()],
        tags: BTreeMap::from([("/home/user/project".into(), vec!["client".into()])]),
        archived: vec!["codex:done".into()],
        ..Default::default()
    };
    state.save(&temp.path().join("state/state.toml")).unwrap();
    let run = |args: &[&str]| {
        let output = Command::new(env!("CARGO_BIN_EXE_sboard"))
            .args(args)
            .env("HOME", temp.path())
            .env("SBOARD_STATE_DIR", temp.path().join("state"))
            .env("SBOARD_CONFIG_DIR", temp.path().join("config"))
            .env("CLAUDE_CONFIG_DIR", temp.path().join("absent"))
            .env("CODEX_HOME", fs::canonicalize("fixtures/codex").unwrap())
            .output()
            .unwrap();
        assert!(
            output.status.success(),
            "{}",
            String::from_utf8_lossy(&output.stderr)
        );
        serde_json::from_slice::<serde_json::Value>(&output.stdout).unwrap()
    };
    let value = run(&["list", "--json", "--sort", "attention"]);
    assert_eq!(value[0]["pinned"], true);
    assert_eq!(value[0]["root"], "tag:client");
    assert_eq!(
        run(&["list", "--json", "--group", "project"])[0]["root"],
        "/home/user/project"
    );
    assert!(
        value[0]["sessions"]
            .as_array()
            .unwrap()
            .iter()
            .all(|s| s["id"] != "codex:done")
    );
    let all = run(&["list", "--json", "--all", "--group", "tag"]);
    assert_eq!(all[0]["root"], "tag:client");
    assert!(
        all[0]["sessions"]
            .as_array()
            .unwrap()
            .iter()
            .any(|s| s["id"] == "codex:done" && s["archived"] == true)
    );
    assert_eq!(fs::read_dir(temp.path().join("state")).unwrap().count(), 1);
}
