use super::*;

#[test]
fn the_hal_buffer_claim_is_affine() {
    let first = claim_buffers().expect("this test claims the HAL static once");
    assert!(claim_buffers().is_none());
    let _claimed = first.into_inner();
    assert!(claim_buffers().is_none());
}
