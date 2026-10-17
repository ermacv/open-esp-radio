use super::*;
use crate::sequence::seq;

const STATION: [u8; 6] = [2, 3, 4, 5, 6, 7];
const BSSID: [u8; 6] = [0x20, 0x21, 0x22, 0x23, 0x24, 0x25];

fn frame(power_management: StaPowerManagement) -> StaNullDataFrame {
    StaNullDataFrame {
        station_address: crate::address::OwnAddress::new(
            crate::address::IndividualAddress::new(STATION).unwrap(),
        ),
        bssid: crate::address::Bssid::new(crate::address::IndividualAddress::new(BSSID).unwrap())
            .unwrap(),
        sequence_number: seq(0x123),
        power_management,
    }
}

#[test]
fn active_null_data_has_exact_to_ds_address_geometry() {
    let mut output = [0xa5; STA_NULL_DATA_FRAME_LEN];
    assert_eq!(
        frame(StaPowerManagement::Active).encode(&mut output),
        Ok(STA_NULL_DATA_FRAME_LEN)
    );
    assert_eq!(u16::from_le_bytes(output[0..2].try_into().unwrap()), 0x0148);
    assert_eq!(&output[2..4], &[0, 0]);
    assert_eq!(&output[4..10], &BSSID);
    assert_eq!(&output[10..16], &STATION);
    assert_eq!(&output[16..22], &BSSID);
    assert_eq!(&output[22..24], &0x1230_u16.to_le_bytes());
}

#[test]
fn power_save_changes_only_the_power_management_bit() {
    let mut active = [0; STA_NULL_DATA_FRAME_LEN];
    let mut sleeping = [0; STA_NULL_DATA_FRAME_LEN];
    frame(StaPowerManagement::Active)
        .encode(&mut active)
        .unwrap();
    frame(StaPowerManagement::PowerSave)
        .encode(&mut sleeping)
        .unwrap();

    assert_eq!(
        u16::from_le_bytes(sleeping[0..2].try_into().unwrap()),
        0x1148
    );
    active[1] |= 0x10;
    assert_eq!(sleeping, active);
}

#[test]
fn validation_happens_before_output_is_mutated() {
    let mut output = [0xa5; STA_NULL_DATA_FRAME_LEN];
    let encoded = crate::address::Bssid::new(
        crate::address::IndividualAddress::new([0; 6]).unwrap(),
    )
    .map(|bssid| {
        StaNullDataFrame {
            bssid,
            ..frame(StaPowerManagement::PowerSave)
        }
        .encode(&mut output)
    });
    assert_eq!(encoded, None);
    assert_eq!(output, [0xa5; STA_NULL_DATA_FRAME_LEN]);
}

#[test]
fn short_output_reports_exact_required_length_without_mutation() {
    let mut output = [0xa5; STA_NULL_DATA_FRAME_LEN - 1];
    assert_eq!(
        frame(StaPowerManagement::PowerSave).encode(&mut output),
        Err(StationFrameError::OutputTooSmall {
            required: STA_NULL_DATA_FRAME_LEN,
        })
    );
    assert_eq!(output, [0xa5; STA_NULL_DATA_FRAME_LEN - 1]);
}
