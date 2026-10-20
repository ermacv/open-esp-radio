#![no_std]
use oer_ieee80211_sta::machine::{PendingGroupRemoval, StaInput, StaTokens};

pub fn return_to_station<'id, A: StaTokens, B: StaTokens, K>(
    local: PendingGroupRemoval<'id, A::EpochPath>,
    foreign: PendingGroupRemoval<'id, B::EpochPath>,
    slot: A::GroupSlot,
    permission: A::GroupPermission,
) -> StaInput<'id, A, K> {
    #[cfg(ui_reject)]
    return StaInput::GroupKeyRemoved { pending: foreign, slot, permission }; // ui-error: E0308
    #[cfg(not(ui_reject))]
    StaInput::GroupKeyRemoved { pending: local, slot, permission }
}
