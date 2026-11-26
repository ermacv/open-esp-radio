use super::*;
use crate::aes::SoftwareAes128;

const KEY: Aes128Key = Aes128Key::copy_from(&[0x77; 16]);
/// A management header and a two-octet body.
const LENGTH: usize = 26;

#[test]
fn a_protected_frame_verifies_under_its_own_identifier_and_key_alone() {
    let mut frame = [0; 64];
    frame[0] = 0xc0;
    let ipn = oer_portable_wire::U64::new(5).unwrap();
    let protected = protect(
        &mut SoftwareAes128,
        IgtkKeyId::Five,
        &KEY,
        ipn,
        &mut frame,
        LENGTH,
    )
    .unwrap();
    assert_eq!(
        protected,
        LENGTH + oer_protocols_ieee80211_wire::bip::MANAGEMENT_MIC_ELEMENT_LEN
    );
    let frame = &frame[..protected];
    let parsed = BipFrame::parse(frame, IgtkKeyId::Five).ok().unwrap();
    assert_eq!(parsed.packet_number(), ipn);
    assert_eq!(parsed.verify(&mut SoftwareAes128, &KEY), Ok(()));
    let parsed = BipFrame::parse(frame, IgtkKeyId::Five).ok().unwrap();
    assert_eq!(
        parsed.verify(&mut SoftwareAes128, &Aes128Key::copy_from(&[0x78; 16])),
        Err(BipError::InvalidMic)
    );
    assert_eq!(
        BipFrame::parse(frame, IgtkKeyId::Four).err(),
        Some(BipError::UnknownKeyId)
    );
}
