use super::*;

#[test]
fn readiness_handles_enabled_and_events_on_both_sides_of_status_response() {
    for ordering in ["already", "before", "after", "disabled", "disabled-before"] {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("hostapd");
        let server = UnixDatagram::bind(&path).unwrap();
        server
            .set_read_timeout(Some(Duration::from_secs(2)))
            .unwrap();
        let worker = std::thread::spawn(move || {
            let mut bytes = [0; 128];
            let (n, client) = server.recv_from(&mut bytes).unwrap();
            let client = client.as_pathname().unwrap();
            assert_eq!(&bytes[..n], b"ATTACH");
            server.send_to(b"OK\n", client).unwrap();
            assert_eq!(
                server.recv(&mut bytes).map(|n| &bytes[..n]).unwrap(),
                b"STATUS"
            );
            if ordering == "already" {
                server.send_to(b"state=ENABLED\n", client).unwrap();
                return;
            }
            if ordering == "disabled-before" {
                server.send_to(b"<3>AP-DISABLED", client).unwrap();
                return;
            }
            if ordering == "before" {
                server.send_to(b"<3>AP-ENABLED", client).unwrap();
            }
            server.send_to(b"state=HT_SCAN\n", client).unwrap();
            if ordering == "disabled" {
                server.send_to(b"<3>AP-DISABLED", client).unwrap();
                return;
            }
            if ordering == "after" {
                server.send_to(b"<3>AP-ENABLED", client).unwrap();
            }
            assert_eq!(
                server.recv(&mut bytes).map(|n| &bytes[..n]).unwrap(),
                b"STATUS"
            );
            server.send_to(b"state=ENABLED\n", client).unwrap();
        });
        let mut control = Control::connect(&path).unwrap();
        let result = control.wait_enabled();
        assert_eq!(
            result.is_ok(),
            !ordering.starts_with("disabled"),
            "{ordering}: {result:?}"
        );
        worker.join().unwrap();
    }
}

#[test]
fn absent_readiness_expires_without_a_polling_loop() {
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("hostapd");
    let _server = UnixDatagram::bind(&path).unwrap();
    let mut control = Control::connect(&path).unwrap();
    control.deadline = Instant::now();
    assert!(
        control
            .wait_enabled()
            .unwrap_err()
            .to_string()
            .contains("deadline")
    );
}
#[cfg(target_os = "linux")]
#[test]
fn control_socket_creation_and_startup_deadline() {
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("wlan0");
    std::thread::scope(|scope| {
        let waiter = scope.spawn(|| super::wait_socket(&path, std::time::Duration::from_secs(2)));
        let _socket = std::os::unix::net::UnixDatagram::bind(&path).unwrap();
        waiter.join().unwrap().unwrap();
        super::wait_socket(&path, std::time::Duration::ZERO).unwrap();
    });
    let error = super::wait_socket(
        &directory.path().join("missing"),
        std::time::Duration::from_millis(10),
    )
    .unwrap_err();
    assert!(
        error
            .to_string()
            .contains("did not create its control socket")
    );
}

#[test]
fn client_subscribes_before_enabling_and_preserves_interleaved_scan_and_rejection() {
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("supplicant");
    let server = UnixDatagram::bind(&path).unwrap();
    server
        .set_read_timeout(Some(Duration::from_secs(2)))
        .unwrap();
    let worker = std::thread::spawn(move || {
        let mut bytes = [0; 128];
        let (n, address) = server.recv_from(&mut bytes).unwrap();
        let client = address.as_pathname().unwrap();
        assert_eq!(&bytes[..n], b"ATTACH");
        server.send_to(b"OK\n", client).unwrap();
        assert_eq!(
            server.recv(&mut bytes).map(|n| &bytes[..n]).unwrap(),
            b"ENABLE_NETWORK 0"
        );
        server
            .send_to(b"<3>CTRL-EVENT-SCAN-RESULTS", client)
            .unwrap();
        server.send_to(b"OK\n", client).unwrap();
        assert_eq!(
            server.recv(&mut bytes).map(|n| &bytes[..n]).unwrap(),
            b"STATUS"
        );
        server.send_to(b"wpa_state=ASSOCIATING\n", client).unwrap();
        assert_eq!(
            server.recv(&mut bytes).map(|n| &bytes[..n]).unwrap(),
            b"SCAN_RESULTS"
        );
        server.send_to(b"bssid / frequency / signal level / flags / ssid\n02:00:00:00:f6:d0\t2472\t-40\t[WPA2-PSK-CCMP]\tfixture\n", client).unwrap();
        // The pending scan event permits one status refresh. Rejection arrives
        // before that response and must survive the later successful retry.
        assert_eq!(
            server.recv(&mut bytes).map(|n| &bytes[..n]).unwrap(),
            b"STATUS"
        );
        server
            .send_to(b"<3>CTRL-EVENT-ASSOC-REJECT status_code=17", client)
            .unwrap();
        server
            .send_to(b"wpa_state=COMPLETED\nbssid=02:00:00:00:f6:d0\n", client)
            .unwrap();
    });
    let trace = directory.path().join("trace.jsonl");
    let mut control = Control::connect(&path).unwrap();
    control.record_to(std::fs::File::create(&trace).unwrap());
    assert!(control.wait_connected().is_ok());
    assert!(
        control
            .last_failure_event
            .as_deref()
            .unwrap()
            .contains("status_code=17")
    );
    let records: Vec<serde_json::Value> = std::fs::read_to_string(trace)
        .unwrap()
        .lines()
        .map(|line| serde_json::from_str(line).unwrap())
        .collect();
    assert!(records.iter().any(|record| record["kind"] == "SCAN_RESULTS"
        && record["message"].as_str().unwrap().contains("2472")));
    assert!(records.iter().any(|record| record["kind"] == "event"
        && record["message"].as_str().unwrap().contains("ASSOC-REJECT")));
    worker.join().unwrap();
}

#[test]
fn client_timeout_waits_for_events_without_repeated_status_requests() {
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("supplicant");
    let server = UnixDatagram::bind(&path).unwrap();
    server
        .set_read_timeout(Some(Duration::from_secs(2)))
        .unwrap();
    let worker = std::thread::spawn(move || {
        let mut bytes = [0; 128];
        let (n, address) = server.recv_from(&mut bytes).unwrap();
        let client = address.as_pathname().unwrap();
        assert_eq!(&bytes[..n], b"ATTACH");
        server.send_to(b"OK\n", client).unwrap();
        assert_eq!(
            server.recv(&mut bytes).map(|n| &bytes[..n]).unwrap(),
            b"ENABLE_NETWORK 0"
        );
        server.send_to(b"OK\n", client).unwrap();
        assert_eq!(
            server.recv(&mut bytes).map(|n| &bytes[..n]).unwrap(),
            b"STATUS"
        );
        server.send_to(b"wpa_state=SCANNING\n", client).unwrap();
        // Only teardown may arrive after this status: no periodic STATUS.
        assert_eq!(
            server.recv(&mut bytes).map(|n| &bytes[..n]).unwrap(),
            b"DETACH"
        );
    });
    let mut control = Control::connect(&path).unwrap();
    control.deadline = Instant::now() + Duration::from_millis(100);
    let error = control.wait_connected().unwrap_err();
    assert!(error.to_string().contains("deadline"));
    assert_eq!(field(&control.last_status, "wpa_state"), Some("SCANNING"));
    drop(control);
    worker.join().unwrap();
}
