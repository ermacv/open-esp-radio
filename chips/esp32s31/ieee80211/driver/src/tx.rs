//! Executor-independent platform ports and resources for ESP32-S31 Wi-Fi TX.
//!
//! The ordinary TX owner may await these ports, but this module does not pick
//! an executor, timer driver or entropy peripheral. It also translates the
//! owned PHY calibration profile into the narrow power lookup consumed by the
//! Wi-Fi descriptor path without exposing the vendor parameter image.

use core::pin::Pin;

use crate::mac::{tx::TxSlot, tx::runtime::WifiTxRuntimePolicy};
use oer_esp32s31_phy::PhyTxTargetPowerProfile;
use oer_radio_regulatory::TxCeiling;

/// State of one finite ESP32-S31 Wi-Fi TX transaction.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum WifiTxProgress {
    /// DMA, acknowledgement or a bounded retry is still in flight.
    Pending,
    /// Hardware no longer owns the TX descriptor or its frame.
    Complete,
}

/// Reason for inspecting one active TX transaction.
///
/// The executor decides how either edge is produced. The transaction owner
/// consumes only this value and therefore remains independent of Embassy.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum WifiTxWake {
    /// A coalesced completion, hardware-timeout or collision interrupt fired.
    Interrupt { events: u32 },
    /// The transaction's external deadline expired without a decisive IRQ.
    Deadline,
}

/// Platform entropy input used only for bounded EDCA slot selection.
pub trait WifiTxEntropy {
    fn next_u32(&mut self) -> u32;
}

impl<F: FnMut() -> u32> WifiTxEntropy for F {
    fn next_u32(&mut self) -> u32 {
        self()
    }
}

/// Calibrated MAC power lookup without exposing a vendor parameter image.
pub trait WifiTxPowerProfile {
    fn power_pair(&self, rate_code: u8) -> WifiTxPowerPair;
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct WifiTxPowerPair {
    pub primary: i8,
    pub alternate: i8,
}

impl WifiTxPowerProfile for PhyTxTargetPowerProfile {
    fn power_pair(&self, rate_code: u8) -> WifiTxPowerPair {
        let pair = self.pair(rate_code);
        WifiTxPowerPair {
            primary: pair.primary,
            alternate: pair.alternate,
        }
    }
}

/// Finite policy for management and EAPOL frames sent before connected IRQ
/// scheduling owns the ordinary descriptor.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct ControlTxConfig {
    /// Executor watchdog for each hardware publication.
    pub completion_timeout: oer_time::Duration,
    /// Cooperative polling interval before the MAC IRQ owner is installed.
    pub poll_interval: oer_time::Duration,
}

/// Resources whose ownership must stay together for every ordinary TX phase.
///
/// Protocol owners add their own finite configuration instead of exposing a
/// growing positional constructor whenever another runtime or hardware port
/// is introduced.
pub struct WifiTxResources<'slot, P, E, T, const BUFFER_SIZE: usize> {
    pub slot: Pin<&'slot mut TxSlot<BUFFER_SIZE>>,
    pub policy: WifiTxRuntimePolicy,
    pub power: P,
    pub entropy: E,
    pub timer: T,
}

/// A calibrated lookup bounded by the same conducted ceiling that the radio
/// client installs in the PHY profile. Every publication and retry, including
/// its control rate, goes through this lookup.
#[derive(Clone, Copy, Debug)]
pub struct CappedTxPowerProfile<P> {
    calibrated: P,
    ceiling: TxCeiling,
}

impl<P> CappedTxPowerProfile<P> {
    pub const fn new(calibrated: P, ceiling: TxCeiling) -> Self {
        Self {
            calibrated,
            ceiling,
        }
    }
}

impl<P: WifiTxPowerProfile> WifiTxPowerProfile for CappedTxPowerProfile<P> {
    fn power_pair(&self, rate_code: u8) -> WifiTxPowerPair {
        capped_power(
            self.calibrated.power_pair(rate_code),
            Some(self.ceiling.dbm()),
        )
    }
}

/// Bound a calibrated power pair by a caller's ceiling.
///
/// The S31 power code is the vendor's quarter-dBm target arithmetic-shifted
/// right by two (`phy/src/tx/power.rs`, `PhyTxTargetPowerProfile::pair`,
/// after `phy_get_max_pwr`), so one code step is one dBm of the calibrated
/// target and a ceiling in dBm compares with it directly. The codes are
/// the vendor's table convention, not a measured radiated power.
pub(crate) const fn capped_power(pair: WifiTxPowerPair, ceiling: Option<i8>) -> WifiTxPowerPair {
    match ceiling {
        None => pair,
        Some(ceiling) => WifiTxPowerPair {
            primary: if pair.primary < ceiling {
                pair.primary
            } else {
                ceiling
            },
            alternate: if pair.alternate < ceiling {
                pair.alternate
            } else {
                ceiling
            },
        },
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use oer_ieee80211_wire::channel::{WifiChannel, WifiChannelWidth};
    use oer_radio_regulatory::{Country, PowerLimits, RegulatoryDomain};

    struct Calibrated;
    impl WifiTxPowerProfile for Calibrated {
        fn power_pair(&self, rate: u8) -> WifiTxPowerPair {
            WifiTxPowerPair {
                primary: rate as i8 - 12,
                alternate: 21,
            }
        }
    }

    #[test]
    fn domain_antenna_chip_and_lab_cap_bound_every_rate_and_both_codes() {
        let domain = RegulatoryDomain::of(Country::new(*b"DE").expect("country")).expect("domain");
        let channel = domain
            .permit(oer_ieee80211_port::RegulatedChannel::from(
                WifiChannel::new_2_4_ghz(1, WifiChannelWidth::Mhz20).expect("channel"),
            ))
            .expect("permitted");
        for (eirp_gain, chip, owner) in [
            (233, 2100, Some(1000)),
            (1233, 2100, None),
            (233, 600, None),
        ] {
            let ceiling = channel.tx_ceiling(PowerLimits {
                antenna_gain_mbm: eirp_gain,
                chip_maximum_mbm: chip,
                owner_cap_mbm: owner,
            });
            let profile = CappedTxPowerProfile::new(Calibrated, ceiling);
            for rate in 0..=42 {
                let original = Calibrated.power_pair(rate);
                let pair = profile.power_pair(rate);
                assert_eq!(pair.primary, original.primary.min(ceiling.dbm()));
                assert_eq!(pair.alternate, original.alternate.min(ceiling.dbm()));
                assert!(i32::from(pair.primary) * 100 <= ceiling.mbm());
                assert!(i32::from(pair.alternate) * 100 <= ceiling.mbm());
            }
        }
    }
}
