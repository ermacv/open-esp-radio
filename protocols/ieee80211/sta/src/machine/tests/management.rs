//! Full negotiated PMF exchanges with independent AESCCM/CMAC wire vectors.
use super::*;
use oer_ieee80211_mac::security::rsn::Akm;
use oer_ieee80211_rsn::{Ptk, frames::RsnTxFrame};

// Generated with Python cryptography AESCCM(tag_length=8) and AES-CMAC.
// PTK = WPA2 PRF(PMK=[7;32], LOCAL/BSSID, SNonce=[3;32], ANonce=[4;32]);
// CCMP nonce = 0x10 || BSSID || big-endian PN, AAD masks SN.
// BIP IGTK id=4, key=[0x77;16] (NEW_KEY=[0x66;16]), initial IPN=1.
const DEAUTH: &str =
    "c040000002000012345630055c11223330055c112233000014000020000000003f30f05c26540ce829d1";
const DISASSOC: &str =
    "a040000002000012345630055c11223330055c112233000014000020000000003f308bba994254b384b9";
const QUERY_REQUEST: &str =
    "d040000002000012345630055c11223330055c11223300000a000020000000006b95b1212ce85ff73d8c1d00";
const QUERY_RESPONSE: &str =
    "d040000002000012345630055c11223330055c11223300000b000020000000002466b98a523fa8d99bcb9d1c";
const QUERY_REQUEST_NEXT: &str =
    "d040000002000012345630055c11223330055c11223300000c00002000000000cdddb81459a2edf799b89e2a";
const GROUP_DEAUTH: &str =
    "c0000000ffffffffffff30055c11223330055c112233000007004c100400050000000000db3f777c91e12cc6";
const GROUP_DISASSOC: &str =
    "a0000000ffffffffffff30055c11223330055c112233000007004c10040005000000000063a38f10f91320f1";
const GROUP_ACTION: &str = "d0000000ffffffffffff30055c11223330055c11223300000302000001004c1004000500000000008999015cb0df67c2";
const GROUP_DEAUTH_NEXT: &str =
    "c0000000ffffffffffff30055c11223330055c112233000007004c10040006000000000009ec1021d7aa6cd8";
const GROUP_DEAUTH_NEW_KEY: &str =
    "c0000000ffffffffffff30055c11223330055c112233000007004c1004000200000000005bf93ec9371f8139";

fn wire(hex: &str) -> Vec<u8> {
    hex.as_bytes()
        .chunks_exact(2)
        .map(|pair| u8::from_str_radix(core::str::from_utf8(pair).unwrap(), 16).unwrap())
        .collect()
}

fn plain_disconnect(subtype: u8, reason: u16) -> Vec<u8> {
    let mut frame = std::vec![0; 26];
    frame[0] = subtype;
    frame[4..10].copy_from_slice(&LOCAL);
    frame[10..16].copy_from_slice(&BSSID);
    frame[16..22].copy_from_slice(&BSSID);
    frame[24..].copy_from_slice(&reason.to_le_bytes());
    frame
}

fn key_data(rsn: Option<&[u8]>, gtk: u8, igtk: u8) -> Vec<u8> {
    let mut plain = Vec::from(rsn.unwrap_or(&[]));
    plain.extend_from_slice(&[0xdd, 22, 0, 0x0f, 0xac, 1, 1, 0]);
    plain.extend_from_slice(&[gtk; 16]);
    plain.extend_from_slice(&[0xdd, 28, 0, 0x0f, 0xac, 9, 4, 0, 1, 0, 0, 0, 0, 0]);
    plain.extend_from_slice(&[igtk; 16]);
    if plain.len() % 8 != 0 {
        plain.push(0xdd);
    }
    while plain.len() % 8 != 0 {
        plain.push(0);
    }
    plain
}

fn associated<'id>(mint: Mint<'id, TestPools>) -> (World<'id>, Ptk, Vec<u8>) {
    let mut rsn = RSN;
    rsn[20] |= 0x80;
    let mut beacon = wpa2_beacon();
    beacon[55..].copy_from_slice(&rsn);
    let (world, ptk) = wpa2_associated_by::<2>(mint, &beacon);
    let plain = key_data(Some(&rsn), 0x99, 0x77);
    let wrapped = oer_ieee80211_rsn::aes::software_aes128_key_wrap(ptk.kek(), &plain).unwrap();
    let m3 = RsnTxFrame::<512>::message3(Akm::Psk, LOCAL, 2, ANONCE, [0; 8], wrapped.as_bytes())
        .unwrap()
        .authenticate(&ptk);
    (world, ptk, eapol_frame(m3.as_bytes()))
}

