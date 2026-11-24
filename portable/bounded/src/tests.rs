use super::{Index, IndexError};

#[test]
fn an_index_outside_its_array_is_an_error() {
    assert_eq!(Index::<4>::new(4), Err(IndexError { index: 4, count: 4 }));
    assert_eq!(
        Index::<4>::new(usize::MAX),
        Err(IndexError {
            index: usize::MAX,
            count: 4
        })
    );
    assert_eq!(Index::<4>::constant::<3>().get(), 3);
    assert_eq!(Index::<4>::new(3).map(Index::get), Ok(3));
}

#[test]
fn every_byte_is_a_number_below_two_hundred_fifty_six() {
    assert_eq!(Index::<256>::from_byte(0xab).get(), 0xab);
    assert_eq!(Index::<300>::from_byte(u8::MAX).get(), 255);
}

#[test]
fn a_number_is_its_byte_and_its_word_whole() {
    assert_eq!(Index::<256>::constant::<255>().to_u8(), 255);
    assert_eq!(Index::<16>::constant::<9>().to_u8(), 9);
    assert_eq!(Index::<300>::constant::<299>().to_u32(), 299);
    assert_eq!(Index::<4>::new(3).map(Index::to_u32), Ok(3));
}

#[test]
fn every_index_is_its_position() {
    assert_eq!(Index::<3>::every().map(Index::get), [0, 1, 2]);
    assert_eq!(Index::<0>::every(), []);
}

#[test]
fn a_word_s_ones_are_counted_whole() {
    assert_eq!(Index::<65>::ones(0).get(), 0);
    assert_eq!(Index::<65>::ones(0b1011).get(), 3);
    assert_eq!(Index::<65>::ones(u64::MAX).get(), 64);
}

#[test]
fn a_number_below_its_bound_is_its_whole_word() {
    assert_eq!(
        Index::<{ 1 << 16 }>::new(0xfedc).map(Index::to_u16),
        Ok(0xfedc)
    );
    assert_eq!(Index::<256>::new(0xab).map(Index::to_u16), Ok(0xab));
}
