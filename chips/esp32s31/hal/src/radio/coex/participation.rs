//! Who takes part in coexistence, as one affine state per radio client.
//!
//! Splitting the radio issues each client its [`CoexAbsent`]. Entering
//! coexistence turns it into a [`Coexisting`] and leaving turns it back, so
//! a client takes part at most once and `coex_enable` and `coex_disable`
//! cannot be unbalanced. A client leaves common power only while it shows
//! its [`CoexAbsent`]: it cannot leave power while it takes part. External
//! coexistence takes part with its own [`ExternalCoexistence`].
//!
//! Coexistence does not depend on the client's common power: Wi-Fi enables
//! it before entering power.

use core::marker::PhantomData;

use crate::power::radio::{RadioClient, RadioClientKind};

/// Client `K` takes no part in coexistence.
///
/// A client leaves common power only while it shows this state:
///
/// ```
/// use oer_chips_esp32s31_hal::radio::domain::{CoexAbsent, CommonPower, RadioDomain, WifiClient};
/// use oer_chips_esp32s31_hal::radio::ieee80211::registers::WifiRadioRegisters;
///
/// fn leave<B: oer_chips_esp32s31_hal::power::PlatformClockBackend>(
///     radio: &mut RadioDomain<B>,
///     wifi: &WifiRadioRegisters,
///     power: CommonPower<WifiClient>,
///     coex: &CoexAbsent<WifiClient>,
/// ) {
///     let _ = radio.exit_common_power(wifi, power, coex);
/// }
/// ```
///
/// Not while it takes part:
///
/// ```compile_fail,E0308
/// use oer_chips_esp32s31_hal::radio::domain::{CommonPower, Coexisting, RadioDomain, WifiClient};
/// use oer_chips_esp32s31_hal::radio::ieee80211::registers::WifiRadioRegisters;
///
/// fn leave<B: oer_chips_esp32s31_hal::power::PlatformClockBackend>(
///     radio: &mut RadioDomain<B>,
///     wifi: &WifiRadioRegisters,
///     power: CommonPower<WifiClient>,
///     coex: &Coexisting<WifiClient>,
/// ) {
///     let _ = radio.exit_common_power(wifi, power, coex);
/// }
/// ```
///
/// And the state cannot be made outside the radio split:
///
/// ```compile_fail,E0451
/// use oer_chips_esp32s31_hal::radio::domain::{CoexAbsent, WifiClient};
///
/// let _coex: CoexAbsent<WifiClient> = CoexAbsent { _client: core::marker::PhantomData };
/// ```
#[must_use = "a client's coexistence state returns to the radio when it reunites"]
#[derive(Debug)]
pub struct CoexAbsent<K: RadioClientKind> {
    _client: PhantomData<fn() -> K>,
}

impl<K: RadioClientKind> CoexAbsent<K> {
    pub(crate) const fn issued() -> Self {
        Self {
            _client: PhantomData,
        }
    }

    /// A state outside a radio split, for host tests of the later stages.
    #[cfg(test)]
    pub const fn for_host_test() -> Self {
        Self::issued()
    }
}

/// Client `K` takes part in coexistence.
#[must_use = "coexistence is left through the shared radio's owner"]
#[derive(Debug)]
pub struct Coexisting<K: RadioClientKind> {
    _client: PhantomData<fn() -> K>,
}

impl<K: RadioClientKind> Coexisting<K> {
    /// The client that takes part.
    pub const fn client(&self) -> RadioClient {
        K::CLIENT
    }
}

/// External coexistence takes part in coexistence.
#[must_use = "external coexistence is stopped through the shared radio's owner"]
#[derive(Debug)]
pub struct ExternalCoexistence {
    _private: (),
}

impl<K: RadioClientKind> CoexAbsent<K> {
    /// Enter coexistence.
    pub(crate) const fn enter(self) -> Coexisting<K> {
        let Self { _client } = self;
        Coexisting { _client }
    }
}

impl<K: RadioClientKind> Coexisting<K> {
    /// Leave coexistence.
    pub(crate) const fn leave(self) -> CoexAbsent<K> {
        let Self { _client } = self;
        CoexAbsent { _client }
    }
}

impl ExternalCoexistence {
    pub(crate) const fn started() -> Self {
        Self { _private: () }
    }

    pub(crate) const fn stop(self) {
        let Self { _private: () } = self;
    }
}

#[cfg(test)]
mod tests;
