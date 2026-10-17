//! How many queued frames one A-MPDU carries.
//!
//! An originator aggregates the frames at the head of its queue that share
//! a TID and a recipient. [`AmpduLimits`] bounds that run by the Block Ack
//! agreement's window, the port's A-MPDU capabilities, the recipient's HT
//! A-MPDU Parameters and the access category's TXOP limit, at the rate the
//! aggregate goes out. The bound counts each subframe as its largest
//! protected encoding ([`subframe_bound`]) and includes the empty delimiters
//! required by Minimum MPDU Start Spacing between successive subframes.
//! The final subframe needs no empty delimiters. PHY-specific hardware limits
//! and vendor density rounding remain the encoder's admission responsibility.

use oer_ieee80211_lower_mac::{AmpduCapabilities, PhyRate};
use oer_ieee80211_mac::{ampdu, phy::LegacyRate};

use crate::ampdu::MAX_AMPDU_SUBFRAMES;

/// The SIFS and the compressed BlockAck that answers an aggregate, at the
/// lowest mandatory ERP-OFDM rate (6 Mb/s): the response a TXOP holds after
/// the aggregate.
pub const BLOCK_ACK_RESPONSE_MICROS: u32 =
    10 + PhyRate::Legacy(LegacyRate::Ofdm6M).max_ppdu_duration_micros(32);

/// The A-MPDU subframe of an Ethernet-II frame of `ethernet_len` octets at
/// most: delimiter, QoS data header, CCMP header, LLC/SNAP, the payload, MIC
/// and FCS, padded to four octets.
pub fn subframe_bound(ethernet_len: usize) -> Option<u32> {
    let payload = u32::try_from(ethernet_len).ok()?;
    // Checked before rounding: an unrepresentable size admits no frame.
    payload
        .checked_add(4 + 26 + 8 + 8 + 8 + 4 + 3)
        .map(|length| length & !3)
}

/// What bounds one A-MPDU to one recipient.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct AmpduLimits {
    /// The operational agreement's window.
    pub window: u16,
    pub port: AmpduCapabilities,
    /// The recipient's HT A-MPDU Parameters field: its Maximum A-MPDU
    /// Length Exponent (bits 0-1) and Minimum MPDU Start Spacing (bits 2-4).
    pub peer_ampdu_parameters: u8,
    /// The TXOP limit of the access category; `None` without one.
    pub txop_limit_micros: Option<u32>,
    /// The rate the aggregate goes out at.
    pub rate: PhyRate,
}

impl AmpduLimits {
    /// The recipient's Maximum A-MPDU Length in octets.
    pub const fn peer_max_length(&self) -> u32 {
        match self.peer_ampdu_parameters & 0x03 {
            0 => 0x1fff,
            1 => 0x3fff,
            2 => 0x7fff,
            _ => 0xffff,
        }
    }

    /// The recipient's Minimum MPDU Start Spacing, IEEE encoding 0-7.
    pub const fn min_mpdu_start_spacing(&self) -> u8 {
        (self.peer_ampdu_parameters >> 2) & 0x07
    }

    /// How many frames of `ethernet_lens`, from the first, one A-MPDU
    /// carries: none when the port sends no A-MPDU at the rate. The
    /// aggregate and its BlockAck stay within the TXOP limit; a run of
    /// fewer than two frames goes out as single MPDUs, which may exceed it.
    pub fn run(&self, ethernet_lens: impl IntoIterator<Item = usize>) -> usize {
        let Some(mut run) = self.begin() else {
            return 0;
        };
        for ethernet_len in ethernet_lens {
            if !run.admit(ethernet_len) {
                break;
            }
        }
        run.frames()
    }

    /// Begin a run whose frames the caller admits one at a time, as it takes
    /// them from its source; `None` when the port sends no A-MPDU at the
    /// rate.
    pub fn begin(&self) -> Option<AmpduRun> {
        let minimum_subframe_bytes = self
            .rate
            .minimum_mpdu_spacing_bytes(self.min_mpdu_start_spacing())?;
        self.port
            .formats
            .contains_rate(self.rate)
            .then(|| AmpduRun {
                limits: *self,
                limit: usize::from(self.window)
                    .min(usize::from(self.port.max_subframes))
                    .min(usize::from(MAX_AMPDU_SUBFRAMES)),
                maximum: self.peer_max_length().min(self.port.max_length),
                length: 0,
                previous: None,
                minimum_subframe_bytes,
                frames: 0,
            })
    }
}

/// One A-MPDU's run being built: the frames admitted so far within its
/// [`AmpduLimits`].
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct AmpduRun {
    limits: AmpduLimits,
    limit: usize,
    maximum: u32,
    length: u32,
    previous: Option<(u32, u32)>,
    minimum_subframe_bytes: u32,
    frames: usize,
}

