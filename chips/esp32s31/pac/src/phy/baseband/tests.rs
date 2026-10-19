use super::{decode_noise_floor_quarter_db, quarter_db_to_dbm};

#[test]
fn noise_floor_decode_reproduces_both_complete_arithmetic_shifts() {
    // -96 dBm is encoded as -1536 sixteenth-dB, or low twelve bits 0xa00.
    assert_eq!(decode_noise_floor_quarter_db(0x0a00), -384);
    assert_eq!(decode_noise_floor_quarter_db(0x0fff), -1);
    assert_eq!(decode_noise_floor_quarter_db(0x0000), -1024);
    assert_eq!(quarter_db_to_dbm(-384), -96);
    assert_eq!(quarter_db_to_dbm(-1), 0);
    assert_eq!(quarter_db_to_dbm(-1024), 0);
}
