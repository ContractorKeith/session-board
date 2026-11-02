use session_board::{
    cli,
    model::{Project, Session},
    scan::{self, Group, Sort},
    state::State,
};

fn fixture() -> (Vec<Project>, State) {
    let mut projects: Vec<Project> =
        serde_json::from_str(include_str!("fixtures/attention-projects.json")).unwrap();
    projects.truncate(1);
    projects[0].sessions.truncate(1);
    projects[0].sessions[0].title = "Résumé \"quoted\"\nsecond line".into();
    projects[0].pinned = true;
    projects[0].tags = vec!["rust/CLI%".into()];
    let mut state = State {
        first_seen: Some(0),
        ..Default::default()
    };
    state.pins.push(projects[0].root.clone());
    state
        .tags
        .insert(projects[0].root.clone(), projects[0].tags.clone());
    (projects, state)
}

// One byte-level contract covers field order, whitespace, optional fields and escaping.
#[test]
fn json_output_contract() {
    let (mut projects, mut state) = fixture();
    let mut output = Vec::new();
    cli::list_with_state(&mut output, &projects, true, &state).unwrap();
    cli::attention_json(&mut output, &projects, &state).unwrap();
    scan::organize(&mut projects, &state, Sort::Activity, Group::Tag);
    cli::list_with_state(&mut output, &projects, true, &state).unwrap();
    cli::attention_json(&mut output, &projects, &state).unwrap();
    let session: &Session = &projects[0].sessions[0];
    state
        .read
        .insert(session.id.clone(), session.last_activity_at);
    state.archived.push(session.id.clone());
    cli::list_with_state(&mut output, &projects, true, &state).unwrap();
    cli::attention_json(&mut output, &projects, &state).unwrap();
    cli::list_with_state(&mut output, &[], true, &state).unwrap();
    assert_eq!(
        String::from_utf8(output).unwrap(),
        std::fs::read_to_string("tests/snapshots/json-output.txt").unwrap()
    );
}

#[test]
fn json_output_propagates_writer_errors() {
    struct Broken;
    impl std::io::Write for Broken {
        fn write(&mut self, _: &[u8]) -> std::io::Result<usize> {
            Err(std::io::ErrorKind::BrokenPipe.into())
        }
        fn flush(&mut self) -> std::io::Result<()> {
            Ok(())
        }
    }
    let (projects, state) = fixture();
    assert_eq!(
        cli::list_with_state(&mut Broken, &projects, true, &state)
            .unwrap_err()
            .kind(),
        std::io::ErrorKind::BrokenPipe
    );
    assert_eq!(
        cli::attention_json(&mut Broken, &projects, &state)
            .unwrap_err()
            .kind(),
        std::io::ErrorKind::BrokenPipe
    );
}
