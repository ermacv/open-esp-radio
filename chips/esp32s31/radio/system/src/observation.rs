//! The radio system's own trace event: each periodic tracking tick, reported
//! through `oer_trace::observe!` to the observer the image gives the backend.
//! Without `cfg(oer_trace)` no tick builds one.

use oer_esp32s31_phy::tracking::PhyParamTrackingOutcome;

/// What one periodic tracking tick did.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct TrackingTick {
    pub ran: TickRan,
    /// The PHY's retained temperature after a tracked tick, in PHY sensor
    /// units (no Celsius conversion); `None` when the tick did not track or
    /// the domain was not settled.
    pub temperature: Option<i16>,
    /// The tick's duration, saturating at 2^32 - 1 microseconds.
    pub micros: u32,
}

/// Whether the tick tracked.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum TickRan {
    /// Tracking was due and completed, with the branches that committed.
    Tracked(TrackingBranches),
    /// The deadline fired before any client's tracking fell due.
    NotDue,
    /// The domain could not track (not registered, RF closed, poisoned);
    /// nothing changed.
    Unavailable,
}

/// The branches of one completed tracking that committed an update, as
/// [`PhyParamTrackingOutcome`] reports them.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct TrackingBranches {
    /// The registered policy inhibited tracking: no child ran.
    pub inhibited: bool,
    pub rfpll_corrected: bool,
    pub wifi_i2c_changed: bool,
    pub wifi_tx_power: bool,
    pub bluetooth_ieee802154_tx_power: bool,
    /// The common (RX) calibration references were recalibrated.
    pub calibration_common: bool,
    /// The transmit calibration references were recalibrated.
    pub calibration_transmit: bool,
}

impl TrackingBranches {
    pub const fn of(outcome: &PhyParamTrackingOutcome) -> Self {
        Self {
            inhibited: outcome.tracking_inhibited,
            rfpll_corrected: outcome.rfpll_corrected,
            wifi_i2c_changed: outcome.wifi_i2c_changed,
            wifi_tx_power: outcome.tx_power.wifi,
            bluetooth_ieee802154_tx_power: outcome.tx_power.bluetooth_ieee802154,
            calibration_common: outcome.calibration.common,
            calibration_transmit: outcome.calibration.transmit,
        }
    }

    const fn bits(self) -> u32 {
        self.inhibited as u32
            | (self.rfpll_corrected as u32) << 1
            | (self.wifi_i2c_changed as u32) << 2
            | (self.wifi_tx_power as u32) << 3
            | (self.bluetooth_ieee802154_tx_power as u32) << 4
            | (self.calibration_common as u32) << 5
            | (self.calibration_transmit as u32) << 6
    }

    const fn from_bits(bits: u32) -> Option<Self> {
        if bits & !BRANCH_BITS != 0 {
            return None;
        }
        Some(Self {
            inhibited: bits & 1 != 0,
            rfpll_corrected: bits & 1 << 1 != 0,
            wifi_i2c_changed: bits & 1 << 2 != 0,
            wifi_tx_power: bits & 1 << 3 != 0,
            bluetooth_ieee802154_tx_power: bits & 1 << 4 != 0,
            calibration_common: bits & 1 << 5 != 0,
            calibration_transmit: bits & 1 << 6 != 0,
        })
    }
}

/// The seven branch flags.
const BRANCH_BITS: u32 = 0x7f;
/// Word 0: bits 0..2 the tick's kind, bit 2 a temperature present, bits
/// 8..15 the branches, bits 16..32 the temperature's two's complement.
const KIND_BITS: u32 = 0b11;
const TEMPERATURE_PRESENT: u32 = 1 << 2;
const BRANCH_SHIFT: u32 = 8;
const TEMPERATURE_SHIFT: u32 = 16;
const TRACKED: u32 = 1;
const NOT_DUE: u32 = 2;
const UNAVAILABLE: u32 = 3;

impl oer_trace::Event for TrackingTick {
    const KIND: oer_trace::Kind = oer_trace::Kind::new(oer_trace::Domain::Phy, 1);
    const CHANNEL: oer_trace::Channel = const {
        match oer_trace::Channel::new(oer_trace::Domain::Phy, 0) {
            Ok(channel) => channel,
            Err(_) => panic!("the tracking channel is in the PHY range"),
        }
    };

