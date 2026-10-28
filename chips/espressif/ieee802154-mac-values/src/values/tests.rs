// SPDX-License-Identifier: Apache-2.0
use super::{Ieee802154MultipanEnableState as State, Ieee802154MultipanIndex as Index};

#[test]
fn multipan_indices_reject_every_unknown_context() {
    let contexts = [
        Index::CONTEXT0,
        Index::CONTEXT1,
        Index::CONTEXT2,
        Index::CONTEXT3,
    ];
    for raw in u8::MIN..=u8::MAX {
        assert_eq!(Index::new(raw), contexts.get(usize::from(raw)).copied());
    }
    for (raw, context) in contexts.into_iter().enumerate() {
        assert_eq!(context.value(), raw as u8);
        assert_eq!(context.as_usize(), raw);
    }
    assert_eq!(usize::from(Index::COUNT), contexts.len());
}

#[test]
fn every_multipan_update_changes_only_the_selected_context() {
    const SELECTED: State = State::NONE.with(Index::CONTEXT3).without(Index::CONTEXT0);
    assert_eq!(SELECTED.enabled(), [false, false, false, true]);
    for bits in 0u8..16 {
        let enabled = core::array::from_fn(|index| bits & (1 << index) != 0);
        let state = State::from_enabled(enabled);
        assert_eq!(
            State::new(enabled[0], enabled[1], enabled[2], enabled[3]),
            state
        );
        for raw in 0..Index::COUNT {
            let context = Index::new(raw).unwrap();
            let index = context.as_usize();
            assert_eq!(state.contains(context), enabled[index]);
            let mut expected = enabled;
            expected[index] = true;
            assert_eq!(state.with(context).enabled(), expected);
            expected[index] = false;
            assert_eq!(state.without(context).enabled(), expected);
        }
    }
}
