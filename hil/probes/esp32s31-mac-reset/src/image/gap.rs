//! The gap run (`tasks/163`): the frame that does not leave after a long
//! one (P15). In blocks, one setting a block: [`PAIRS`] pairs of a
//! 1 500-octet frame and then a short one (case C's control), the short
//! frame under the block's setting; a quiet of [`QUIET_MS`] between blocks
//! lets the receiver split its counters by block.
//!
//! Each frame carries case C's counter with the setting in bits 27:16 of its
//! round and the pair below.
use super::amsdu::AmsduParts;
use super::transmit::{ORDINARY_SLOT, TIMEOUT, await_completion, parameters, start_with};
use super::*;
use crate::case_c::{Case, byte};
use oer_chips_esp32s31_hal::types::MacLegacyRate;
use oer_portable_embassy_time::EmbassyClock;
use oer_portable_time::{Clock, Duration, Timer};

/// Pairs a setting.
const PAIRS: u32 = 520;
const QUIET_MS: u32 = 400;

/// One block's setting of the short frame.
#[derive(Clone, Copy)]
enum Setting {
    /// The pause, µs, between the long frame's completion and the start.
    Pause(u32),
    /// Contention window 15 in place of 0.
    Window,
    /// AIFSN 7 in place of 2.
    Aifsn,
    /// 24 Mb/s with its calibrated power code.
    Rate24,
    /// 6 Mb/s with the 24 Mb/s power code.
    Power,
}

const SETTINGS: [Setting; 9] = [
    Setting::Pause(0),
    Setting::Pause(100),
    Setting::Pause(300),
    Setting::Pause(1_000),
    Setting::Pause(3_000),
    Setting::Window,
    Setting::Aifsn,
    Setting::Rate24,
    Setting::Power,
];

async fn pause(micros: u32) {
    if micros == 0 {
        return;
    }
    if let Some(at) = EmbassyClock
        .now()
        .checked_add(Duration::from_micros(u64::from(micros)))
    {
        EmbassyClock.wait_until(at).await;
    }
}

/// Count one end: acknowledged, ACK timeout, other.
fn count(row: &mut [u32; 3], status: u32) {
    let at = match status {
        0 => 0,
        5 => 1,
        _ => 2,
    };
    if let Some(cell) = row.get_mut(at) {
        *cell = cell.saturating_add(1);
    }
}

pub(super) async fn series(parts: AmsduParts<'_>, access: &mut RadioAccess) -> Result<(), Failure> {
    let AmsduParts {
        frames,
        ordinary,
        station,
        peer,
        journal,
        power_24m,
    } = parts;
    mark(92);
    for (index, setting) in SETTINGS.into_iter().enumerate() {
        // Long frames' ends, then the short frames'.
        let mut long = [0_u32; 3];
        let mut short = [0_u32; 3];
        for pair in 0..PAIRS {
            let round = (index as u32) << 16 | pair;
            for case in [Case::Provenance1500, Case::Control] {
                let counter = case.counter(round);
                let length = case.length();
                let lease = frames.lease_frame(ORDINARY_SLOT, length, |offset| {
                    byte(case, station, peer, counter, offset)
                })?;
                let mut program = parameters(length, frames.power, TIMEOUT);
                if case == Case::Control {
                    match setting {
                        Setting::Pause(micros) => pause(micros).await,
                        Setting::Window => program.contention_window = 15,
                        Setting::Aifsn => program.aifsn = 7,
                        Setting::Rate24 => {
                            program.rate = MacLegacyRate::Ofdm24M;
                            program.data_power = power_24m;
                        }
                        Setting::Power => program.data_power = power_24m,
                    }
                }
                start_with(access, ordinary, lease, length, program)?;
                let ended = await_completion(access, ordinary, 0).await?;
                count(
                    if case == Case::Control {
                        &mut short
                    } else {
                        &mut long
                    },
                    ended.status,
                );
            }
        }
        journal.record(
            Kind::GapSent,
            &[
                index as u32,
                PAIRS,
                long[0],
                long[1],
                long[2],
                short[0],
                short[1],
                short[2],
                u32::from(power_24m),
                u32::from(frames.power),
            ],
        );
        pause(QUIET_MS * 1_000).await;
    }
    mark(93);
    Ok(())
}