impl AmpduRun {
    /// Admit the next frame, an Ethernet-II frame of `ethernet_len` octets:
    /// `false`, admitting nothing, when it would exceed a limit.
    pub fn admit(&mut self, ethernet_len: usize) -> bool {
        if self.frames == self.limit {
            return false;
        }
        let Some(psdu) = subframe_bound(ethernet_len).and_then(|subframe| subframe.checked_sub(4))
        else {
            return false;
        };
        let Some(delimiters) = ampdu::empty_delimiters(psdu, self.minimum_subframe_bytes) else {
            return false;
        };
        let Some(length) = ampdu::length_after_append(self.length, self.previous, psdu) else {
            return false;
        };
        if length > self.maximum
            || self.limits.txop_limit_micros.is_some_and(|txop| {
                self.limits
                    .rate
                    .max_ppdu_duration_micros(length)
                    .checked_add(BLOCK_ACK_RESPONSE_MICROS)
                    .is_none_or(|duration| duration > txop)
            })
        {
            return false;
        }
        let Some(frames) = self.frames.checked_add(1) else {
            return false;
        };
        self.length = length;
        self.previous = Some((psdu, delimiters));
        self.frames = frames;
        true
    }

    /// The frames admitted.
    pub const fn frames(&self) -> usize {
        self.frames
    }
}

#[cfg(test)]
mod tests {
    use oer_ieee80211_lower_mac::PhyFormatSet;
    use oer_ieee80211_mac::phy::{
        FecCoding, HeGiLtf, HeMcs, HeRate, HtMcs, HtRate, PpduBandwidth, SpatialStreams,
    };

    use super::*;

    fn ht_mcs7() -> PhyRate {
        PhyRate::Ht(HtRate::new(HtMcs::new(7).unwrap(), PpduBandwidth::Mhz20, false).unwrap())
    }

    fn limits() -> AmpduLimits {
        AmpduLimits {
            window: 32,
            port: AmpduCapabilities {
                max_subframes: 16,
                formats: PhyFormatSet::HT,
                max_length: 65_535,
            },
            // A Maximum A-MPDU Length of 65 535 octets, 8 µs spacing.
            peer_ampdu_parameters: 0x03 | (6 << 2),
            txop_limit_micros: None,
            rate: ht_mcs7(),
        }
    }

    #[test]
    fn the_smallest_of_window_port_and_frames_bounds_a_run() {
        let limits = limits();
        assert_eq!(limits.run([100; 20]), 16);
        assert_eq!(limits.run([100; 5]), 5);
        assert_eq!(
            AmpduLimits {
                window: 3,
                ..limits
            }
            .run([100; 20]),
            3
        );
        assert_eq!(limits.min_mpdu_start_spacing(), 6);
    }

    #[test]
    fn the_peers_length_exponent_ends_a_run() {
        // 8 191 octets hold five subframes of 1 514-octet frames.
        let limits = AmpduLimits {
            peer_ampdu_parameters: 0,
            ..limits()
        };
        assert_eq!(limits.peer_max_length(), 8_191);
        assert_eq!(subframe_bound(1_514), Some(1_572));
        assert_eq!(limits.run([1_514; 8]), 5);
    }

    #[test]
    fn the_txop_limit_holds_the_aggregate_and_its_block_ack() {
        let rate = ht_mcs7();
        let one = subframe_bound(1_514).unwrap();
        // Room for two subframes and the BlockAck, not three.
        let txop = rate.max_ppdu_duration_micros(2 * one) + BLOCK_ACK_RESPONSE_MICROS;
        let limits = AmpduLimits {
            txop_limit_micros: Some(txop),
            ..limits()
        };
        assert_eq!(limits.run([1_514; 8]), 2);
    }

    #[test]
    fn a_run_admits_frames_one_at_a_time_until_a_limit() {
        let limits = AmpduLimits {
            window: 2,
            ..limits()
        };
        let mut run = limits.begin().unwrap();
        assert!(run.admit(1_514));
        assert!(run.admit(100));
        // The window's two frames are taken; nothing more is admitted.
        assert!(!run.admit(100));
        assert_eq!(run.frames(), 2);
    }

    #[test]
    fn a_rate_the_port_does_not_aggregate_carries_no_run() {
        let limits = AmpduLimits {
            rate: PhyRate::Legacy(LegacyRate::Ofdm54M),
            ..limits()
        };
        assert_eq!(limits.run([100; 8]), 0);
        assert!(limits.begin().is_none());
    }

