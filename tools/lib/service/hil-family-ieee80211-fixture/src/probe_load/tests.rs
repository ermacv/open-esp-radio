use super::*;
#[test]
fn source_does_not_start_before_controller_event() {
    let dir = tempfile::tempdir().unwrap();
    let marker = dir.path().join("started");
    let script = "read config; echo 'probe-source-v1 ready'; read command; test \"$command\" = start || exit 1; touch \"$1\"; echo '{\"bssid\":[2,0,0,0,0,1],\"submitted\":401,\"maximum_lateness_us\":0,\"error\":null}'";
    let mut command = Command::new("sh");
    command.args(["-c", script, "probe-test"]).arg(&marker);
    let config = model::Config {
        ssid: "test".into(),
        channel: 13,
    };
    let mut source = Source::prepare_command(&mut command, &config, dir.path()).unwrap();
    assert!(!marker.exists());
    source.start().unwrap();
    source.finish().unwrap();
    assert!(marker.exists());
}
#[test]
fn premature_helper_completion_cannot_pass_as_ready() {
    let dir = tempfile::tempdir().unwrap();
    let mut command = Command::new("sh");
    command.args(["-c", "read config; echo done"]);
    assert!(
        Source::prepare_command(
            &mut command,
            &model::Config {
                ssid: "test".into(),
                channel: 13
            },
            dir.path()
        )
        .is_err()
    );
}

#[test]
fn dropping_an_armed_source_waits_for_resource_cleanup() {
    let dir = tempfile::tempdir().unwrap();
    let marker = dir.path().join("resource");
    let mut command = Command::new("sh");
    command.args(["-c", r#"trap 'rm -f "$1"' EXIT; trap 'exit 0' TERM; read config; touch "$1"; echo 'probe-source-v1 ready'; read command"#, "probe-test"]).arg(&marker);
    let source = Source::prepare_command(
        &mut command,
        &model::Config {
            ssid: "test".into(),
            channel: 13,
        },
        dir.path(),
    )
    .unwrap();
    assert!(marker.exists());
    drop(source);
    assert!(!marker.exists());
}

#[test]
fn cooperative_shutdown_does_not_signal_the_cleanup_handler() {
    let dir = tempfile::tempdir().unwrap();
    let interrupted = dir.path().join("interrupted");
    let cleaned = dir.path().join("cleaned");
    let mut command = Command::new("sh");
    // The short delay models a blocking cleanup operation. Readiness is still
    // explicit and the test synchronizes on Drop, never on an assumed sleep.
    command.args(["-c", r#"trap 'touch "$1"' TERM; read config; echo 'probe-source-v1 ready'; read command; sleep 0.05; touch "$2""#, "probe-test"]).arg(&interrupted).arg(&cleaned);
    let source = Source::prepare_command(
        &mut command,
        &model::Config {
            ssid: "test".into(),
            channel: 13,
        },
        dir.path(),
    )
    .unwrap();
    drop(source);
    assert!(cleaned.exists());
    assert!(!interrupted.exists());
}
