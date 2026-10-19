use super::*;

/// The sequence and the offset network time go into the IE content,
/// little endian, as the port writes them at the SFD.
#[test]
fn the_time_ie_gets_the_sequence_and_the_network_time() {
    let sync = TimeSync {
        ie_offset: 2,
        sequence: 7,
        network_time_offset: -1_000,
    };
    let mut mac = [0xee; 12];
    assert!(sync.write(&mut mac, 0x0102_0304_0506_0708 + 1_000));
    assert_eq!(
        mac,
        [
            0xee, 0xee, 7, 0x08, 0x07, 0x06, 0x05, 0x04, 0x03, 0x02, 0x01, 0xee
        ]
    );
}

/// A negative network time wraps as the port's signed addition does.
#[test]
fn the_network_time_wraps() {
    let sync = TimeSync {
        ie_offset: 0,
        sequence: 1,
        network_time_offset: -5,
    };
    let mut mac = [0; 9];
    assert!(sync.write(&mut mac, 3));
    assert_eq!(mac[1..], (-2_i64 as u64).to_le_bytes());
}

/// An IE that does not fit leaves the frame.
#[test]
fn an_ie_beyond_the_frame_is_not_written() {
    let sync = TimeSync {
        ie_offset: 4,
        sequence: 1,
        network_time_offset: 0,
    };
    let mut mac = [0; 12];
    assert!(!sync.write(&mut mac, 3));
    assert_eq!(mac, [0; 12]);
}