    #[test]
    fn aggregate_admission_must_include_minimum_start_spacing() {
        let limits = AmpduLimits {
            window: 64,
            port: AmpduCapabilities {
                max_subframes: 64,
                formats: PhyFormatSet::HT,
                max_length: 8_191,
            },
            peer_ampdu_parameters: 7 << 2,
            txop_limit_micros: None,
            rate: PhyRate::Ht(
                HtRate::new(HtMcs::new(7).unwrap(), PpduBandwidth::Mhz40, true).unwrap(),
            ),
        };
        // Each 16 us interval at 150 Mb/s needs 300 octets. The final
        // subframe needs only its own 116 octets: 27 fit, 28 do not.
        assert_eq!(limits.run([60; 64]), 27);
    }

    #[test]
    fn every_density_obeys_encoded_length_and_txop_boundaries() {
        let rates = [
            // MCS7 N_DBPS: 260 (20 MHz), 540 (40 MHz).
            (
                PhyRate::Ht(
                    HtRate::new(HtMcs::new(7).unwrap(), PpduBandwidth::Mhz20, false).unwrap(),
                ),
                260,
                4_000,
            ),
            (
                PhyRate::Ht(
                    HtRate::new(HtMcs::new(7).unwrap(), PpduBandwidth::Mhz20, true).unwrap(),
                ),
                260,
                3_600,
            ),
            (
                PhyRate::Ht(
                    HtRate::new(HtMcs::new(7).unwrap(), PpduBandwidth::Mhz40, false).unwrap(),
                ),
                540,
                4_000,
            ),
            (
                PhyRate::Ht(
                    HtRate::new(HtMcs::new(7).unwrap(), PpduBandwidth::Mhz40, true).unwrap(),
                ),
                540,
                3_600,
            ),
            // HE20 MCS11: 234 data subcarriers, 10 bits, coding rate 5/6.
            (
                PhyRate::He(
                    HeRate::new(
                        HeMcs::new(11).unwrap(),
                        SpatialStreams::new(1).unwrap(),
                        PpduBandwidth::Mhz20,
                        HeGiLtf::Ltf2xGi800Ns,
                        FecCoding::Ldpc,
                        false,
                    )
                    .unwrap(),
                ),
                1_950,
                13_600,
            ),
        ];
        for (rate, bits_per_symbol, symbol_ns) in rates {
            for (encoding, spacing_ns) in [0, 250, 500, 1_000, 2_000, 4_000, 8_000, 16_000]
                .into_iter()
                .enumerate()
            {
                // Independently append empty delimiters until enough time
                // has elapsed, then append the final delimited PSDU.
                let mut interval = subframe_bound(60).unwrap();
                while interval * 8 * symbol_ns < bits_per_symbol * spacing_ns {
                    interval += 4;
                }
                let encoded_two = interval + subframe_bound(60).unwrap();
                let mut limits = AmpduLimits {
                    peer_ampdu_parameters: 3 | ((encoding as u8) << 2),
                    port: AmpduCapabilities {
                        max_subframes: 64,
                        formats: PhyFormatSet::HT.union(PhyFormatSet::HE),
                        max_length: encoded_two,
                    },
                    rate,
                    ..limits()
                };
                assert_eq!(limits.run([60; 3]), 2, "{rate:?} {encoding}");
                limits.port.max_length -= 1;
                assert_eq!(limits.run([60; 3]), 1, "{rate:?} {encoding}");
                limits.port.max_length = 65_535;
                let txop = rate.max_ppdu_duration_micros(encoded_two) + BLOCK_ACK_RESPONSE_MICROS;
                limits.txop_limit_micros = Some(txop);
                let three_duration = rate.max_ppdu_duration_micros(encoded_two + interval)
                    + BLOCK_ACK_RESPONSE_MICROS;
                let expected = if three_duration <= txop { 3 } else { 2 };
                assert_eq!(limits.run([60; 3]), expected, "{rate:?} {encoding}");
                limits.txop_limit_micros = Some(txop - 1);
                // One and two very short HE MPDUs can occupy the same
                // symbol; that boundary then excludes even the first.
                let one_fits = rate.max_ppdu_duration_micros(subframe_bound(60).unwrap())
                    + BLOCK_ACK_RESPONSE_MICROS
                    < txop;
                assert_eq!(
                    limits.run([60; 3]),
                    usize::from(one_fits),
                    "{rate:?} {encoding}"
                );
            }
        }
    }

    #[test]
    fn unrepresentable_subframes_leave_admission_unchanged() {
        let mut run = limits().begin().unwrap();
        assert_eq!(subframe_bound(usize::MAX), None);
        assert_eq!(subframe_bound(u32::MAX as usize), None);
        assert!(!run.admit(usize::MAX));
        assert_eq!(run.frames(), 0);
        assert!(run.admit(100));
        assert!(!run.admit(u32::MAX as usize));
        assert_eq!(run.frames(), 1);
        assert!(run.admit(100));
        assert_eq!(run.frames(), 2);
    }
}
