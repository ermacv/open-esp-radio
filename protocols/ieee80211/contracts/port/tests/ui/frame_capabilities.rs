#![no_std]
use oer_protocols_ieee80211_contracts_port::{
    ClassFrame, ClassSizedFrame, DataPort, GroupAddress, HeaderSpec, Ieee80211Capabilities,
    PhyRate,
};

pub fn prepare<'id, P, C, K>(
    port: &P,
    own: ClassSizedFrame<C, HeaderSpec<GroupAddress>, P::Caps>,
    foreign: ClassSizedFrame<C, HeaderSpec<GroupAddress>, K>,
    raw: PhyRate,
) where
    P: DataPort<'id>,
    K: Ieee80211Capabilities,
{
    let rate = port.rate(raw).unwrap();
    #[cfg(ui_reject)]
    let frame = ClassFrame::<'_, C, K>::group(foreign, rate); // ui-error: E0308
    #[cfg(not(ui_reject))]
    let frame = ClassFrame::group(own, rate);
    drop(frame);
}
