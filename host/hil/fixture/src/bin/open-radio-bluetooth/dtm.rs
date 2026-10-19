//! Fixed DTM command checks. The owner retains all Reset and restoration duties.

use super::{
    Result,
    hci::Socket,
    model::{Check, DtmProfile, DtmVersion},
};
use bt_hci::cmd::{
    controller_baseband::Reset,
    info::ReadLocalSupportedCmds,
    le::{LeReceiverTest, LeReceiverTestV2, LeTestEnd, LeTransmitterTestV2},
};
use std::time::Duration;

// bt-hci 0.10.1 assigns LeTransmitterTest OCF 0x001c (Read Supported States).
// Core 5.4 Vol 4 Part E §7.8.29 specifies 0x001e for Transmitter Test [v1]:
// https://www.bluetooth.com/wp-content/uploads/Files/Specification/HTML/Core-54/out/en/host-controller-interface/host-controller-interface-functional-specification.html#UUID-15c2cfce-06a0-5da7-5cbb-45c1896cca8d
// Reuse its typed parameters and completion handling with the corrected opcode.
// The dependency macro emits an optional defmt cfg in its caller's scope.
#[allow(unexpected_cfgs)]
mod v1 {
    bt_hci::cmd! {
        LeTransmitterTestV1(LE, 0x001e) {
            Params = bt_hci::cmd::le::LeTransmitterTestParams;
            Return = ();
        }
    }
}

pub(super) fn check(user: &Socket, report: &mut Check) -> Result<()> {
    let mask = user.command(ReadLocalSupportedCmds::new())?;
    report.dtm_v1_advertised =
        mask.le_receiver_test_v1() && mask.le_transmitter_test_v1() && mask.le_test_end();
    report.dtm_v2_advertised =
        mask.le_receiver_test_v2() && mask.le_transmitter_test_v2() && mask.le_test_end();
    match report.dtm_version {
        DtmVersion::V1 if !report.dtm_v1_advertised => {
            return Err("adapter does not advertise DTM v1 RX/TX/Test End".into());
        }
        DtmVersion::V2 if !report.dtm_v2_advertised => {
            return Err("adapter does not advertise DTM v2 RX/TX/Test End".into());
        }
        DtmVersion::V1 | DtmVersion::V2 => {}
    }
    match report.profile {
        DtmProfile::ReceiveTransmit => {
            receive(user, report)?;
            report.rx_packets = Some(user.command(LeTestEnd::new())?);
            transmit(user, report)
        }
        DtmProfile::Transmit => transmit(user, report),
        DtmProfile::ReceiveSilence => {
            receive(user, report)?;
            // The Intel AX211 never completes LE Test End after a receiver
            // test that heard nothing; Reset ends the test without a count.
            user.command(Reset::new())?;
            report.rx_ended_by_reset = true;
            Ok(())
        }
    }
}

fn receive(user: &Socket, report: &mut Check) -> Result<()> {
    match report.dtm_version {
        DtmVersion::V1 => user.command(LeReceiverTest::new(0))?,
        DtmVersion::V2 => user.command(LeReceiverTestV2::new(0, 1, 0))?,
    }
    report.rx_started = true;
    oer_os_process::sleep(Duration::from_millis(100))?;
    Ok(())
}