    fn encode(&self) -> [u32; 2] {
        let (kind, branches) = match self.ran {
            TickRan::Tracked(branches) => (TRACKED, branches.bits()),
            TickRan::NotDue => (NOT_DUE, 0),
            TickRan::Unavailable => (UNAVAILABLE, 0),
        };
        let temperature = match self.temperature {
            Some(value) => TEMPERATURE_PRESENT | u32::from(value as u16) << TEMPERATURE_SHIFT,
            None => 0,
        };
        [kind | temperature | branches << BRANCH_SHIFT, self.micros]
    }

    fn decode([word, micros]: [u32; 2]) -> Option<Self> {
        let branches = (word >> BRANCH_SHIFT) & 0xff;
        let ran = match word & KIND_BITS {
            TRACKED => TickRan::Tracked(TrackingBranches::from_bits(branches)?),
            NOT_DUE if branches == 0 => TickRan::NotDue,
            UNAVAILABLE if branches == 0 => TickRan::Unavailable,
            _ => return None,
        };
        let reserved = word & 0xf8;
        let raw = (word >> TEMPERATURE_SHIFT) as u16;
        let temperature = match (word & TEMPERATURE_PRESENT != 0, raw) {
            (true, raw) => Some(raw as i16),
            (false, 0) => None,
            (false, _) => return None,
        };
        (reserved == 0).then_some(Self {
            ran,
            temperature,
            micros,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use oer_trace::Event as _;

    #[test]
    fn every_tick_round_trips_and_unknown_words_are_refused() {
        let mut branches = TrackingBranches::default();
        let ticks = [
            TrackingTick {
                ran: TickRan::Tracked(branches),
                temperature: Some(-3),
                micros: 1_234,
            },
            TrackingTick {
                ran: TickRan::NotDue,
                temperature: None,
                micros: 0,
            },
            TrackingTick {
                ran: TickRan::Unavailable,
                temperature: None,
                micros: u32::MAX,
            },
            TrackingTick {
                ran: TickRan::Tracked(branches),
                temperature: Some(i16::MIN),
                micros: 7,
            },
        ];
        for tick in ticks {
            assert_eq!(TrackingTick::decode(tick.encode()), Some(tick));
        }
        for bit in 0..7 {
            branches = TrackingBranches::from_bits(1 << bit).unwrap();
            let tick = TrackingTick {
                ran: TickRan::Tracked(branches),
                temperature: Some(i16::MAX),
                micros: 1,
            };
            assert_eq!(TrackingTick::decode(tick.encode()), Some(tick));
        }
        // No kind, a reserved bit, an eighth branch, branches without a
        // tracked tick, a temperature without its present bit.
        for word in [0, 1 | 1 << 3, 1 | 1 << 15, 2 | 1 << 8, 3 | 5 << 16] {
            assert_eq!(TrackingTick::decode([word, 0]), None, "{word:#x}");
        }
    }

    #[test]
    fn the_branches_are_the_outcome_s_committed_children() {
        use oer_esp32s31_phy::tracking::{
            CalibrationProgress, PhyParamTrackRequest, TxPowerProgress,
        };
        let outcome = PhyParamTrackingOutcome {
            clients: PhyParamTrackRequest::new(true, false),
            tracking_inhibited: false,
            rfpll_corrected: true,
            wifi_i2c_changed: true,
            tx_power: TxPowerProgress {
                wifi: true,
                bluetooth_ieee802154: false,
            },
            calibration: CalibrationProgress {
                common: false,
                transmit: true,
                wifi: true,
                bluetooth_ieee802154: false,
            },
        };
        assert_eq!(
            TrackingBranches::of(&outcome),
            TrackingBranches {
                inhibited: false,
                rfpll_corrected: true,
                wifi_i2c_changed: true,
                wifi_tx_power: true,
                bluetooth_ieee802154_tx_power: false,
                calibration_common: false,
                calibration_transmit: true,
            }
        );
    }
}
