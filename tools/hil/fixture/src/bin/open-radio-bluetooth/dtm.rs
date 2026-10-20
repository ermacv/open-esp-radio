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
