#![no_std]
use oer_portable_time::Instant;
use oer_protocols_ieee80211_contracts_port::{
    AccessPoint, CommandPlace, DataPort, GroupKeyId, GroupCommand, GroupSlot, Station,
};
use oer_protocols_ieee80211_contracts_security::Ccmp128Key;
// A key slot names its interface by its type: a station's slot is not an
// access point's key command's.
pub fn install<'id, P: DataPort<'id>>(
    port: &mut P,
    place: CommandPlace<'id>,
    station: GroupSlot<'id, Station>,
    access_point: GroupSlot<'id, AccessPoint>,
    key_id: GroupKeyId,
    key: Ccmp128Key,
    now: Instant,
) {
    #[cfg(ui_reject)]
    let command = GroupCommand::<'id, AccessPoint>::Install { slot: station, key_id, key }; // ui-error: E0308
    #[cfg(not(ui_reject))]
    let command = GroupCommand::<'id, AccessPoint>::Install { slot: access_point, key_id, key };
    let _running = port.begin_group(place, command, now);
    let _ = station;
}
