use super::*;
use oer_network_port::LinkSnapshot;

const BSSID: [u8; 6] = [2, 0x11, 0x22, 0x33, 0x44, 0x55];

fn code(error: StationError) -> u32 {
    match error {
        StationError::Failed { code } => code,
        other => panic!("not a failure: {other:?}"),
    }
}

#[test]
fn a_connect_that_completed_answers_the_join_with_its_access_point() {
    assert_eq!(
        joined(&CommandOutcome::Connected {
            bssid: BSSID,
            channel: 6
        }),
        Joined::Linked {
            bssid: BSSID,
            channel: 6
        }
    );
}

#[test]
fn a_failed_connect_numbers_its_reason_and_the_access_points_code() {
    assert_eq!(
        joined(&CommandOutcome::ConnectFailed(
            DisconnectReason::HandshakeFailed
        )),
        Joined::Refused(StationError::Failed { code: 0x0106_0000 })
    );
    assert_eq!(
        code(connect_failed(&DisconnectReason::Peer { reason_code: 15 })),
        0x0101_000f
    );
    assert_eq!(
        code(connect_failed(&DisconnectReason::NotFound {
            partial: true
        })),
        0x0105_0001
    );
    assert_eq!(
        code(connect_failed(&DisconnectReason::Sae(
            SaeFailure::Rejected { status_code: 0x7b }
        ))),
        0x0103_007b
    );
}

#[test]
fn refusals_and_admission_failures_have_their_own_classes() {
    assert_eq!(code(refused(CommandRefusal::Transitioning)), 0x0202_0000);
    assert_eq!(code(not_admitted(ControlError::Stopped)), 0x0301_0000);
    assert_eq!(
        joined(&CommandOutcome::BackgroundScan { interrupted: None }),
        Joined::Refused(StationError::Failed { code: 0x0203_0000 })
    );
    assert_eq!(
        code(StationError::failed(class::STATION_ENDED, 5, 0x0102)),
        0x0405_0102
    );
}

#[test]
fn only_an_up_link_after_the_connection_notice_names_its_access_point() {
    let connected = Some(Notice::Connected {
        bssid: BSSID,
        channel: 11,
    });
    let up = LinkSnapshot {
        state: LinkState::Up,
        epoch: 1,
    };
    let status = |link, notice| Status {
        link,
        notice,
        stopped: false,
    };
    assert_eq!(
        link(&status(up, connected)),
        Some(Link {
            bssid: BSSID,
            channel: 11
        })
    );
    assert_eq!(link(&status(LinkSnapshot::DOWN, connected)), None);
    assert_eq!(
        link(&status(
            up,
            Some(Notice::Disconnected(DisconnectReason::LocalRequest))
        )),
        None
    );
    assert_eq!(link(&status(up, None)), None);
}
