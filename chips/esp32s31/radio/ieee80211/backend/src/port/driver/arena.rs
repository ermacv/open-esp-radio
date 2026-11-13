//! The station's two data arenas in the driver: built over the A-MPDU
//! chains at preparation, served in the port's transmit service on a
//! transmit edge and at their deadline, and lent to the datapath actor for
//! one turn ([`super::super::Esp32s31Arena`]).
use super::*;
use crate::lower_mac::arena::{ArenaPort, Esp32s31TxArena};
use oer_protocols_ieee80211_contracts_port::arena::TxArena;

/// The station's data arenas of an installation over its two chains.
pub(in crate::port) type DataArenas<'id, Id, const B: usize> =
    Esp32s31TxArena<'id, 'static, TxBacking<B>, CorePath<Id>, TX_AMPDU_SUBFRAMES, TX_TICKETS>;

/// One step's arena port of an installation.
pub(in crate::port) type Port<'a, 'id, Id, D, const B: usize, O> = ArenaPort<
    'a,
    'id,
    'static,
    <D as Device<B>>::Power,
    <D as Device<B>>::Entropy,
    <D as Device<B>>::Timer,
    <D as Device<B>>::Coex,
    <D as Device<B>>::Hardware,
    Sink<'id, Id, B>,
    O,
    TxBacking<B>,
    Esp32s31Caps<B>,
    B,
    CorePath<Id>,
    TX_TICKETS,
    TX_AMPDU_SUBFRAMES,
>;

/// The station's installed pairwise transmit key the arenas admit: the
/// newest active one. A key whose removal began is no longer active.
pub(in crate::port) fn station_pairwise<'a, 'id, Id: SlotIdentity>(
    keys: &'a [Option<KeyPlace<'id, Id>>],
) -> Option<&'a KeyHandle<'id, CorePath<Id>>> {
    keys.iter()
        .flatten()
        .filter_map(KeyPlace::installed)
        .filter_map(|(identity, installed)| match installed {
            Installed::Transmit(key) if identity.role == VifRole::Station && identity.active() => {
                Some((identity.serial, key))
            }
            _ => None,
        })
        .max_by_key(|(serial, _)| *serial)
        .map(|(_, key)| key)
}

impl<'id, Id: SlotIdentity, D: Device<B>, const B: usize, O: Observations>
    Driver<'id, Id, D, B, O>
{
    /// Whether an arena is on the air.
    pub(in crate::port) fn arena_on_air(&self) -> bool {
        self.arena
            .as_ref()
            .is_some_and(|arena| arena.on_air().is_some())
    }

    /// Whether the arenas give their chains and tickets back at retirement:
    /// both idle and empty, holding no key right, never faulted.
    pub(in crate::port) fn arenas_returnable(&self) -> bool {
        self.arena_state() == oer_protocols_ieee80211_contracts_port::ArenaState::Returnable
    }

    /// The aggregate arenas as a retirement reads them.
    pub(in crate::port) fn arena_state(
        &self,
    ) -> oer_protocols_ieee80211_contracts_port::ArenaState {
        use oer_protocols_ieee80211_contracts_port::ArenaState;
        match &self.arena {
            None => ArenaState::Absent,
            Some(arena) if arena.returnable() => ArenaState::Returnable,
            Some(_) => ArenaState::Held,
        }
    }

    /// Whether the station's pending pairwise key removal waits for an
    /// arena's key right: a frame it encrypted is on the air, awaits
    /// settlement or is retained.
    pub(in crate::port) fn arena_holds_removed_key(&self) -> bool {
        self.barrier
            .is_some_and(|identity| identity.role == VifRole::Station)
            && self
                .arena
                .as_ref()
                .is_some_and(DataArenas::<Id, B>::holds_key_right)
    }

    /// Whether a timeout abort settles: the arena's own or an MPDU's, which
    /// forces CCA.
    pub(in crate::port) fn arena_abort_settling(&mut self) -> bool {
        self.arena
            .as_ref()
            .is_some_and(DataArenas::<Id, B>::settling)
            || self
                .core
                .view()
                .arena_core()
                .is_some_and(|core| core.abort_settling())
    }

    /// Offer one TX edge to the arena on the air at `now`: its completion,
    /// and `true` when its timeout abort settled, so the MPDUs' latched
    /// timeouts are offered again.
    pub(in crate::port) fn service_arena(
        &mut self,
        device: &mut D,
        wake: WifiTxWake,
        now: Instant,
    ) -> Result<(Option<brand::ArenaDone>, bool), Fault> {
        let Some(arena) = self.arena.as_mut() else {
            return Ok((None, false));
        };
        if arena.on_air().is_none() {
            return Ok((None, false));
        }
        let was_settling = arena.settling();
        let hardware = device.hardware()?;
        let core = self
            .core
            .view()
            .arena_core()
            .ok_or(Fault::CoreViewUnavailable(CoreOp::Arena))?;
        let observations = self
            .observations
            .as_mut()
            .ok_or(Fault::InstallationResources)?;
        let pairwise = station_pairwise(&self.keys);
        // The arena core hands its port nothing: a receipt or a completion
        // it finishes waits with the core for the next ordinary service.
        let mut sink = Sink::refusing();
        let (done, settled) = {
            let mut port: Port<'_, 'id, Id, D, B, O> = ArenaPort::new(
                arena,
                core,
                hardware,
                &mut self.free.bank,
                VifRole::Station,
                pairwise,
                &mut sink,
                observations,
            );
            let done = port.service(wake, now).map_err(Fault::Arena)?;
            (done, was_settling && !port.arenas.settling())
        };
        Ok((done, settled))
    }
}
