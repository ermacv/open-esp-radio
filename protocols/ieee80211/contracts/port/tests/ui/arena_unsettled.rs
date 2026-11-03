#![no_std]
use oer_portable_time::Instant;
use oer_protocols_ieee80211_contracts_port::arena::{
    Arena, ArenaCompleted, ArenaFormat, ArenaPublication, TxArena,
};
use oer_protocols_ieee80211_contracts_port::{MpduProtection, Protection, SupportedPower};
use oer_protocols_ieee80211_wire::qos::WmmAccessCategory;

pub fn unsettled<R, D>(done: ArenaCompleted<R, D>) -> Option<Arena> {
    #[cfg(ui_reject)]
    return Some(done.arena); // ui-error: E0616
    #[cfg(not(ui_reject))]
    {
        drop(done);
        None
    }
}

pub fn forged() -> Option<Arena> {
    #[cfg(ui_reject)]
    return Some(Arena(oer_protocols_ieee80211_contracts_port::arena::ArenaId::First)); // ui-error: E0423
    #[cfg(not(ui_reject))]
    None
}

pub fn published<'p, 'id: 'p, A: TxArena<'p, 'id>>(
    port: &mut A,
    arena: Arena,
    format: ArenaFormat<'p, 'id, A::Caps>,
    power: SupportedPower<A::Caps>,
    memory: A::Memory,
    now: Instant,
) {
    let publication = ArenaPublication {
        arena,
        format,
        access_category: WmmAccessCategory::BestEffort,
        protection: Protection::None,
        encryption: MpduProtection::Open,
        power,
    };
    drop(port.publish(publication, now));
    #[cfg(ui_reject)]
    drop(port.intake(&arena, memory, |_| None)); // ui-error: E0382
    #[cfg(not(ui_reject))]
    drop(memory);
}
