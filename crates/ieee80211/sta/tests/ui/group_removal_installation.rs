#![no_std]
use oer_ieee80211_sta::machine::{PendingGroupRemoval, StaInput, StaTokens};

pub fn return_to_installation<'a, 'b, T: StaTokens, K>(
    local: PendingGroupRemoval<'a, T::EpochPath>,
    foreign: PendingGroupRemoval<'b, T::EpochPath>,
    slot: T::GroupSlot,
    permission: T::GroupPermission,
) -> StaInput<'a, T, K> {
    #[cfg(ui_reject)]
    return StaInput::GroupKeyRemoved { pending: foreign, slot, permission }; // ui-error: null lifetime may not live long enough // ui-error: null lifetime may not live long enough
    #[cfg(not(ui_reject))]
    StaInput::GroupKeyRemoved { pending: local, slot, permission }
}
