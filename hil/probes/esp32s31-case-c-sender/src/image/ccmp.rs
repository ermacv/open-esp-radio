//! Cell E of `tasks/161`: case C's QoS cases (the control first in kind,
//! then A-MSDU of one, two short and two long subframes) protected under
//! CCMP, to the receiving rx-chain probe, which installs the same pairwise
//! key through the HAL and lets its MAC decrypt.
//!
//! Each frame is sealed here in software by the protocol layer's own CCMP
//! (`CcmpAssociation`, SPP not negotiated: the A-MSDU Present bit masked
//! out of the AAD, IEEE 802.11-2020 12.5.3.3.3), as an access point that
//! follows the standard does; the MAC sends it as it is, no hardware key
//! selected. First [`HELLO`] unprotected frames give the receiver this
//! board's address for its key. The rounds draw their order as case C's do;
//! the probe draws no backoff.
use super::amsdu::AmsduParts;
use super::transmit::{ORDINARY_SLOT, TIMEOUT, await_completion, start, tx_phase};
use super::*;
use crate::case_c::{CASES, Case, TEMPORAL_KEY, byte, order, protected_byte};
use oer_protocols_ieee80211_contracts_security::Ccmp128Key;
use oer_protocols_ieee80211_core_security_cipher::{
    aes::SoftwareAes128, ccmp_mpdu::CcmpAssociation,
};
use oer_protocols_ieee80211_wire::security::Spp;

/// Unprotected frames before the cell: the receiver learns this board's
/// address from them.
const HELLO: u32 = 40;
/// The longest protected MPDU: case 4's.
const LONGEST: usize = Case::AmsduLong.protected_length();

/// One case's ends: acknowledged, ACK timeouts, other, seal refusals.
type Ends = [u32; 4];

pub(super) async fn series(
    parts: AmsduParts<'_>,
    access: &mut RadioAccess,
) -> Result<heapless::Vec<service::CaseEnds, { service::CASES }>, Failure> {
    let AmsduParts {
        frames,
        ordinary,
        station,
        peer,
        rounds,
    } = parts;
    for hello in 0..HELLO {
        let case = Case::Provenance700;
        let counter = case.counter(hello);
        let lease = frames.lease_frame(ORDINARY_SLOT, case.length(), |offset| {
            byte(case, station, peer, counter, offset)
        })?;
        start(
            access,
            ordinary,
            lease,
            case.length(),
            frames.power,
            TIMEOUT,
        )?;
        await_completion(access, ordinary).await?;
    }
    let association = CcmpAssociation::new(Spp::NotNegotiated);
    let key = Ccmp128Key::new(&TEMPORAL_KEY);
    let mut engine = SoftwareAes128;
    // The sealed frame, kept for its one publication (a payload the frame
    // pool copies into its slot).
    let mut sealed = [0_u8; LONGEST];
    let mut ends: [Ends; CASES.len()] = [[0; 4]; CASES.len()];
    let mut pn = 0_u64;
    for round in 0..rounds {
        for case in order(round).into_iter().filter(|case| case.qos()) {
            pn = pn.saturating_add(1);
            let counter = case.counter(round);
            let length = case.protected_length();
            let at = CASES.iter().position(|each| *each == case).unwrap_or(0);
            let Some(frame) = sealed.get_mut(..length) else {
                return Err((tx_phase::FRAME, 0x10));
            };
            for (offset, cell) in frame.iter_mut().enumerate() {
                *cell = protected_byte(case, station, peer, counter, pn, offset);
            }
            if association.seal(&mut engine, &key, frame).is_err() {
                if let Some(row) = ends.get_mut(at) {
                    row[3] = row[3].saturating_add(1);
                }
                continue;
            }
            let frame: &[u8] = frame;
            let lease = frames.lease_frame(ORDINARY_SLOT, length, |offset| {
                frame.get(offset).copied().unwrap_or(0)
            })?;
            start(access, ordinary, lease, length, frames.power, TIMEOUT)?;
            let ended = await_completion(access, ordinary).await?;
            if let Some(cell) = ends.get_mut(at).and_then(|row| {
                row.get_mut(match ended.status {
                    0 => 0,
                    5 => 1,
                    _ => 2,
                })
            }) {
                *cell = cell.saturating_add(1);
            }
        }
    }
    // A seal the protocol layer refused is a failure of this probe, not
    // an end of the air's.
    if ends.iter().any(|row| row[3] != 0) {
        return Err((tx_phase::FRAME, 0x11));
    }
    let mut first = [0_u32; service::BEFORE];
    Ok(CASES
        .iter()
        .zip(ends)
        .filter(|(case, _)| case.qos())
        .map(|(case, row)| service::CaseEnds {
            tag: case.tag(),
            acknowledged: {
                first[0] = row[0];
                first
            },
            timeouts: {
                let mut timeouts = [0_u32; service::BEFORE];
                timeouts[0] = row[1];
                timeouts
            },
            other: row[2],
        })
        .collect())
}