fn connected<'id>(mint: Mint<'id, TestPools>) -> (World<'id>, Ptk, Vec<u8>) {
    let (mut world, ptk, m3) = associated(mint);
    world.receive(at(33), &m3);
    assert!(world.station.is_connected());
    world.take();
    (world, ptk, m3)
}

fn rekey(ptk: &Ptk, counter: u64, gtk: u8, igtk: u8) -> Vec<u8> {
    let wrapped =
        oer_ieee80211_rsn::aes::software_aes128_key_wrap(ptk.kek(), &key_data(None, gtk, igtk))
            .unwrap();
    let message =
        RsnTxFrame::<512>::group_message1(Akm::Psk, LOCAL, counter, [0; 8], wrapped.as_bytes())
            .unwrap()
            .authenticate(ptk);
    protected_received(eapol_frame(message.as_bytes()), counter)
}

fn assert_released(world: &mut World<'_>) {
    assert!(matches!(world.station.state, State::Idle));
    assert!(world.station.link().is_none());
    assert!(world.station.keys.clear());
    assert_eq!(world.station.tickets.len(), 2);
    assert_eq!(world.station.buffers.len(), 2);
    assert!(world.station.keys.group_permission.is_some());
    assert!(world.submitted.is_empty() && world.pending_installs.is_empty());
}

#[test]
fn sa_query_deadline_overflow_disconnects_once_and_returns_every_key_and_credit() {
    installed(|mint| {
        let (mut world, _, _) = connected(mint);
        world.completed.clear();
        world.frames.clear();
        let late = Instant::from_micros(u64::MAX - 1);
        world.receive(late, &plain_disconnect(0xc0, 6));
        assert_released(&mut world);
        assert_eq!(
            world
                .take()
                .iter()
                .filter(|event| matches!(
                    event,
                    Seen::Notify(StaNotice::Disconnected(
                        StaDisconnectReason::DeadlineOverflow
                    ))
                ))
                .count(),
            1
        );
        assert!(
            world.completed.is_empty(),
            "a lifecycle exit is not a second Connect terminal"
        );
        assert!(
            !world.frames.iter().any(|frame| frame[0] == 0xd0),
            "no unschedulable SA Query is sent"
        );
        assert!(world.station.poll_deadline().is_none());
        assert!(world.station.poll_action(late).is_none());
    });
}

#[test]
fn only_authenticated_management_disconnects_end_the_negotiated_link() {
    for vector in [DEAUTH, DISASSOC, GROUP_DEAUTH, GROUP_DISASSOC] {
        installed(|mint| {
            let (mut world, _, _) = connected(mint);
            let valid = wire(vector);
            for position in [4, 10, 16, 24, valid.len() - 1] {
                let mut changed = valid.clone();
                changed[position] ^= 1;
                world.receive_verified(at(40), &changed);
                assert!(
                    world.station.is_connected(),
                    "forged frame at octet {position}"
                );
            }
            world.receive(at(50), &valid);
            assert!(!world.station.is_connected());
            assert_released(&mut world);
            assert_eq!(
                world
                    .take()
                    .iter()
                    .filter(|seen| matches!(
                        seen,
                        Seen::Notify(StaNotice::Disconnected(StaDisconnectReason::Peer {
                            reason_code: 7
                        }))
                    ))
                    .count(),
                1
            );
        });
    }
}

#[test]
fn query_requests_are_protected_and_replays_cannot_create_another_response() {
    installed(|mint| {
        let (mut world, _, m3) = connected(mint);
        let first = wire(QUERY_REQUEST);
        world.receive(at(40), &first);
        let response = world.frames.last().unwrap();
        assert_eq!(response[..2], [0xd0, 0x40]);
        assert_eq!(response[32..36], [8, 1, 0x12, 0x34]);
        let count = world.frames.len();
        world.receive(at(41), &first);
        assert_eq!(world.frames.len(), count);
        // Exact completed M3 recovery must not reset PMF's pairwise PN.
        world.receive(at(42), &m3);
        let count = world.frames.len();
        world.receive(at(43), &first);
        assert_eq!(world.frames.len(), count);
        let mut damaged = wire(QUERY_REQUEST_NEXT);
        *damaged.last_mut().unwrap() ^= 1;
        world.receive(at(44), &damaged);
        assert_eq!(world.frames.len(), count);
        world.receive(at(45), &wire(QUERY_REQUEST_NEXT));
        assert_eq!(world.frames.last().unwrap()[32..36], [8, 1, 0x22, 0x34]);
    });
}

