use super::*;

fn records() -> [[u8; CHANNEL_INFO_BYTES]; CHANNEL_COUNT] {
    core::array::from_fn(|index| {
        let primary = index as u8 + 1;
        let mut record = [0; CHANNEL_INFO_BYTES];
        record[0] = primary;
        record[2..4].copy_from_slice(&expected_frequency_mhz(primary).to_le_bytes());
        record[8..12].copy_from_slice(&0x83_u32.to_le_bytes());
        record
    })
}

#[test]
fn adoption_copies_home_current_and_opaque_records() {
    let mut state = ChannelState::new();
    state.adopt([6, 0], [11, 0], records()).unwrap();

    assert!(state.adopted());
    assert_eq!(state.home(), Some([6, 0]));
    assert_eq!(state.current(), Some([11, 0]));
    assert_eq!(
        state.prepare([14, 0]),
        Some(PreparedChannel {
            frequency_mhz: 2484,
            cbw: 0,
        })
    );
}

#[test]
fn invalid_table_does_not_publish_partial_state() {
    let mut bad = records();
    bad[6][0] = 9;
    let mut state = ChannelState::new();

    assert_eq!(
        state.adopt([1, 0], [1, 0], bad),
        Err(ChannelStateAdoptionError::InvalidRecord(7))
    );
    assert!(!state.adopted());
    assert_eq!(state.home(), None);
}

#[test]
fn secondary_channel_geometry_is_explicit() {
    let mut state = ChannelState::new();
    state.adopt([6, 0], [6, 0], records()).unwrap();

    assert_eq!(
        state.prepare([5, 2]),
        Some(PreparedChannel {
            frequency_mhz: 2422,
            cbw: 3,
        })
    );
    assert_eq!(
        state.prepare([9, 1]),
        Some(PreparedChannel {
            frequency_mhz: 2462,
            cbw: 2,
        })
    );
    assert_eq!(state.prepare([1, 2]), None);
    assert_eq!(state.prepare([13, 1]), None);
}

#[test]
fn home_promotion_is_owned_state_transition() {
    let mut state = ChannelState::new();
    state.adopt([1, 0], [11, 0], records()).unwrap();

    state.promote_current_to_home().unwrap();
    assert_eq!(state.home(), Some([11, 0]));
}
