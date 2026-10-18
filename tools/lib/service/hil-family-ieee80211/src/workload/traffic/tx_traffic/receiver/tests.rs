use super::*;

fn pair() -> (UdpSocket, UdpSocket) {
    let receiver = UdpSocket::bind((Ipv4Addr::LOCALHOST, 0)).unwrap();
    let sender = UdpSocket::bind((Ipv4Addr::LOCALHOST, 0)).unwrap();
    sender.connect(receiver.local_addr().unwrap()).unwrap();
    (receiver, sender)
}
fn saved(output: &Path) -> serde_json::Value {
    serde_json::from_slice(&fs::read(output.join("test-reception.json")).unwrap()).unwrap()
}

#[test]
fn complete_delivery_waits_for_finished_but_not_for_the_workload_deadline() {
    let output = tempfile::tempdir().unwrap();
    let (socket, sender) = pair();
    sender.send(&0_u32.to_be_bytes()).unwrap();
    let receiver = Receiver::start(
        &socket,
        Ipv4Addr::LOCALHOST,
        Duration::from_secs(30),
        output.path(),
        "test",
    )
    .unwrap();
    let started = Instant::now();
    let bursts = receiver.finish(Some(1)).unwrap();
    assert!(started.elapsed() < Duration::from_secs(1));
    assert_eq!(bursts[0].datagrams, 1);
    assert_eq!(saved(output.path())["completion"], "delivered");
    // A clone's receive handling must not change the concurrent send contract.
    let flags = rustix::fs::fcntl_getfl(&socket).unwrap();
    assert!(!flags.contains(rustix::fs::OFlags::NONBLOCK));
}

#[test]
fn missing_packets_are_a_delivery_deadline_with_preserved_progress() {
    let output = tempfile::tempdir().unwrap();
    let (socket, sender) = pair();
    sender.send(&0_u32.to_be_bytes()).unwrap();
    let mut receiver = Receiver::start(
        &socket,
        Ipv4Addr::LOCALHOST,
        Duration::from_secs(30),
        output.path(),
        "test",
    )
    .unwrap();
    receiver.signal(Stop::Finished {
        expected: Some(2),
        deadline: Instant::now(),
    });
    let bursts = receiver.join().unwrap();
    assert_eq!(bursts[0].datagrams, 1);
    let value = saved(output.path());
    assert_eq!(value["completion"], "delivery-deadline");
    assert_eq!(value["expected_datagrams"], 2);
}

#[test]
fn abandonment_wakes_an_idle_receiver_and_preserves_the_reason() {
    let output = tempfile::tempdir().unwrap();
    let (socket, _) = pair();
    let receiver = Receiver::start(
        &socket,
        Ipv4Addr::LOCALHOST,
        Duration::from_secs(30),
        output.path(),
        "test",
    )
    .unwrap();
    let started = Instant::now();
    drop(receiver);
    assert!(started.elapsed() < Duration::from_secs(1));
    assert_eq!(saved(output.path())["completion"], "aborted");
}

#[test]
fn unavailable_target_result_does_not_turn_partial_delivery_into_success() {
    let output = tempfile::tempdir().unwrap();
    let (socket, sender) = pair();
    sender.send(&0_u32.to_be_bytes()).unwrap();
    let receiver = Receiver::start(
        &socket,
        Ipv4Addr::LOCALHOST,
        Duration::from_secs(30),
        output.path(),
        "test",
    )
    .unwrap();
    assert_eq!(receiver.finish(None).unwrap()[0].datagrams, 1);
    assert_eq!(saved(output.path())["completion"], "target-unavailable");
}

