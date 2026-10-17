use bt_hci::{
    FromHciBytes, WriteHci,
    cmd::{
        Cmd,
        le::{LeSetScanEnable, LeSetScanParams},
    },
    event::{CommandComplete, CommandCompleteWithStatus, Event, le::LeEvent},
    param::{
        AddrKind, BdAddr, Duration, Error as HciError, LeAdvEventKind, LeScanKind,
        ScanningFilterPolicy,
    },
};

use super::{
    LeLegacyAdvertisingReportEvent, LeLegacyAdvertisingReportEventError, LeLegacyScanningCommand,
    LeLegacyScanningConfiguration, LeLegacyScanningConfigurationCommand,
    LeLegacyScanningDecodeError, LeLegacyScanningDuplicatePolicy, LeLegacyScanningEnableCommand,
    LeLegacyScanningParametersMissing,
};
use crate::HciCommandPacket;

fn decode<C>(command: &C) -> Result<LeLegacyScanningCommand, LeLegacyScanningDecodeError>
where
    C: Cmd,
{
    let mut encoded = [0_u8; 16];
    let length = command.params().size();
    command
        .params()
        .write_hci(&mut &mut encoded[..length])
        .expect("the standard parameters fit their declared size");
    LeLegacyScanningCommand::decode(HciCommandPacket::new(C::OPCODE, &encoded[..length]))
}

#[test]
fn standard_passive_parameters_become_owned_timing() {
    let command = LeSetScanParams::new(
        LeScanKind::Passive,
        Duration::from_u16(0x20),
        Duration::from_u16(0x10),
        AddrKind::PUBLIC,
        ScanningFilterPolicy::BasicUnfiltered,
    );
    let LeLegacyScanningCommand::SetParameters(parameters) =
        decode(&command).expect("the supported standard parameters decode")
    else {
        panic!("parameters changed semantic command kind");
    };
    assert_eq!(parameters.interval_units_625_us(), 0x20);
    assert_eq!(parameters.window_units_625_us(), 0x10);
}

#[test]
fn standard_enable_retains_duplicate_policy() {
    for (filter_duplicates, expected) in [
        (false, LeLegacyScanningDuplicatePolicy::ReportAll),
        (true, LeLegacyScanningDuplicatePolicy::FilterDuplicates),
    ] {
        let LeLegacyScanningCommand::SetEnable(enable) =
            decode(&LeSetScanEnable::new(true, filter_duplicates))
                .expect("the standard Enable command decodes")
        else {
            panic!("Enable changed semantic command kind");
        };
        assert!(enable.enable());
        assert_eq!(enable.duplicate_policy(), expected);
    }
}

#[test]
fn unsupported_profiles_and_invalid_timing_fail_closed() {
    let Ok(LeLegacyScanningCommand::SetParameters(active)) = decode(&LeSetScanParams::new(
        LeScanKind::Active,
        Duration::from_u16(0x20),
        Duration::from_u16(0x10),
        AddrKind::PUBLIC,
        ScanningFilterPolicy::BasicUnfiltered,
    )) else {
        panic!("active scanning parameters decode");
    };
    assert!(active.is_active());
    assert!(!active.is_accept_list_only());

    let Ok(LeLegacyScanningCommand::SetParameters(filtered)) = decode(&LeSetScanParams::new(
        LeScanKind::Passive,
        Duration::from_u16(0x20),
        Duration::from_u16(0x10),
        AddrKind::PUBLIC,
        ScanningFilterPolicy::BasicFiltered,
    )) else {
        panic!("accept-list scanning parameters decode");
    };
    assert!(filtered.is_accept_list_only());

    for command in [
        LeSetScanParams::new(
            LeScanKind::Passive,
            Duration::from_u16(0x20),
            Duration::from_u16(0x10),
            AddrKind::RANDOM,
            ScanningFilterPolicy::BasicUnfiltered,
        ),
        LeSetScanParams::new(
            LeScanKind::Passive,
            Duration::from_u16(0x20),
            Duration::from_u16(0x10),
            AddrKind::PUBLIC,
            ScanningFilterPolicy::ExtUnfiltered,
        ),
        LeSetScanParams::new(
            LeScanKind::Passive,
            Duration::from_u16(0x20),
            Duration::from_u16(0x10),
            AddrKind::PUBLIC,
            ScanningFilterPolicy::ExtFiltered,
        ),
    ] {
        let response = decode(&command)
            .expect_err("the unsupported role must not become Controller intent")
            .into_command_complete()
            .expect("the opcode belongs to scanning");
        assert_eq!(response.status(), HciError::UNSUPPORTED.to_status());
    }

    for command in [
        LeSetScanParams::new(
            LeScanKind::Passive,
            Duration::from_u16(3),
            Duration::from_u16(3),
            AddrKind::PUBLIC,
            ScanningFilterPolicy::BasicUnfiltered,
        ),
        LeSetScanParams::new(
            LeScanKind::Passive,
            Duration::from_u16(8),
            Duration::from_u16(9),
            AddrKind::PUBLIC,
            ScanningFilterPolicy::BasicUnfiltered,
        ),
    ] {
        let response = decode(&command)
            .expect_err("invalid scan timing must not become Controller intent")
            .into_command_complete()
            .expect("the opcode belongs to scanning");
        assert_eq!(
            response.status(),
            HciError::INVALID_HCI_PARAMETERS.to_status()
        );
    }
}