#[test]
fn same_igtk_rekey_keeps_its_ipn_and_a_new_igtk_activates_at_the_terminal() {
    installed(|mint| {
        let (mut world, ptk, _) = connected(mint);
        world.receive(at(40), &wire(GROUP_ACTION)); // authenticates IPN 5
        world.receive_verified(at(41), &rekey(&ptk, 3, 0x55, 0x77));
        world.receive(at(42), &wire(GROUP_DEAUTH));
        assert!(
            world.station.is_connected(),
            "same IGTK did not reset IPN 5"
        );
        world.hold_key_installs = true;
        world.receive_verified(at(43), &rekey(&ptk, 4, 0x44, 0x66));
        assert!(!world.pending_installs.is_empty());
        world.receive(at(44), &wire(GROUP_DEAUTH_NEW_KEY));
        assert!(
            world.station.is_connected(),
            "new IGTK not active before terminal"
        );
        world.hold_key_installs = false;
        for input in world.pending_installs.drain(..).collect::<Vec<_>>() {
            world.station.input(at(45), input);
        }
        world.run(at(45));
        world.receive(at(46), &wire(GROUP_DEAUTH_NEXT));
        assert!(
            world.station.is_connected(),
            "old IGTK cannot disconnect after terminal"
        );
        world.receive(at(47), &wire(GROUP_DEAUTH_NEW_KEY));
        assert_released(&mut world);
    });
}

#[test]
fn a_forged_disconnect_starts_query_and_only_a_protected_matching_response_ends_it() {
    installed(|mint| {
        let (mut world, _, _) = connected(mint);
        world.receive(at(50), &plain_disconnect(0xc0, 7));
        let request = world.frames.last().unwrap();
        assert_eq!(request[..2], [0xd0, 0x40]);
        assert_eq!(request[32..34], [8, 0]);
        assert!(world.station.is_connected());
        let mut unprotected = wire(QUERY_RESPONSE);
        unprotected[1] = 0;
        world.receive_verified(at(51), &unprotected);
        assert!(world.station.poll_deadline().is_some());
        world.receive(at(52), &wire(QUERY_RESPONSE));
        assert!(world.station.poll_deadline().is_none());
        world.run(at(1100));
        assert!(world.station.is_connected());
    });
}

#[test]
fn query_timeout_and_local_leave_keep_the_pairwise_key_until_protected_tx_returns() {
    for timeout in [false, true] {
        for outcome in [
            Some(ended(End::Delivered)),
            Some(ended(End::AckTimeout)),
            Some(TxOutcome::Withdrawn(
                oer_ieee80211_lower_mac::WithdrawCause::Disabled,
            )),
            Some(TxOutcome::Indeterminate(
                oer_ieee80211_lower_mac::IndeterminateCause::HardwareTimeout,
            )),
            None,
        ] {
            installed(|mint| {
                let (mut world, _, _) = connected(mint);
                let now = if timeout {
                    world.receive(at(50), &plain_disconnect(0xa0, 6));
                    world.station.input(at(1074), StaInput::Tick);
                    at(1074)
                } else {
                    world
                        .station
                        .command(at(50), StaRequest::new(8, StaCommand::Disconnect))
                        .unwrap();
                    at(50)
                };
                let Some(StaAction::TransmitProtected { transmit, .. }) =
                    world.station.poll_action(now)
                else {
                    panic!("protected Deauthentication");
                };
                world.transmitted(transmit, true);
                assert_eq!(world.frames.last().unwrap()[..2], [0xc0, 0x40]);
                assert!(matches!(world.station.keys.pairwise, KeyCell::Installed(_)));
                assert!(world.station.poll_action(now).is_none());
                match outcome {
                    Some(outcome) => world.outcomes.push_back((outcome, true)),
                    None => world.unsend_next = true,
                }
                world.end_transmissions(now);
                world.run(now);
                assert_released(&mut world);
                assert_eq!(
                    world
                        .take()
                        .iter()
                        .filter(|seen| matches!(seen, Seen::Notify(StaNotice::Disconnected(_))))
                        .count(),
                    1
                );
            });
        }
    }
}
