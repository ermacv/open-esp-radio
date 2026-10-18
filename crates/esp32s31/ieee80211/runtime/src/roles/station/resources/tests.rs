use super::*;

#[test]
fn storage_group_returns_each_exact_owner() {
    let mut table = ScanTable::<2>::new();
    let mut management = [0_u8; 32];
    let mut ethernet = [0_u8; 48];
    let mut resources =
        StationStorageResources::new(11_u8, 22_u16, &mut table, &mut management, &mut ethernet);
    let (dma, tx, _, management, ethernet) = resources.parts_mut();
    *dma = 12;
    *tx = 23;
    management[0] = 0xa5;
    ethernet[0] = 0x5a;

    let (dma, tx, returned_table, management, ethernet) = resources.into_parts();
    assert_eq!(dma, 12);
    assert_eq!(tx, 23);
    assert_eq!(returned_table.summary().records, 0);
    assert_eq!(management[0], 0xa5);
    assert_eq!(ethernet[0], 0x5a);
}
