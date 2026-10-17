use super::*;

#[test]
fn only_the_last_mic_octet_of_a_data_pdu_is_inverted() {
    let pdu = [0x02, 5, 0x11, 0x22, 0x33, 0x44, 0x55];
    let mut buffer = [0; 16];
    assert_eq!(
        corrupt_data_mic(&pdu, &mut buffer),
        Some(&[0x02, 5, 0x11, 0x22, 0x33, 0x44, 0xaa][..])
    );
    let continuation = [0x01, 5, 1, 2, 3, 4, 5];
    assert_eq!(
        corrupt_data_mic(&continuation, &mut buffer).map(|pdu| pdu[6]),
        Some(!5)
    );
}

#[test]
fn control_and_empty_pdus_are_left_alone() {
    let mut buffer = [0; 16];
    assert_eq!(
        corrupt_data_mic(&[0x03, 5, 0x06, 1, 2, 3, 4], &mut buffer),
        None
    );
    assert_eq!(corrupt_data_mic(&[0x01, 0], &mut buffer), None);
    assert_eq!(corrupt_data_mic(&[0x02], &mut buffer), None);
}

#[test]
fn command_complete_echoes_status_and_handle() {
    assert_eq!(
        command_complete(Status::new(0x0c), [0x40, 0x00]),
        [0x0e, 6, 1, 0x01, 0xfc, 0x0c, 0x40, 0x00]
    );
}