fn transmit(user: &Socket, report: &mut Check) -> Result<()> {
    match report.dtm_version {
        DtmVersion::V1 => user.command(v1::LeTransmitterTestV1::new(
            bt_hci::cmd::le::LeTransmitterTestParams {
                tx_frequency: 0,
                length_of_test_data: 37,
                packet_payload: 0,
            },
        ))?,
        DtmVersion::V2 => user.command(LeTransmitterTestV2::new(0, 37, 0, 1))?,
    }
    report.tx_started = true;
    oer_os_process::sleep(Duration::from_millis(100))?;
    if user.command(LeTestEnd::new())? != 0 {
        return Err("transmitter Test End returned a nonzero receiver count".into());
    }
    report.tx_test_end = true;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::super::model::Adapter;
    use super::*;
    use std::os::unix::net::UnixDatagram;

    fn exchange(server: &UnixDatagram, expected: &[u8], response: &[u8]) {
        let mut request = [0; 300];
        let count = server.recv(&mut request).unwrap();
        assert_eq!(&request[..count], expected);
        server.send(response).unwrap();
    }

    fn supported(server: &UnixDatagram, v1: bool, v2: bool) {
        let mut response = vec![4, 14, 68, 1, 2, 0x10, 0];
        // Core 5.4 Vol 4 Part E §6.27 Supported Commands, independent of
        // bt-hci's command definitions. Test End is advertised in both cases.
        let mut mask = [0; 64];
        mask[28] = 0x40 | if v1 { 0x30 } else { 0 };
        mask[35] = if v2 { 0x80 } else { 0 };
        mask[36] = u8::from(v2);
        response.extend(mask);
        exchange(server, &[1, 2, 0x10, 0], &response);
    }

    fn pair() -> (Socket, UnixDatagram) {
        let (client, server) = UnixDatagram::pair().unwrap();
        client.set_nonblocking(true).unwrap();
        server
            .set_read_timeout(Some(Duration::from_secs(2)))
            .unwrap();
        (Socket(client.into()), server)
    }

    #[test]
    fn selected_profile_drives_real_command_bytes_and_preserves_counts() {
        for version in [DtmVersion::V1, DtmVersion::V2] {
            let (client, server) = pair();
            let worker = std::thread::spawn(move || {
                supported(&server, true, true);
                let (rx, tx): (&[u8], &[u8]) = match version {
                    DtmVersion::V1 => (&[1, 0x1d, 0x20, 1, 0], &[1, 0x1e, 0x20, 3, 0, 37, 0]),
                    DtmVersion::V2 => (
                        &[1, 0x33, 0x20, 3, 0, 1, 0],
                        &[1, 0x34, 0x20, 4, 0, 37, 0, 1],
                    ),
                };
                exchange(&server, rx, &[4, 14, 4, 1, rx[1], 0x20, 0]);
                exchange(
                    &server,
                    &[1, 0x1f, 0x20, 0],
                    &[4, 14, 6, 1, 0x1f, 0x20, 0, 42, 0],
                );
                exchange(&server, tx, &[4, 14, 4, 1, tx[1], 0x20, 0]);
                exchange(
                    &server,
                    &[1, 0x1f, 0x20, 0],
                    &[4, 14, 6, 1, 0x1f, 0x20, 0, 0, 0],
                );
            });
            let mut report = Check::new(Adapter(0), version, DtmProfile::ReceiveTransmit);
            check(&client, &mut report).unwrap();
            worker.join().unwrap();
            assert!(report.rx_started && report.tx_started && report.tx_test_end);
            assert_eq!(report.rx_packets, Some(42));
            assert!(!report.restored, "only the owner can establish restoration");
        }
    }

    #[test]
    fn unsupported_profile_does_not_fall_back_or_start_rf() {
        for version in [DtmVersion::V1, DtmVersion::V2] {
            let (client, server) = pair();
            let worker = std::thread::spawn(move || {
                supported(
                    &server,
                    version != DtmVersion::V1,
                    version != DtmVersion::V2,
                );
                server
            });
            let mut report = Check::new(Adapter(0), version, DtmProfile::ReceiveTransmit);
            assert!(
                check(&client, &mut report)
                    .unwrap_err()
                    .to_string()
                    .contains("does not advertise")
            );
            let server = worker.join().unwrap();
            server.set_nonblocking(true).unwrap();
            assert_eq!(
                server.recv(&mut [0; 64]).unwrap_err().kind(),
                std::io::ErrorKind::WouldBlock
            );
            assert!(!report.rx_started && !report.tx_started);
        }
    }

    #[test]
    fn rejected_rx_does_not_retry_switch_profile_or_start_tx() {
        let (client, server) = pair();
        let worker = std::thread::spawn(move || {
            supported(&server, true, true);
            exchange(
                &server,
                &[1, 0x1d, 0x20, 1, 0],
                &[4, 14, 4, 1, 0x1d, 0x20, 0x0c],
            );
            server
        });
        let mut report = Check::new(Adapter(0), DtmVersion::V1, DtmProfile::ReceiveTransmit);
        assert!(check(&client, &mut report).is_err());
        let server = worker.join().unwrap();
        server.set_nonblocking(true).unwrap();
        assert_eq!(
            server.recv(&mut [0; 64]).unwrap_err().kind(),
            std::io::ErrorKind::WouldBlock
        );
        assert!(!report.rx_started && !report.tx_started);
    }

    #[test]
    fn transmit_profile_never_starts_a_receiver() {
        let (client, server) = pair();
        let worker = std::thread::spawn(move || {
            supported(&server, true, true);
            let tx = [1, 0x34, 0x20, 4, 0, 37, 0, 1];
            exchange(&server, &tx, &[4, 14, 4, 1, 0x34, 0x20, 0]);
            exchange(
                &server,
                &[1, 0x1f, 0x20, 0],
                &[4, 14, 6, 1, 0x1f, 0x20, 0, 0, 0],
            );
        });
        let mut report = Check::new(Adapter(0), DtmVersion::V2, DtmProfile::Transmit);
        check(&client, &mut report).unwrap();
        worker.join().unwrap();
        assert!(!report.rx_started && report.tx_started && report.tx_test_end);
    }

    #[test]
    fn silent_receiver_ends_with_reset_and_reports_no_count() {
        let (client, server) = pair();
        let worker = std::thread::spawn(move || {
            supported(&server, true, true);
            exchange(
                &server,
                &[1, 0x33, 0x20, 3, 0, 1, 0],
                &[4, 14, 4, 1, 0x33, 0x20, 0],
            );
            // HCI Reset, not LE Test End.
            exchange(&server, &[1, 3, 0x0c, 0], &[4, 14, 4, 1, 3, 0x0c, 0]);
        });
        let mut report = Check::new(Adapter(0), DtmVersion::V2, DtmProfile::ReceiveSilence);
        check(&client, &mut report).unwrap();
        worker.join().unwrap();
        assert!(report.rx_started && report.rx_ended_by_reset);
        assert_eq!(report.rx_packets, None);
        assert!(!report.tx_started);
    }
}