#[test]
fn malformed_standard_parameter_body_is_rejected_by_bt_hci() {
    let error =
        LeLegacyScanningCommand::decode(HciCommandPacket::new(LeSetScanEnable::OPCODE, &[1]))
            .expect_err("a truncated standard command must fail closed");
    let response = error
        .into_command_complete()
        .expect("the opcode belongs to scanning");
    assert_eq!(
        response.status(),
        HciError::INVALID_HCI_PARAMETERS.to_status()
    );
}

#[test]
fn rejection_completion_roundtrips_through_bt_hci() {
    let response =
        LeLegacyScanningCommand::decode(HciCommandPacket::new(LeSetScanEnable::OPCODE, &[2, 0]))
            .expect_err("bt-hci rejects an invalid bool")
            .into_command_complete()
            .expect("the opcode belongs to scanning");
    let complete = CommandComplete::from_hci_bytes_complete(&response.as_bytes()[2..])
        .expect("the event parameters decode through bt-hci");
    let complete: CommandCompleteWithStatus<'_> = complete
        .try_into()
        .expect("the completion carries a status");
    assert_eq!(complete.cmd_opcode, LeSetScanEnable::OPCODE);
    assert_eq!(
        complete.status,
        HciError::INVALID_HCI_PARAMETERS.to_status()
    );
    assert_eq!(response.status(), complete.status);
}

#[test]
fn reset_scoped_configuration_freezes_an_enable_snapshot() {
    let parameters = decode(&LeSetScanParams::new(
        LeScanKind::Passive,
        Duration::from_u16(0x20),
        Duration::from_u16(0x10),
        AddrKind::PUBLIC,
        ScanningFilterPolicy::BasicUnfiltered,
    ))
    .expect("the fixture parameters decode");
    let parameters = LeLegacyScanningConfigurationCommand::from_command(parameters)
        .expect("Set Parameters is configuration");
    let mut configuration = LeLegacyScanningConfiguration::new();
    let enable = LeLegacyScanningEnableCommand {
        enable: true,
        duplicate_policy: LeLegacyScanningDuplicatePolicy::FilterDuplicates,
    };
    assert_eq!(
        configuration.enable_request(enable),
        Err(LeLegacyScanningParametersMissing)
    );
    configuration.configure(parameters);
    let request = configuration
        .enable_request(enable)
        .expect("configured Enable takes a snapshot");
    assert_eq!(request.parameters(), parameters.parameters());
    assert_eq!(
        request.duplicate_policy(),
        LeLegacyScanningDuplicatePolicy::FilterDuplicates
    );
}

#[test]
fn single_report_event_roundtrips_through_bt_hci() {
    let event = LeLegacyAdvertisingReportEvent::new(
        LeAdvEventKind::AdvNonconnInd,
        AddrKind::RANDOM,
        BdAddr::new([1, 2, 3, 4, 5, 0xc6]),
        &[2, 1, 6],
        -71,
    )
    .expect("the legacy report fits its standard event");
    let Event::Le(LeEvent::LeAdvertisingReport(event)) =
        Event::from_hci_bytes_complete(event.as_bytes())
            .expect("bt-hci decodes the emitted complete event")
    else {
        panic!("the emitted event changed standard kind");
    };
    assert_eq!(event.reports.len(), 1);
    let report = event
        .reports
        .iter()
        .next()
        .expect("one report was declared")
        .expect("the report fields decode");
    assert_eq!(report.event_kind, LeAdvEventKind::AdvNonconnInd);
    assert_eq!(report.addr_kind, AddrKind::RANDOM);
    assert_eq!(report.addr, BdAddr::new([1, 2, 3, 4, 5, 0xc6]));
    assert_eq!(report.data, [2, 1, 6]);
    assert_eq!(report.rssi, -71);
}

#[test]
fn report_event_rejects_unrepresentable_legacy_fields() {
    assert_eq!(
        LeLegacyAdvertisingReportEvent::new(
            LeAdvEventKind::AdvInd,
            AddrKind::PUBLIC,
            BdAddr::default(),
            &[0; 32],
            0,
        ),
        Err(LeLegacyAdvertisingReportEventError::DataTooLong { length: 32 })
    );
    assert_eq!(
        LeLegacyAdvertisingReportEvent::new(
            LeAdvEventKind::AdvInd,
            AddrKind::RESOLVABLE_PRIVATE_OR_PUBLIC,
            BdAddr::default(),
            &[],
            0,
        ),
        Err(LeLegacyAdvertisingReportEventError::UnsupportedAddressKind(
            AddrKind::RESOLVABLE_PRIVATE_OR_PUBLIC,
        ))
    );
}
