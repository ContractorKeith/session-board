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
        ["client", "urgent", "untagged"]
    );
    let mut json = vec![];
    cli::list_with_state(&mut json, &tree, true, &state).unwrap();
    let value: serde_json::Value = serde_json::from_slice(&json).unwrap();
    assert_eq!(value[0]["sessions"][0]["project_root"], "/wait");
    assert_eq!(value[0]["sessions"][0]["unread"], true);
    assert!(
        value[2]["sessions"]
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
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_secs() as i64;
    std::thread::scope(|scope| {
        for side in ["left", "right"] {
            let path = &path;
            scope.spawn(move || {
                let mut state = State::default();
                state.pins.push(format!("/{side}").into());
                for tick in 0..30 {
                    state.read.insert(format!("codex:{side}"), now - 29 + tick);
                    state.save(path).unwrap();
                }
            });
        }
    });
    let old = State::load(&path).unwrap().0;
    assert_eq!(
        old.read,
        BTreeMap::from([("codex:left".into(), now), ("codex:right".into(), now)])
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
    let warnings = state.save_changes(&path, None).unwrap();
    assert!(
        warnings
            .iter()
            .any(|w| w.contains("non-UTF-8 state path skipped"))
    );
    assert_eq!(
        State::load(&path).unwrap().0.pins,
        [std::path::PathBuf::from("/good")]
    );
}

#[test]
fn issue75_cli_defaults_are_project_activity_despite_saved_preferences() {
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
    let default = run(&["list", "--json"]);
    assert_eq!(default[0]["root"], "/home/user/project");
    assert_eq!(default[0]["sessions"][0]["id"], "codex:error");
    let value = run(&["list", "--json", "--sort", "attention"]);
    assert_eq!(value[0]["pinned"], true);
    assert_eq!(value[0]["root"], "/home/user/project");
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
    assert_eq!(all[0]["sessions"][0]["group"], "tag:client");
    assert!(
        all[0]["sessions"]
            .as_array()
            .unwrap()
            .iter()
            .any(|s| s["id"] == "codex:done" && s["archived"] == true)
    );
    assert_eq!(fs::read_dir(temp.path().join("state")).unwrap().count(), 1);
}

#[test]
fn issue69_stale_writer_does_not_resurrect_removals_or_preferences() {
    let temp = tempfile::tempdir().unwrap();
    let path = temp.path().join("state.toml");
    let seed = State {
        pins: vec!["/alpha".into()],
        tags: BTreeMap::from([("/alpha".into(), vec!["old".into()])]),
        archived: vec!["codex:old".into()],
        ..Default::default()
    };
    seed.save(&path).unwrap();
    let baseline_a = State::load(&path).unwrap().0;
    let baseline_b = State::load(&path).unwrap().0;
    let mut a = baseline_a.clone();
    a.pins.clear();
    a.tags.clear();
    a.archived.clear();
    a.sort = Sort::Attention;
    a.group = Group::Tag;
    a.show_archived = true;
    a.save_changes(&path, Some(&baseline_a)).unwrap();
    let mut b = baseline_b.clone();
    b.tags.insert("/beta".into(), vec!["client".into()]);
    b.save_changes(&path, Some(&baseline_b)).unwrap();
    let saved = State::load(&path).unwrap().0;
    assert!(saved.pins.is_empty(), "stale pin resurrected");
    assert!(
        !saved.tags.contains_key(Path::new("/alpha")),
        "stale tag resurrected"
    );
    assert!(saved.archived.is_empty(), "stale archive resurrected");
    assert_eq!(saved.tags[Path::new("/beta")], ["client"]);
    assert_eq!(
        (saved.sort, saved.group, saved.show_archived),
        (Sort::Attention, Group::Tag, true)
    );
}

#[cfg(unix)]
#[test]
fn issue70_save_canonicalizes_aliases_before_removal() {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path().join("b");
    fs::create_dir(&root).unwrap();
    fs::create_dir(temp.path().join("a")).unwrap();
    let alias = temp.path().join("alias");
    std::os::unix::fs::symlink(&root, &alias).unwrap();
    let path = temp.path().join("state.toml");
    let home = cli::home().unwrap();
    let home_alias = home.join(".cache/../.cache");
    let seed = State {
        pins: vec![root, temp.path().join("a/../b/"), alias],
        tags: BTreeMap::from([(home_alias, vec!["safe".into()])]),
        ..Default::default()
    };
    fs::write(&path, toml::to_string(&seed).unwrap()).unwrap();
    let mut baseline = State::load(&path).unwrap().0;
    session_board::project::normalize_settings(&mut Config::default(), &mut baseline, &home);
    let mut edit = baseline.clone();
    edit.pins.clear();
    edit.save_changes(&path, Some(&baseline)).unwrap();
    let saved = State::load(&path).unwrap().0;
    assert!(
        saved.pins.is_empty(),
        "aliases survived removal: {:?}",
        saved.pins
    );
    assert!(
        saved
            .tags
            .keys()
            .all(|p| p.is_absolute()
                && !p.components().any(|c| c == std::path::Component::ParentDir))
    );
}

#[test]
fn issue77_read_max_merge_is_not_last_writer_wins() {
    let temp = tempfile::tempdir().unwrap();
    let path = temp.path().join("state.toml");
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_secs() as i64;
    let newer = State {
        read: BTreeMap::from([("codex:one".into(), now)]),
        ..Default::default()
    };
    let older = State {
        read: BTreeMap::from([("codex:one".into(), now - 10)]),
        ..Default::default()
    };
    newer.save(&path).unwrap();
    older.save(&path).unwrap();
    assert_eq!(State::load(&path).unwrap().0.read["codex:one"], now);
}

#[test]
fn issue73_lock_timeout_is_deterministic() {
    let temp = tempfile::tempdir().unwrap();
    let path = temp.path().join("state.toml");
    let lock = fs::File::open(temp.path()).unwrap();
    lock.lock().unwrap();
    let (tx, rx) = std::sync::mpsc::channel();
    std::thread::scope(|scope| {
        scope.spawn(|| {
            tx.send(State::default().save(&path)).unwrap();
        });
        let result = rx.recv_timeout(std::time::Duration::from_secs(3));
        lock.unlock().unwrap();
        let error = result
            .expect("save froze instead of timing out")
            .unwrap_err();
        assert!(
            error
                .to_string()
                .contains("state lock timed out after 2 seconds"),
            "{error}"
        );
    });
    assert!(fs::read_dir(temp.path()).unwrap().next().is_none());
}

#[cfg(unix)]
#[test]
fn issue73_dangling_symlink_is_kept() {
    let temp = tempfile::tempdir().unwrap();
    let path = temp.path().join("state.toml");
    let target = temp.path().join("target/state.toml");
    std::os::unix::fs::symlink("target/state.toml", &path).unwrap();
    State::default().save(&path).unwrap();
    assert!(
        fs::symlink_metadata(&path)
            .unwrap()
            .file_type()
            .is_symlink()
    );
    assert!(target.is_file());
}

#[cfg(unix)]
#[test]
fn issue73_unreadable_input_creates_no_backup() {
    use std::os::unix::fs::PermissionsExt;
    let temp = tempfile::tempdir().unwrap();
    let path = temp.path().join("state.toml");
    fs::write(&path, "broken = [").unwrap();
    fs::set_permissions(&path, fs::Permissions::from_mode(0o0)).unwrap();
    assert!(State::default().save(&path).is_err());
    assert_eq!(fs::read_dir(temp.path()).unwrap().count(), 1);
    fs::set_permissions(&path, fs::Permissions::from_mode(0o600)).unwrap();
}

#[test]
fn issue73_only_old_regular_temporary_files_are_removed() {
    let temp = tempfile::tempdir().unwrap();
    let path = temp.path().join("state.toml");
    let old = temp.path().join(".tmpold");
    let recent = temp.path().join(".tmpnew");
    let other = temp.path().join("keep");
    for p in [&old, &recent, &other] {
        fs::write(p, "x").unwrap();
    }
    let time = std::time::SystemTime::now() - std::time::Duration::from_secs(601);
    for p in [&old, &other] {
        fs::File::options()
            .write(true)
            .open(p)
            .unwrap()
            .set_times(fs::FileTimes::new().set_modified(time))
            .unwrap();
    }
    assert!(State::cleanup(&path).is_empty());
    assert!(!old.exists());
    assert!(recent.exists());
    assert!(other.exists());
}

#[cfg(unix)]
#[test]
fn issue77_corrupt_backup_keeps_mode_and_returns_warning() {
    use std::os::unix::fs::PermissionsExt;
    let temp = tempfile::tempdir().unwrap();
    let path = temp.path().join("state.toml");
    fs::write(&path, "[broken").unwrap();
    fs::set_permissions(&path, fs::Permissions::from_mode(0o600)).unwrap();
    let warnings = State::default().save_changes(&path, None).unwrap();
    assert!(
        warnings
            .iter()
            .any(|w| w.contains("corrupt state preserved"))
    );
    let backup = fs::read_dir(temp.path())
        .unwrap()
        .map(|e| e.unwrap().path())
        .find(|p| p != &path)
        .unwrap();
    assert_eq!(
        fs::metadata(backup).unwrap().permissions().mode() & 0o777,
        0o600
    );
}

#[test]
fn issue77_m0_shape_accepts_new_state_keys() {
    #[derive(serde::Deserialize)]
    struct OldState {
        pins: Vec<std::path::PathBuf>,
        tags: BTreeMap<std::path::PathBuf, Vec<String>>,
        archived: Vec<String>,
        read: BTreeMap<String, i64>,
    }
    let temp = tempfile::tempdir().unwrap();
    let path = temp.path().join("state.toml");
    State {
        pins: vec!["/alpha".into()],
        group: Group::Tag,
        sort: Sort::Attention,
        show_archived: true,
        ..Default::default()
    }
    .save(&path)
    .unwrap();
    let old: OldState = toml::from_str(&fs::read_to_string(path).unwrap()).unwrap();
    assert_eq!(old.pins, [std::path::PathBuf::from("/alpha")]);
    assert!(old.tags.is_empty() && old.archived.is_empty() && old.read.is_empty());
}

#[test]
fn issue74_tag_buckets_do_not_collide_and_pins_order_members() {
    let mut state = State {
        first_seen: Some(0),
        pins: vec!["/pinned".into()],
        ..Default::default()
    };
    state.tags.insert("/wait".into(), vec!["untagged".into()]);
    state.tags.insert("/pinned".into(), vec!["untagged".into()]);
    state.tags.insert("/done".into(), vec!["tag:evil".into()]);
    let mut tree = projects(&state);
    scan::organize(&mut tree, &state, Sort::Attention, Group::Tag);
    assert!(
        tree.iter()
            .any(|p| p.root == Path::new("untagged:") && p.tags.is_empty())
    );
    let tagged = tree
        .iter()
        .find(|p| p.root == Path::new("tag:untagged"))
        .unwrap();
    assert_eq!(tagged.sessions.len(), 2);
    assert_eq!(tagged.sessions[0].project_root, Path::new("/pinned"));
    assert!(!tagged.pinned, "a group is not a pinned project");
}

#[test]
fn issue76_archives_excluded_even_with_all() {
    let state = State {
        first_seen: Some(0),
        archived: vec!["codex:waiting".into(), "/error".into()],
        ..Default::default()
    };
    let mut tree = projects(&state);
    cli::filter(&mut tree, &state, None, None, None, true);
    assert_eq!(tree.iter().map(|p| p.unread_count).sum::<usize>(), 2);
    let mut tree = projects(&state);
    cli::filter(
        &mut tree,
        &state,
        None,
        Some(cli::StatusFilter::Attention),
        None,
        true,
    );
    assert_eq!(
        tree.iter()
            .flat_map(|p| &p.sessions)
            .map(|s| s.id.as_str())
            .collect::<Vec<_>>(),
        ["codex:done"]
    );
}

#[test]
fn issue76_marks_pruned_and_tags_sanitized_on_load() {
    let temp = tempfile::tempdir().unwrap();
    let path = temp.path().join("state.toml");
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_secs() as i64;
    let state = State {
        tags: BTreeMap::from([(
            "/root".into(),
            vec!["\u{1b}[31mred\u{1b}[0m\nline\u{7}".into()],
        )]),
        read: BTreeMap::from([
            ("old".into(), now - 91 * 86400),
            ("future".into(), now + 60),
            ("valid".into(), now),
        ]),
        ..Default::default()
    };
    fs::write(&path, toml::to_string(&state).unwrap()).unwrap();
    let (loaded, _) = State::load(&path).unwrap();
    assert_eq!(loaded.tags[Path::new("/root")], ["red line"]);
    loaded.save(&path).unwrap();
    assert_eq!(
        State::load(&path).unwrap().0.read,
        BTreeMap::from([("valid".into(), now)])
    );
}

#[test]
fn issue77_archive_removal_is_persisted() {
    let temp = tempfile::tempdir().unwrap();
    let path = temp.path().join("state.toml");
    State {
        archived: vec!["codex:one".into(), "/root".into()],
        ..Default::default()
    }
    .save(&path)
    .unwrap();
    let previous = State::load(&path).unwrap().0;
    let mut edit = previous.clone();
    edit.archived.clear();
    edit.save_changes(&path, Some(&previous)).unwrap();
    assert!(State::load(&path).unwrap().0.archived.is_empty());
}

#[test]
fn issue77_tag_removal_preserves_another_writers_addition() {
    let temp = tempfile::tempdir().unwrap();
    let path = temp.path().join("state.toml");
    State {
        tags: BTreeMap::from([("/root".into(), vec!["remove".into()])]),
        ..Default::default()
    }
    .save(&path)
    .unwrap();
    let previous = State::load(&path).unwrap().0;
    let mut other = previous.clone();
    other
        .tags
        .get_mut(Path::new("/root"))
        .unwrap()
        .push("keep".into());
    other.save_changes(&path, Some(&previous)).unwrap();
    let mut edit = previous.clone();
    edit.tags.clear();
    edit.save_changes(&path, Some(&previous)).unwrap();
    assert_eq!(
        State::load(&path).unwrap().0.tags[Path::new("/root")],
        ["keep"]
    );
}

// The macOS CI filesystem rejects this filename before a source can read it.
#[cfg(target_os = "linux")]
#[test]
fn issue76_doctor_counts_non_utf8_transcript_path() {
    use std::{ffi::OsString, os::unix::ffi::OsStringExt, process::Command};
    let temp = tempfile::tempdir().unwrap();
    let store = temp.path().join("claude/projects/project");
    fs::create_dir_all(&store).unwrap();
    fs::copy(
        "fixtures/claude_code/project/done.jsonl",
        store.join(OsString::from_vec(b"bad\xff.jsonl".to_vec())),
    )
    .unwrap();
    let output = Command::new(env!("CARGO_BIN_EXE_sboard"))
        .args(["doctor", "--json"])
        .env("HOME", temp.path())
        .env("CLAUDE_CONFIG_DIR", temp.path().join("claude"))
        .env("CODEX_HOME", temp.path().join("codex"))
        .env("SBOARD_STATE_DIR", temp.path().join("state"))
        .env("SBOARD_CONFIG_DIR", temp.path().join("config"))
        .output()
        .unwrap();
    assert!(output.status.success());
    let value: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(value["stores"][0]["sessions"], 0);
    assert_eq!(value["stores"][0]["skipped_sessions"], 1);
    assert!(
        value["stores"][0]["warnings"]
            .as_array()
            .unwrap()
            .iter()
            .any(|w| w.as_str().unwrap().contains("non-UTF-8"))
    );
}

#[test]
fn issue74_path_like_tag_names_keep_distinct_group_roots() {
    let mut state = State::default();
    state.tags.insert(
        "/wait".into(),
        vec!["a/b".into(), "a//b".into(), "a/./b".into(), "a%2Fb".into()],
    );
    let mut tree = projects(&state);
    scan::organize(&mut tree, &state, Sort::Activity, Group::Tag);
    let tagged: Vec<_> = tree.iter().filter(|p| p.name != "untagged").collect();
    assert_eq!(tagged.len(), 4);
    assert_eq!(
        tagged
            .iter()
            .map(|p| &p.root)
            .collect::<std::collections::BTreeSet<_>>()
            .len(),
        4
    );
}

#[test]
fn issue95_equivalent_tag_spellings_are_unioned_during_deserialization() {
    let temp = tempfile::tempdir().unwrap();
    let path = temp.path().join("state.toml");
    fs::write(&path, "[tags]\n\"/p\" = [\"one\"]\n\"/p/\" = [\"two\"]\n").unwrap();
    let (state, warnings) = State::load(&path).unwrap();
    assert!(warnings.is_empty());
    assert_eq!(state.tags[Path::new("/p")], ["one", "two"]);
    state.save(&path).unwrap();
    assert_eq!(
        State::load(&path).unwrap().0.tags[Path::new("/p")],
        ["one", "two"]
    );
}

#[test]
fn issue103_transcript_path_breaks_equal_timestamp_and_id_ties() {
    let state = State::default();
    for sort in [Sort::Activity, Sort::Attention] {
        for group in [Group::Project, Group::Tag] {
            let mut tree = projects(&state);
            tree.truncate(1);
            let mut a = tree[0].sessions[0].clone();
            a.id = "codex:duplicate".into();
            a.transcript_path = "a.jsonl".into();
            let mut z = a.clone();
            z.transcript_path = "z.jsonl".into();
            tree[0].sessions = vec![z, a];
            scan::organize(&mut tree, &state, sort, group);
            assert_eq!(
                tree[0]
                    .sessions
                    .iter()
                    .map(|s| s.transcript_path.as_path())
                    .collect::<Vec<_>>(),
                [Path::new("a.jsonl"), Path::new("z.jsonl")]
            );
        }
    }
}
