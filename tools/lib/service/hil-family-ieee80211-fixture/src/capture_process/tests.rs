use super::*;

#[test]
fn stop_is_explicit_and_ready_does_not_require_packets() {
    let directory = tempfile::tempdir().unwrap();
    let stopped = directory.path().join("stopped");
    let script = format!(
        "printf 'capture-open\\n' >&2; read command; test \"$command\" = stop; touch {}; printf 'capture-closed\\n' >&2",
        quote(stopped.to_str().unwrap())
    );
    let mut command = Command::new("sh");
    command.args(["-c", &script]);
    let capture =
        Capture::start(&mut command, "capture-open".into(), Duration::from_secs(10)).unwrap();
    assert!(!stopped.exists());
    let output = capture.finish().unwrap();
    assert!(output.status.success());
    assert!(stopped.exists());
    assert!(
        String::from_utf8(output.stderr)
            .unwrap()
            .contains("capture-closed")
    );
}

#[test]
fn exit_without_a_ready_event_cannot_start_a_session() {
    let mut command = Command::new("sh");
    command.args(["-c", "printf 'permission denied\\n' >&2; exit 1"]);
    assert!(Capture::start(&mut command, "capture-open".into(), Duration::from_secs(5)).is_err());
}

#[test]
fn a_single_unterminated_diagnostic_line_is_bounded() {
    let (sender, receiver) = mpsc::channel();
    let error = read_diagnostics(std::io::repeat(b'x'), "capture-open", &sender).unwrap_err();
    assert!(error.to_string().contains("exceeded 1 MiB"));
    assert!(receiver.try_recv().is_err());
}

#[test]
fn premature_exit_preserves_the_capture_failure() {
    let mut command = Command::new("sh");
    command.args([
        "-c",
        "exec 0<&-; printf 'capture-open\\nwrite failed: no space\\n' >&2; exit 4",
    ]);
    let capture =
        Capture::start(&mut command, "capture-open".into(), Duration::from_secs(5)).unwrap();
    let error = capture.finish().unwrap_err();
    assert!(error.to_string().contains("write failed: no space"));
    assert!(error.to_string().contains("exit status: 4"));
}

#[test]
fn nonzero_exit_after_accepting_stop_is_still_a_capture_failure() {
    let mut command = Command::new("sh");
    command.args([
        "-c",
        "printf 'capture-open\n' >&2; read command; test \"$command\" = stop; printf 'capture failed after stop\n' >&2; exit 4",
    ]);
    let capture =
        Capture::start(&mut command, "capture-open".into(), Duration::from_secs(5)).unwrap();
    let error = capture.finish().unwrap_err();
    assert!(
        error
            .to_string()
            .contains("capture process failed while completing Stop")
    );
    assert!(error.to_string().contains("capture failed after stop"));
    assert!(error.to_string().contains("exit status: 4"));
}

#[test]
#[ignore = "requires installed dumpcap and local packet-capture permissions"]
fn installed_dumpcap_acknowledges_ready_and_reports_explicit_stop() {
    let capture = dumpcap(
        "lo",
        Some("udp and port 9"),
        128,
        std::path::Path::new("/dev/null"),
        Duration::from_secs(1),
    )
    .unwrap();
    let output = capture.finish().unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(String::from_utf8_lossy(&output.stderr).contains("Packets captured:"));
}

#[test]
fn every_capture_has_a_ring_for_ceiling_bursts() {
    let program = dumpcap_program("eth0", Some("arp"), 128, "/tmp/x.pcapng");
    assert!(program.contains(" -B 64 "), "{program}");
}
