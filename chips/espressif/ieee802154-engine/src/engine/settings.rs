//! Register settings retained by the engine across MAC reset and power loss.

use super::*;

#[derive(Clone, Copy, Default)]
pub(super) struct Identity {
    pub panid: u16,
    pub short_address: u16,
    pub extended_address: [u8; 8],
}

pub(super) struct RetainedSettings {
    identities: [Identity; 4],
    captured: u8,
    pub ack_timeout: Option<u16>,
    pub enable: Option<Ieee802154MultipanEnableState>,
}

impl RetainedSettings {
    pub const fn new() -> Self {
        Self {
            identities: [Identity {
                panid: 0,
                short_address: 0,
                extended_address: [0; 8],
            }; 4],
            captured: 0,
            ack_timeout: None,
            enable: None,
        }
    }

    pub fn identity<L: Ieee802154LowLevel + ?Sized>(
        &mut self,
        ll: &mut L,
        index: Ieee802154MultipanIndex,
    ) -> &mut Identity {
        let slot = &mut self.identities[usize::from(index.value())];
        if self.captured & (1 << index.value()) == 0 {
            *slot = Identity {
                panid: ll.multipan_panid(index),
                short_address: ll.multipan_short_address(index),
                extended_address: ll.multipan_extended_address(index),
            };
            self.captured |= 1 << index.value();
        }
        slot
    }

    pub fn restore<L: Ieee802154LowLevel + ?Sized>(&mut self, ll: &mut L, count: u8) {
        // Capture the first hardware epoch's identity, including any cold
        // policy. Later epochs write the shadow even when all registers lost
        // their state; no reset value or power-domain retention is assumed.
        let enable = *self.enable.get_or_insert_with(|| ll.multipan_enable());
        let timeout = *self.ack_timeout.get_or_insert_with(|| ll.ack_timeout());
        for number in 0..count {
            let index = Ieee802154MultipanIndex::new(number).expect("configured interface exists");
            let identity = *self.identity(ll, index);
            ll.set_multipan_panid(index, identity.panid);
            ll.set_multipan_short_address(index, identity.short_address);
            ll.set_multipan_extended_address(index, identity.extended_address);
        }
        ll.set_multipan_enable(enable);
        ll.set_ack_timeout(timeout);
    }
}
