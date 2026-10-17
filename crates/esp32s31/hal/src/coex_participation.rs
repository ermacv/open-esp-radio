//! Who takes part in coexistence, as one affine state per radio client.
//!
//! Splitting the radio issues each client its [`CoexAbsent`]. Entering
//! coexistence turns it into a [`Coexisting`] and leaving turns it back, so
//! a client takes part at most once and `coex_enable` and `coex_disable`
//! cannot be unbalanced. A client leaves common power only while it shows
//! its [`CoexAbsent`]: it cannot leave power while it takes part. External
//! coexistence takes part with its own [`ExternalCoexistence`]. The
//! arbiter's participant set follows these states; the coexistence policy
//! acts on it in the shared radio's owner (the `oer-espressif-coex` README
//! records the participant policy).
//!
//! Coexistence does not depend on the client's common power: Wi-Fi enables
//! it before entering power, Bluetooth after its PHY engine and IEEE 802.15.4
//! on request.

use core::marker::PhantomData;

use crate::clock::{RadioClient, RadioClientKind};

/// Client `K` takes no part in coexistence.
///
/// A client leaves common power only while it shows this state:
///
/// ```
/// use oer_esp32s31_hal::{
///     ieee802154::Ieee802154Powered,
///     shared_radio::{CoexAbsent, CommonPower, Ieee802154Client, SharedRadioLease},
/// };
///
/// fn leave(
///     lease: &mut SharedRadioLease<'_>,
///     powered: Ieee802154Powered,
///     power: CommonPower<Ieee802154Client>,
///     coex: &CoexAbsent<Ieee802154Client>,
/// ) {
///     let _ = powered.power_down(lease, power, coex);
/// }
/// ```
///
/// Not while it takes part:
///
/// ```compile_fail,E0308
/// use oer_esp32s31_hal::{
///     ieee802154::Ieee802154Powered,
///     shared_radio::{CommonPower, Coexisting, Ieee802154Client, SharedRadioLease},
/// };
///
/// fn leave(
///     lease: &mut SharedRadioLease<'_>,
///     powered: Ieee802154Powered,
///     power: CommonPower<Ieee802154Client>,
///     coex: &Coexisting<Ieee802154Client>,
/// ) {
///     let _ = powered.power_down(lease, power, coex);
/// }
/// ```
///
/// And the state cannot be made outside the radio split:
///
/// ```compile_fail,E0451
/// use oer_esp32s31_hal::shared_radio::{CoexAbsent, WifiClient};
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
    #[cfg(any(test, feature = "validation-probes"))]
    pub const fn for_validation() -> Self {
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

/// One participant of the coexistence set.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum Participant {
    Client(RadioClient),
    External,
}

impl Participant {
    const fn bit(self) -> u8 {
        match self {
            Self::Client(RadioClient::Wifi) => 1,
            Self::Client(RadioClient::Bluetooth) => 1 << 1,
            Self::Client(RadioClient::Ieee802154) => 1 << 2,
            Self::External => 1 << 3,
        }
    }
}

/// The radios that enabled coexistence (`coex_core_enable`).
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub(crate) struct CoexParticipants(u8);

impl CoexParticipants {
    pub(crate) const fn count(self) -> u32 {
        self.0.count_ones()
    }

    fn enter(&mut self, participant: Participant) -> u32 {
        self.0 |= participant.bit();
        self.count()
    }

    fn leave(&mut self, participant: Participant) -> u32 {
        self.0 &= !participant.bit();
        self.count()
    }

    /// Enter client `K`; returns how many take part.
    pub(crate) fn enter_client<K: RadioClientKind>(
        &mut self,
        absent: CoexAbsent<K>,
    ) -> (Coexisting<K>, u32) {
        let CoexAbsent { _client } = absent;
        let count = self.enter(Participant::Client(K::CLIENT));
        (Coexisting { _client }, count)
    }

    /// Leave client `K`; returns how many still take part.
    pub(crate) fn leave_client<K: RadioClientKind>(
        &mut self,
        coexisting: Coexisting<K>,
    ) -> (CoexAbsent<K>, u32) {
        let Coexisting { _client } = coexisting;
        let count = self.leave(Participant::Client(K::CLIENT));
        (CoexAbsent { _client }, count)
    }

    pub(crate) fn enter_external(&mut self) -> (ExternalCoexistence, u32) {
        let count = self.enter(Participant::External);
        (ExternalCoexistence { _private: () }, count)
    }

    pub(crate) fn leave_external(&mut self, external: ExternalCoexistence) -> u32 {
        let ExternalCoexistence { _private: () } = external;
        self.leave(Participant::External)
    }
}

#[cfg(test)]
mod tests;
