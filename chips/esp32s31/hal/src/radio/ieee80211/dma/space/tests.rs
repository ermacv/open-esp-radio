use super::*;

#[test]
fn a_host_field_is_addressed_by_its_offset_in_the_dma_window() {
    let arena = [0_u32; 4];
    assert_eq!(HostDma::address(&arena, &arena[0]), Some(HOST_ARENA_BASE));
    assert_eq!(
        HostDma::address(&arena, &arena[3]),
        Some(HOST_ARENA_BASE + 12)
    );
    // A field before its arena has no address.
    assert_eq!(HostDma::address(&arena[1], &arena[0]), None);
}

#[test]
fn a_chip_field_is_addressed_by_itself_when_it_fits() {
    let value = 0_u32;
    let address = ChipDma::address(&value, &value);
    assert_eq!(
        address.is_some(),
        u32::try_from((&raw const value).addr()).is_ok()
    );
}
