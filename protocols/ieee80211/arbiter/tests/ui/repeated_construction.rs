#![no_std]
use oer_ieee80211_arbiter::{Arbiter, ArbiterAuthority, ArbiterPolicy};
use oer_ieee80211_lower_mac::{GateOpen, Ieee80211Capabilities, SupportedChannel};
use oer_time::Duration;

pub fn construct<'id, K: Ieee80211Capabilities>(
    authority: ArbiterAuthority<'id>,
    gate: GateOpen<'id>,
    returned_gate: GateOpen<'id>,
    channel: SupportedChannel<K>,
) {
    let (_arbiter, _place, _absence) = Arbiter::<K>::new(
        ArbiterPolicy::DEFAULT.checked(Duration::ZERO).unwrap(), channel, authority, gate,
    );
    #[cfg(ui_reject)]
    let _second = Arbiter::<K>::new(
        ArbiterPolicy::DEFAULT.checked(Duration::ZERO).unwrap(), channel, authority, returned_gate, // ui-error: E0382
    );
}
