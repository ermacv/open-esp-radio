use super::*;

#[cfg(target_os = "linux")]
#[test]
fn an_eio_opening_the_port_is_retried_before_espflash_connects() {
    use crate::port::{Lines, Port, Settings};
    use std::os::fd::AsRawFd as _;

    let (master, mut slave) = serialport::TTYPort::pair().unwrap();
    let path = PathBuf::from(format!("/proc/self/fd/{}", slave.as_raw_fd()));
    drop(master);
    // A lock broker forked by a concurrent test keeps the master open until it
    // closes its inherited descriptors; the slave reopens without EIO until
    // then. The last close of the master hangs the slave up, which reads
    // report as BrokenPipe instead of their timeout.
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(30);
    loop {
        match std::io::Read::read(&mut slave, &mut [0]) {
            Err(error) if error.kind() == std::io::ErrorKind::TimedOut => {
                assert!(
                    std::time::Instant::now() < deadline,
                    "the slave was not hung up"
                );
            }
            Err(error) if error.kind() == std::io::ErrorKind::BrokenPipe => break,
            other => panic!("the slave read {other:?} instead of a hang-up"),
        }
    }
    let mut calls = 0;
    let mut resets = 0;
    let result = retry_transient(
        ATTEMPTS,
        &mut || -> crate::Result<()> {
            calls += 1;
            let error = Port::open(&path, Settings::CONSOLE.lines(Lines::Kept)).unwrap_err();
            assert_eq!(error.kind(), std::io::ErrorKind::Other);
            Err(FlashFailure {
                port: path.clone(),
                operation: "connect ROM loader",
                source: error.into(),
            }
            .into())
        },
        |_, _| resets += 1,
    );
    assert!(result.is_err());
    assert_eq!(calls, ATTEMPTS);
    assert_eq!(resets, ATTEMPTS - 1);
}

#[test]
fn permanent_open_errors_and_unclassified_io_errors_are_not_retried() {
    let failures: [crate::Result<()>; 3] = [
        Err(serialport::Error::new(serialport::ErrorKind::InvalidInput, "invalid baud").into()),
        Err(std::io::Error::from(std::io::ErrorKind::PermissionDenied).into()),
        Err(std::io::Error::other("unclassified application failure").into()),
    ];
    for failure in failures {
        let mut calls = 0;
        let mut failure = Some(failure);
        let result = retry_transient(
            ATTEMPTS,
            &mut || {
                calls += 1;
                failure.take().expect("permanent failure was retried")
            },
            |_, _| panic!("permanent failure reset the device"),
        );
        assert!(result.is_err());
        assert_eq!(calls, 1);
    }
}

#[test]
fn a_link_failure_is_retried_and_an_image_failure_is_not() {
    let mut calls = 0;
    let mut resets = Vec::new();
    let written = retry_transient(
        ATTEMPTS,
        &mut || {
            calls += 1;
            if calls == 1 {
                Err(FlashFailure {
                    port: "/dev/test".into(),
                    operation: "connect ROM loader",
                    source: espflash::Error::Connection(
                        std::io::Error::new(std::io::ErrorKind::TimedOut, "waiting for ROM sync")
                            .into(),
                    )
                    .into(),
                }
                .into())
            } else {
                Ok(calls)
            }
        },
        |attempt, _| resets.push(attempt),
    );
    assert_eq!(written.unwrap(), 2);
    assert_eq!(resets, [1]);

    let mut calls = 0;
    let refused = retry_transient(
        ATTEMPTS,
        &mut || -> crate::Result<()> {
            calls += 1;
            Err(espflash::Error::InvalidElf(
                std::io::Error::from(std::io::ErrorKind::UnexpectedEof).into(),
            )
            .into())
        },
        |_, _| {},
    );
    assert!(refused.is_err());
    assert_eq!(calls, 1);
}

#[test]
fn a_link_that_keeps_failing_ends_after_the_last_attempt() {
    let mut calls = 0;
    let mut resets = 0;
    let result = retry_transient(
        ATTEMPTS,
        &mut || -> crate::Result<()> {
            calls += 1;
            Err(std::io::Error::from(std::io::ErrorKind::TimedOut).into())
        },
        |_, _| resets += 1,
    );
    assert!(result.is_err());
    assert_eq!(calls, ATTEMPTS);
    assert_eq!(resets, ATTEMPTS - 1);
}

#[test]
fn a_start_by_power_leaves_the_chip_in_its_rom() {
    assert_eq!(After::of(oer_chip_profile::Start::Reset), After::HardReset);
    assert_eq!(
        After::of(oer_chip_profile::Start::PowerOn),
        After::StayInBootloader
    );
}

#[test]
fn connection_context_preserves_the_typed_cause_and_diagnostic() {
    let error = FlashFailure {
        port: "/dev/test".into(),
        operation: "connect ROM loader",
        source: espflash::Error::Connection(
            std::io::Error::new(std::io::ErrorKind::TimedOut, "waiting for ROM sync").into(),
        )
        .into(),
    };
    assert!(
        error
            .source()
            .unwrap()
            .downcast_ref::<espflash::Error>()
            .is_some()
    );
    assert!(transient_failure(&error));
    assert!(error.to_string().contains("waiting for ROM sync"));
    assert!(error.to_string().contains("/dev/test"));
    assert!(!transient_failure(&std::io::Error::from(
        std::io::ErrorKind::PermissionDenied
    )));
}
