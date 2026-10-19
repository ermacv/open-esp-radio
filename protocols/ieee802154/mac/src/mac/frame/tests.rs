use super::*;

#[test]
fn every_legal_length_round_trips_without_dma_layout() {
    let source = [0xa5; MAX_MAC_FRAME_LEN];
    for length in MIN_MAC_FRAME_LEN..=MAX_MAC_FRAME_LEN {
        let frame = Frame::try_from_bytes(&source[..length]).unwrap();
        assert_eq!(frame.len(), length);
        assert_eq!(frame.as_bytes(), &source[..length]);
        assert_eq!(frame.view().bytes(), &source[..length]);
        assert_eq!(core::mem::size_of_val(&frame), MAX_MAC_FRAME_LEN + 1);
    }
}

#[test]
fn invalid_replacement_preserves_the_previous_frame() {
    let mut frame = Frame::try_from_bytes(&[1, 2, 3]).unwrap();
    assert_eq!(frame.replace(&[]), Err(FrameError::Empty));
    assert_eq!(frame.as_bytes(), &[1, 2, 3]);
    assert_eq!(
        frame.replace(&[0; MAX_MAC_FRAME_LEN + 1]),
        Err(FrameError::TooLong {
            length: MAX_MAC_FRAME_LEN + 1,
            maximum: MAX_MAC_FRAME_LEN,
        })
    );
    assert_eq!(frame.as_bytes(), &[1, 2, 3]);
}

#[test]
fn shorter_replacement_clears_trailing_storage() {
    let mut frame = Frame::try_from_bytes(&[0x55; 8]).unwrap();
    frame.replace(&[0xaa; 2]).unwrap();
    assert_eq!(frame.as_bytes(), &[0xaa; 2]);
    assert!(frame.bytes[2..].iter().all(|byte| *byte == 0));
}

#[test]
fn acknowledgement_requirement_is_derived_from_every_first_fcf_octet() {
    for fcf in u8::MIN..=u8::MAX {
        let view = FrameView::new(core::slice::from_ref(&fcf)).unwrap();
        assert_eq!(
            view.acknowledgement_requested(),
            fcf & ACKNOWLEDGEMENT_REQUEST_BIT != 0,
            "FCF 0x{fcf:02x}",
        );
    }
}
