use std::process::Command;

#[test]
fn binary_without_a_subcommand_refuses_standalone_launch() {
    let output = Command::new(env!("CARGO_BIN_EXE_herdr-workbench"))
        .output()
        .expect("run binary");
    assert!(!output.status.success());
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(stderr.contains("companion plugin"));
    assert!(stderr.contains("does not launch standalone"));
}

#[test]
fn sidebar_subcommand_requires_a_managed_plugin_pane() {
    let directory = tempfile::tempdir().expect("state directory");
    let output = Command::new(env!("CARGO_BIN_EXE_herdr-workbench"))
        .arg("sidebar")
        .env("HERDR_PLUGIN_STATE_DIR", directory.path())
        .env_remove("HERDR_ENV")
        .env_remove("HERDR_SOCKET_PATH")
        .env_remove("HERDR_PLUGIN_ID")
        .env_remove("HERDR_PLUGIN_ENTRYPOINT_ID")
        .output()
        .expect("run binary");
    assert!(!output.status.success());
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(stderr.contains("must run as a Herdr-managed plugin pane"));
}
