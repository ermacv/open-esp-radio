//! A capture, fixture restoration and the next boot under one cancellation.

use std::{
    fs,
    time::{Duration, Instant},
};

use oer_hil_link::test_support::{Output, activate, capture, frame, hello};

#[cfg(unix)]
#[test]
fn signal_cancellation_harness() {
    let Ok(signal) = std::env::var("OER_HIL_CAPTURE_TEST_SIGNAL") else {
        return;
    };
    let signal = rustix::process::Signal::from_named_raw(signal.parse().unwrap()).unwrap();
    let _signals = oer_process::install_signal_handlers().unwrap();
    let output = Output::new();
    let cleanup = crate::fixture::cleanup::Scope::new(&output.0);
    let (capture, input) = capture(&output, false);
    activate(&capture, &input);
    let sender = std::thread::spawn(move || {
        std::thread::sleep(Duration::from_millis(50));
        // This isolated test process installed the handler above.
        rustix::process::kill_process(rustix::process::getpid(), signal).unwrap();
    });
    let started = Instant::now();
    // A protocol wait no message ends: only the cancellation does.
    let error = capture
        .wait_for_message_after(0, Duration::from_secs(60), |_| false)
        .unwrap_err();
    sender.join().unwrap();
    assert!(oer_process::is_cancelled(&*error));
    assert!(started.elapsed() < Duration::from_secs(2));
    drop(capture);
    crate::fixture::cleanup::record("restore after cancellation", || {
        oer_process::check_cancelled()?;
        fs::write(output.0.join("restored"), b"yes")?;
        Ok(())
    });
    let records = cleanup.finish().unwrap();
    assert_eq!(records.len(), 1);
    assert!(records[0].failure.is_none());
    assert!(oer_process::check_cancelled().is_err());
    let lab = oer_hil_lab::config::LabConfig::for_test();
    let context = super::Context::new(&lab, &output.0);
    let next = output.0.join("must-not-reset");
    let error = context
        .capture(&next)
        .err()
        .expect("cancel before opening another boot");
    assert!(oer_process::is_cancelled(&*error));
    assert!(!next.exists());
    assert_eq!(
        fs::read(output.0.join("uart.bin")).unwrap(),
        frame(hello(7, 0))
    );
    let protocol = fs::read_to_string(output.0.join("protocol.jsonl")).unwrap();
    assert!(protocol.lines().any(|line| {
        let record: serde_json::Value = serde_json::from_str(line).unwrap();
        record["cancelled"] == true
    }));
}

#[cfg(unix)]
#[test]
fn signals_cancel_protocol_wait_and_preserve_partial_capture() {
    for signal in [rustix::process::Signal::INT, rustix::process::Signal::TERM] {
        let status = oer_process::owned::Child::spawn(
            std::process::Command::new(std::env::current_exe().unwrap())
                .args(["--exact", "context::tests::signal_cancellation_harness"])
                .env("OER_HIL_CAPTURE_TEST_SIGNAL", signal.as_raw().to_string()),
        )
        .unwrap()
        .wait_timeout(Some(Duration::from_secs(10)))
        .unwrap();
        assert!(status.success());
    }
}
