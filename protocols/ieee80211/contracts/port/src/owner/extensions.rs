//! The optional operations of a lower-MAC phase.
//!
//! A structural feature a backend may lack is an extension trait of the
//! phase or interface that performs it: an upper layer that needs the
//! feature requires the bound, and a backend without it does not implement
//! the trait. Every rule a caller controls is a type here, as in the base
//! contract: an A-MPDU's rate; only the world's outcome stays a result. No
//! operation awaits: a setting takes effect before the next submission, and
//! what the hardware answers arrives as an event.
//!
//! - [`AmpduPhase`]: the rates the backend aggregates at, which a data
//!   arena's A-MPDU publication takes
//!   ([`ArenaFormat::Aggregate`](crate::arena::ArenaFormat::Aggregate)).
//! - [`BeaconTimingPhase`]: an interface's TSF and TBTT schedule; its TBTT
//!   events and TSF samples come in the phase's stream.
//! - [`AirReservationPhase`]: a gate's closing that reserves the access
//!   point's air for an absence of the station beside it.
//! - [`MonitorPhase`]: monitor reception, a radio-wide setting.

use crate::tsf::{TsfDuration, TsfInstant};

use super::PhaseRate;
use crate::{
    AmpduCapabilities,
    phase::{GateOpen, InterfacePhase, LowerMacPhase, PhaseTypes},
};

/// A rate the backend aggregates at, checked by [`AmpduPhase::ampdu_rate`].
#[derive(Debug)]
pub struct AmpduRate<'p, 'id, K>(PhaseRate<'p, 'id, K>);
impl<K> Copy for AmpduRate<'_, '_, K> {}
impl<K> Clone for AmpduRate<'_, '_, K> {
    fn clone(&self) -> Self {
        *self
    }
}
impl<'p, 'id, K> AmpduRate<'p, 'id, K> {
    /// The rate.
    pub const fn rate(&self) -> &PhaseRate<'p, 'id, K> {
        &self.0
    }
}

/// The minimum MPDU start spacing of an aggregate, as the 802.11 encoding
/// 0 to 7.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct MpduStartSpacing(u8);

impl MpduStartSpacing {
    /// `encoding`, when it is 0 to 7.
    pub const fn new(encoding: u8) -> Option<Self> {
        if encoding <= 7 {
            Some(Self(encoding))
        } else {
            None
        }
    }

    /// The encoding.
    pub const fn get(self) -> u8 {
        self.0
    }
}

/// An interface whose data arenas publish A-MPDUs.
pub trait AmpduPhase<'p, 'id: 'p>: InterfacePhase<'p, 'id> {
    /// What the backend aggregates.
    fn ampdu_capabilities(&self) -> AmpduCapabilities;

    /// Check that the backend aggregates at this phase's rate: a rate of its
    /// formats with an A-MPDU length.
    fn ampdu_rate(
        &self,
        rate: PhaseRate<'p, 'id, Self::Caps>,
    ) -> Result<AmpduRate<'p, 'id, Self::Caps>, PhaseRate<'p, 'id, Self::Caps>> {
        let capabilities = self.ampdu_capabilities();
        if capabilities.formats.contains_rate(rate.get())
            && capabilities.max_length.at(rate.get()).is_ok()
        {
            Ok(AmpduRate(rate))
        } else {
            Err(rate)
        }
    }
}

/// The TBTT schedule of an interface: the wake plan the backend carries
/// out at each TBTT.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct TbttPlan {
    /// The TSF of the next TBTT.
    pub next: TsfInstant,
    /// The beacon interval.
    pub beacon_interval: TsfDuration,
    /// How far ahead of each TBTT its event fires.
    pub lead: TsfDuration,
    /// How far ahead of each TBTT the radio wakes to receive its beacon.
    pub wake_ahead: TsfDuration,
    /// The window the radio keeps open for the beacon, which it asks the air
    /// for as [`Operation::BeaconWindow`](crate::Operation::BeaconWindow);
    /// `None` keeps no window.
    pub beacon_window: Option<BeaconWindow>,
}

/// The window after a TBTT the radio keeps open for the beacon.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct BeaconWindow {
    /// How long the window stays open after the TBTT.
    pub duration: TsfDuration,
}

/// An interface with a TSF and a TBTT schedule. Its settings take effect
/// before the next submission; [`Self::sample_tsf`]'s answer and each
/// TBTT arrive as events.
pub trait BeaconTimingPhase<'p, 'id: 'p>: InterfacePhase<'p, 'id> {
    /// Ask for a paired reading of the interface's TSF and the port's radio
    /// clock, which [`LowerMacTerminal::TsfSampled`](crate::LowerMacTerminal::TsfSampled)
    /// answers.
    fn sample_tsf(
        &mut self,
        permission: crate::TsfPermission<
            'id,
            Self::Role,
            <Self::Role as crate::RoleKeys<'id, Self::Types>>::CommandPath,
        >,
    );

    /// Set the interface's TSF to `at`.
    fn set_tsf(&mut self, at: TsfInstant);

    /// Follow `plan`, in place of the earlier one.
    fn set_tbtt(&mut self, plan: TbttPlan);

    /// Stop the TBTT schedule.
    fn stop_tbtt(&mut self);
}

/// A phase whose gate's closing can take one last frame that reserves the
/// air: a CTS-to-self of the access point, which its peers honour by
/// keeping quiet while the station beside it is away.
pub trait AirReservationPhase<'p, 'id: 'p>: LowerMacPhase<'p, 'id> {
    /// Close the gate as [`LowerMacPhase::close_gate`] does, and after the
    /// published attempts end, before
    /// [`LowerMacTerminal::GateClosed`](crate::LowerMacTerminal::GateClosed),
    /// send the access point's CTS-to-self reserving the air until `until`:
    /// its Duration is `min(until − now, MAX_DURATION_MICROS)` when it goes,
    /// and no frame goes once no time is left.
    fn close_gate_reserving(
        &mut self,
        gate: GateOpen<'id, <Self::Types as PhaseTypes<'id>>::GatePath>,
        until: crate::Ieee80211Instant,
    );
}

/// A phase that receives as a monitor.
pub trait MonitorPhase<'p, 'id: 'p>: LowerMacPhase<'p, 'id> {
    /// Receive every frame the radio hears, or only the interfaces' own.
    fn set_monitor(&mut self, enabled: bool);
}
