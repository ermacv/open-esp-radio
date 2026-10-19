#![no_std]
use oer_ieee80211_lower_mac::{
    ClassFrame, ClassSizedFrame, GroupAddress, HeaderSpec, Ieee80211Capabilities, LowerMacPhase,
    PhyRate,
};

pub fn prepare<'p, 'id: 'p, P, C, K>(
    phase: &P,
    own: ClassSizedFrame<C, HeaderSpec<GroupAddress>, P::Caps>,
    foreign: ClassSizedFrame<C, HeaderSpec<GroupAddress>, K>,
    raw: PhyRate,
) where
    P: LowerMacPhase<'p, 'id>,
    K: Ieee80211Capabilities,
{
    let rate = phase.rate(raw).unwrap();
    #[cfg(ui_reject)]
    let frame = ClassFrame::<'p, 'id, C, K>::group(foreign, rate); // ui-error: E0308
    #[cfg(not(ui_reject))]
    let frame = ClassFrame::group(own, rate);
    drop(frame);
}
