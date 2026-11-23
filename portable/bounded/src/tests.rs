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
