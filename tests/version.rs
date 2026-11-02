use std::process::Command;

#[test]
fn version_subcommand_and_flag_match_cargo_manifest() {
    let manifest: toml::Value = toml::from_str(include_str!("../Cargo.toml")).unwrap();
    let version = manifest["package"]["version"].as_str().unwrap();
    for argument in ["version", "--version"] {
        let output = Command::new(env!("CARGO_BIN_EXE_sboard"))
            .arg(argument)
            .output()
            .unwrap();
        assert!(output.status.success(), "{argument}: {output:?}");
        assert_eq!(output.stdout, format!("sboard {version}\n").as_bytes());
        assert!(output.stderr.is_empty(), "{argument}: {output:?}");
    }
}
