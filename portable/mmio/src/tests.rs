use super::{Bits, Block, Host, Index, IndexError, ValueError, VolatileCell, Words};

const SIZE: usize = 16;

#[test]
fn a_block_reads_and_writes_only_the_word_at_its_offset() {
    let mut memory = [0x1111_1111, 0x2222_2222, 0x3333_3333, 0x4444_4444];
    let mut block = Block::<Host<'_>, SIZE>::host(&mut memory).expect("four words");
    assert_eq!(block.read::<8>(), 0x3333_3333);
    block.write::<4>(0xa5a5_a5a5);
    assert_eq!(block.read::<4>(), 0xa5a5_a5a5);
    drop(block);
    assert_eq!(memory, [0x1111_1111, 0xa5a5_a5a5, 0x3333_3333, 0x4444_4444]);
}

#[test]
fn host_memory_smaller_than_the_block_is_refused() {
    let mut memory = [0_u32; 3];
    let refused = Block::<Host<'_>, SIZE>::host(&mut memory).err();
    assert_eq!(
        refused,
        Some(super::HostTooSmall {
            required: SIZE,
            available: 12
        })
    );
}

#[test]
fn an_indexed_access_reaches_the_element_its_index_names() {
    let mut memory = [0_u32; 4];
    let mut block = Block::<Host<'_>, SIZE>::host(&mut memory).expect("four words");
    let index = Index::<2>::new(1).expect("index 1 of 2");
    block.write_indexed::<4, 8, 2>(index, 7);
    assert_eq!(block.read_indexed::<4, 8, 2>(index), 7);
    drop(block);
    assert_eq!(memory, [0, 0, 0, 7]);
}

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
}

#[test]
fn a_value_wider_than_its_field_is_an_error() {
    assert_eq!(Bits::<7>::new(0x7f).map(Bits::get), Ok(0x7f));
    assert_eq!(
        Bits::<7>::new(0x80),
        Err(ValueError {
            value: 0x80,
            width: 7
        })
    );
    assert_eq!(Bits::<32>::new(u32::MAX).map(Bits::get), Ok(u32::MAX));
}

struct Low;
impl Words for Low {
    const OWNED: &'static [usize] = &[0, 4];
    const OBSERVED: &'static [usize] = &[8];
}

struct High;
impl Words for High {
    const OWNED: &'static [usize] = &[8, 12];
    const OBSERVED: &'static [usize] = &[];
}

#[test]
fn split_parts_write_their_own_words_and_read_observed_ones() {
    let mut memory = [0_u32; 4];
    let block = Block::<Host<'_>, SIZE>::host(&mut memory).expect("four words");
    let (mut low, mut high) = block.split::<Low, High>();
    low.write::<4>(1);
    high.write::<8>(2);
    assert_eq!(low.read::<8>(), 2);
    let block = Block::join(low, high);
    assert_eq!(block.read::<4>(), 1);
    drop(block);
    assert_eq!(memory, [0, 1, 2, 0]);
}

#[test]
fn a_volatile_cell_holds_the_value_last_written() {
    let cell = VolatileCell::new(0_u32);
    cell.set(0xdead_beef);
    assert_eq!(cell.get(), 0xdead_beef);
    let zero: VolatileCell<u32> = bytemuck::Zeroable::zeroed();
    assert_eq!(zero.get(), 0);
}

#[test]
fn the_compile_time_predicates_accept_exactly_the_words_inside() {
    use super::{all_in_block, array_in_block, array_listed, disjoint, in_block};
    assert!(in_block(12, 16));
    assert!(!in_block(16, 16));
    assert!(!in_block(2, 16));
    assert!(array_in_block(4, 8, 2, 16));
    assert!(!array_in_block(4, 8, 3, 16));
    assert!(!array_in_block(0, 4, 0, 16));
    assert!(!array_in_block(0, usize::MAX, 2, 16));
    assert!(disjoint(&[0, 4], &[8, 12]));
    assert!(!disjoint(&[0, 4], &[4]));
    assert!(all_in_block(&[0, 12], 16));
    assert!(!all_in_block(&[0, 16], 16));
    assert!(array_listed(&[4, 12], 4, 8, 2));
    assert!(!array_listed(&[4], 4, 8, 2));
}

#[test]
fn transaction_checks_accept_only_declared_partitions() {
    use super::check::{partition, within};
    assert!(within(0x0002_8000, 0x7fff_ffff));
    assert!(!within(0x8000_0000, 0x7fff_ffff));
    assert!(partition(0xff00_0000, 0x00ff_ffff, 0, 0x0fff_ffff));
    assert!(!partition(0xff00_0000, 0x00ff_ffff, 0, 0x0000_ffff));
    assert!(!partition(0xff00_0000, 0x00ff_fffe, 0, 0x00ff_ffff));
    assert!(!partition(0xff00_0000, 0x00ff_ffff, 1, 0x00ff_ffff));
}