#[cfg(target_os = "linux")]
#[test]
fn signal_cancels_idle_collector_without_a_manual_completion_wake() {
    const CHILD: &str = "OER_UDP_CANCEL_TEST";
    if let Some(directory) = std::env::var_os(CHILD) {
        use std::io::Write;
        let _signals = oer_process::install_signal_handlers().unwrap();
        let (socket, _) = pair();
        let mut receiver = Receiver::start(
            &socket,
            Ipv4Addr::LOCALHOST,
            Duration::from_secs(30),
            Path::new(&directory),
            "test",
        )
        .unwrap();
        println!("udp-cancel-ready");
        std::io::stdout().flush().unwrap();
        let error = receiver.join().unwrap_err();
        assert!(oer_process::is_cancelled(&*error));
        assert_eq!(saved(Path::new(&directory))["completion"], "cancelled");
        return;
    }
    use std::io::{BufRead, BufReader};
    for signal in [rustix::process::Signal::INT, rustix::process::Signal::TERM] {
        let directory = tempfile::tempdir().unwrap();
        let mut child = std::process::Command::new(std::env::current_exe().unwrap())
            .args(["--exact", "workload::traffic::tx_traffic::receiver::tests::signal_cancels_idle_collector_without_a_manual_completion_wake", "--nocapture"])
            .env(CHILD, directory.path()).stdout(std::process::Stdio::piped()).spawn().unwrap();
        let mut stdout = BufReader::new(child.stdout.take().unwrap());
        let mut line = String::new();
        loop {
            line.clear();
            assert_ne!(
                stdout.read_line(&mut line).unwrap(),
                0,
                "child failed before readiness"
            );
            if line.trim() == "udp-cancel-ready" {
                break;
            }
        }
        rustix::process::kill_process(rustix::process::Pid::from_child(&child), signal).unwrap();
        let deadline = Instant::now() + Duration::from_secs(2);
        loop {
            if let Some(status) = child.try_wait().unwrap() {
                assert!(status.success());
                break;
            }
            if Instant::now() >= deadline {
                child.kill().unwrap();
                child.wait().unwrap();
                panic!("signal did not wake the UDP reactor");
            }
            thread::sleep(Duration::from_millis(10));
        }
    }
}

#[test]
fn delayed_probe_replies_do_not_become_measured_packets() {
    let output = tempfile::tempdir().unwrap();
    let (socket, sender) = pair();
    let probe = oer_hil_protocol::network::UdpProbe {
        nonce: 42,
        response: true,
    };
    sender.send(&probe.encode()).unwrap();
    sender.send(&0_u32.to_be_bytes()).unwrap();
    sender.send(&probe.encode()).unwrap();
    sender.send(&1_u32.to_be_bytes()).unwrap();
    let receiver = Receiver::start(
        &socket,
        Ipv4Addr::LOCALHOST,
        Duration::from_secs(30),
        output.path(),
        "test",
    )
    .unwrap();
    let bursts = receiver.finish(Some(2)).unwrap();
    assert_eq!(bursts[0].datagrams, 2);
    assert_eq!(bursts[0].highest_sequence, 1);
    assert_eq!(saved(output.path())["received_unique_datagrams"], 2);
}

#[cfg(target_os = "linux")]
#[test]
fn kernel_overflow_invalidates_delivery_as_an_infrastructure_failure() {
    let output = tempfile::tempdir().unwrap();
    let (socket, sender) = pair();
    rustix::net::sockopt::set_socket_recv_buffer_size(&socket, 4096).unwrap();
    let before = oer_hil_net_traffic::udp::kernel_drops(&socket).unwrap();
    // Deliberately suspend collection across a burst. This exercises the real
    // kernel failure boundary independently of the radio and serial protocol.
    for sequence in 0_u32..256 {
        let mut packet = [0; 1472];
        packet[..4].copy_from_slice(&sequence.to_be_bytes());
        sender.send(&packet).unwrap();
    }
    let poll = EventPoll::new().unwrap();
    poll.register(&socket, false).unwrap();
    let stop = Arc::new(Mutex::new(Some(Stop::Finished {
        expected: Some(256),
        deadline: Instant::now(),
    })));
    let error = collect(
        socket,
        Ipv4Addr::LOCALHOST,
        Instant::now(),
        stop,
        poll,
        ReceptionOutput {
            directory: output.path().to_owned(),
            label: String::from("test"),
            drops_before: before,
        },
        None,
    )
    .unwrap_err();
    assert_eq!(
        oer_hil_workload::failure::classify(&*error).kind,
        oer_hil_run_bundle_format::run::FailureKind::Infrastructure
    );
    let record = saved(output.path());
    assert_eq!(record["completion"], "host-overflow");
    assert!(record["host_kernel_drops"].as_u64().unwrap() > 0);
    assert!(record["received_unique_datagrams"].as_u64().unwrap() > 0);
}

#[test]
fn pause_trigger_requires_measured_unique_datagrams() {
    let output = tempfile::tempdir().unwrap();
    let (socket, sender) = pair();
    oer_hil_net_traffic::udp::configure_qualification_receive_buffer(&socket).unwrap();
    let receiver = Receiver::start(
        &socket,
        Ipv4Addr::LOCALHOST,
        Duration::from_secs(5),
        output.path(),
        "test",
    )
    .unwrap();
    assert!(receiver.wait_started(Duration::ZERO).is_err());
    for sequence in 0..256_u32 {
        sender.send(&sequence.to_be_bytes()).unwrap();
    }
    assert!(receiver.wait_started(Duration::from_secs(2)).unwrap() >= 256);
    let bursts = receiver.finish(Some(256)).unwrap();
    assert_eq!(bursts[0].datagrams, 256);
}
