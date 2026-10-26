use super::{
    Bits, Block, Fixed, Host, Index, IndexError, NotOneSplit, Reader, Registers, ValueError, Words,
};

/// A block of four consecutive words.
struct Four;
impl Registers for Four {
    type Sharing = super::sharing::Owned;
    const WORDS: &'static [usize] = &[0, 4, 8, 12];
}

/// A block whose words leave a hole at byte 4.
struct Holed;
impl Registers for Holed {
    type Sharing = super::sharing::Owned;
    const WORDS: &'static [usize] = &[0, 8];
}

#[test]
fn a_block_reads_and_writes_only_the_word_at_its_offset() {
    let mut memory = [0x1111_1111, 0x2222_2222, 0x3333_3333, 0x4444_4444];
    let mut block = Block::<Host<'_>, Four>::host(&mut memory).expect("four words");
    assert_eq!(block.read::<8>(), 0x3333_3333);
    block.write::<4>(0xa5a5_a5a5);
    assert_eq!(block.read::<4>(), 0xa5a5_a5a5);
    drop(block);
    assert_eq!(memory, [0x1111_1111, 0xa5a5_a5a5, 0x3333_3333, 0x4444_4444]);
}

#[test]
fn host_memory_smaller_than_the_block_is_refused() {
    let mut memory = [0_u32; 3];
    let refused = Block::<Host<'_>, Four>::host(&mut memory).err();
    assert_eq!(
        refused,
        Some(super::HostTooSmall {
            required: 16,
            available: 12
        })
    );
}

#[test]
fn a_block_with_a_hole_writes_only_its_listed_words() {
    let mut memory = [0x1111_1111, 0x2222_2222, 0x3333_3333];
    let mut block = Block::<Host<'_>, Holed>::host(&mut memory).expect("three words");
    block.write::<0>(0);
    block.write::<8>(0);
    drop(block);
    assert_eq!(memory, [0, 0x2222_2222, 0]);
}

#[test]
fn a_field_value_read_back_keeps_its_low_bits() {
    assert_eq!(Bits::<7>::truncate(0xffff_ff85).get(), 0x05);
    assert_eq!(Bits::<32>::truncate(u32::MAX).get(), u32::MAX);
}

#[test]
fn an_indexed_access_reaches_the_element_its_index_names() {
    let mut memory = [0_u32; 4];
    let mut block = Block::<Host<'_>, Four>::host(&mut memory).expect("four words");
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
    let block = Block::<Host<'_>, Four>::host(&mut memory).expect("four words");
    let (mut low, mut high) = block.split::<Low, High>();
    low.write::<4>(1);
    high.write::<8>(2);
    assert_eq!(low.read::<8>(), 2);
    let block = Block::try_join(low, high).expect("parts of one split");
    assert_eq!(block.read::<4>(), 1);
    drop(block);
    assert_eq!(memory, [0, 1, 2, 0]);
}

#[test]
fn parts_of_two_host_blocks_do_not_join() {
    let mut first = [0_u32; 4];
    let mut second = [0_u32; 4];
    let (low, high) = Block::<Host<'_>, Four>::host(&mut first)
        .expect("four words")
        .split::<Low, High>();
    let (other_low, other_high) = Block::<Host<'_>, Four>::host(&mut second)
        .expect("four words")
        .split::<Low, High>();
    // The refused parts come back and join with their own siblings.
    let Err(NotOneSplit {
        first: low,
        second: other_high,
    }) = Block::try_join(low, other_high)
    else {
        panic!("a part of another block's split joined");
    };
    let mut block = Block::try_join(low, high).expect("parts of one split");
    block.write::<12>(3);
    drop(block);
    drop(Block::try_join(other_low, other_high).expect("parts of one split"));
    assert_eq!(first, [0, 0, 0, 3]);
    assert_eq!(second, [0; 4]);
}

#[test]
fn the_compile_time_predicates_accept_exactly_the_words_inside() {
    use super::{all_listed, array_listed, ascending, disjoint, end, listed};
    assert!(ascending(&[0, 4, 0x10]));
    assert!(!ascending(&[0, 0]));
    assert!(!ascending(&[4, 0]));
    assert!(!ascending(&[2]));
    assert_eq!(end(&[0, 0x10]), 0x14);
    assert_eq!(end(&[]), 0);
    assert!(listed(&[0, 8, 0x10, 0x7c80], 0x7c80));
    assert!(!listed(&[0, 8, 0x10], 4));
    assert!(!listed(&[], 0));
    assert!(array_listed(&[4, 12], 4, 8, 2));
    assert!(!array_listed(&[4], 4, 8, 2));
    assert!(!array_listed(&[0], 0, 4, 0));
    assert!(!array_listed(&[0], 0, usize::MAX, 2));
    assert!(disjoint(&[0, 4], &[8, 12]));
    assert!(!disjoint(&[0, 4], &[4]));
    assert!(all_listed(&[0, 12], &[0, 4, 12]));
    assert!(!all_listed(&[0, 16], &[0, 12]));
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

#[test]
fn interleaved_blocks_are_disjoint_unless_they_share_a_word() {
    use super::check::disjoint_blocks;
    // A block spanning another's range, with no word inside it.
    let outer: (usize, &[usize]) = (0x1000, &[0, 0x40]);
    let inner: (usize, &[usize]) = (0x1010, &[0, 4]);
    assert!(disjoint_blocks(&[outer, inner]));
    let touching: (usize, &[usize]) = (0x1038, &[8]);
    assert!(!disjoint_blocks(&[outer, inner, touching]));
    assert!(disjoint_blocks(&[]));
}

#[test]
fn two_sides_cover_a_block_only_when_each_word_has_exactly_one() {
    use super::check::sides_cover;
    assert!(sides_cover(&[0, 4, 8], &[0, 8], &[4]));
    assert!(!sides_cover(&[0, 4, 8], &[0], &[4]));
    assert!(!sides_cover(&[0, 4, 8], &[0, 4], &[4, 8]));
    assert!(!sides_cover(&[0, 4], &[0, 4], &[12]));
}

/// The first channel's pair of words; it enables its channel with bit 0 of
/// the shared trigger word at byte 12.
struct FirstChannel;
impl Words for FirstChannel {
    const OWNED: &'static [usize] = &[0];
    const OBSERVED: &'static [usize] = &[8];
    const TRIGGERS: &'static [(usize, u32)] = &[(12, 0b0001)];
}

/// The second channel; bits 1 and 2 of the trigger word are its own.
struct SecondChannel;
impl Words for SecondChannel {
    const OWNED: &'static [usize] = &[4];
    const OBSERVED: &'static [usize] = &[8];
    const TRIGGERS: &'static [(usize, u32)] = &[(12, 0b0110)];
}

/// The status word at byte 8, owned by a third part.
struct Status;
impl Words for Status {
    const OWNED: &'static [usize] = &[8];
    const OBSERVED: &'static [usize] = &[];
}

#[test]
fn divided_parts_store_only_their_own_trigger_bits() {
    let mut memory = [0_u32; 4];
    let block = Block::<Host<'_>, Four>::host(&mut memory).expect("four words");
    let (mut first, mut second, mut status) =
        block.divide::<(FirstChannel, SecondChannel, Status)>();
    first.write::<0>(0x11);
    second.write::<4>(0x22);
    status.write::<8>(0x33);
    assert_eq!(
        first.read::<8>(),
        0x33,
        "a part reads the words it observes"
    );
    // Each store carries only the writer's bits; the others are zero.
    first.write_trigger::<12>(u32::MAX);
    assert_eq!(status.read::<8>(), 0x33);
    drop((first, second, status));
    assert_eq!(memory, [0x11, 0x22, 0x33, 0b0001]);

    let block = Block::<Host<'_>, Four>::host(&mut memory).expect("four words");
    let (first, mut second, status) = block.divide::<(FirstChannel, SecondChannel, Status)>();
    second.write_trigger::<12>(u32::MAX);
    drop((first, second, status));
    assert_eq!(memory, [0x11, 0x22, 0x33, 0b0110]);
}

#[test]
fn a_reader_reads_the_words_it_took_over_in_every_copy() {
    let mut memory = [1_u32, 2, 3, 4];
    let block = Block::<Host<'_>, Four>::host(&mut memory).expect("four words");
    let (low, high) = block.split::<Low, High>();
    let reader: Reader<Host<'_>, Four, Low> = low.into_reader();
    let copy = reader;
    let mut high = high;
    high.write::<8>(9);
    assert_eq!(reader.read::<4>(), 2);
    assert_eq!(
        copy.read::<8>(),
        9,
        "an observed word shows its owner's write"
    );
    drop(high);

    let mut whole = [5_u32, 6, 7, 8];
    let reader = Block::<Host<'_>, Four>::host(&mut whole)
        .expect("four words")
        .into_reader::<Low>();
    let index = Index::<2>::new(1).expect("index 1 of 2");
    assert_eq!(reader.read_indexed::<0, 4, 2>(index), 6);
}

#[test]
fn the_division_checks_accept_only_disjoint_owners_and_trigger_bits() {
    use super::{PartWords, owned_disjoint, owners_cover, parts_listed, triggers_disjoint};
    const FIRST: PartWords = (&[0], &[8], &[(12, 0b0001)]);
    const SECOND: PartWords = (&[4], &[8], &[(12, 0b0110)]);
    const BLOCK: &[usize] = &[0, 4, 8, 12];
    assert!(parts_listed(BLOCK, &[FIRST, SECOND]));
    assert!(
        !parts_listed(&[0, 4, 8], &[FIRST]),
        "the trigger word is listed"
    );
    assert!(
        !parts_listed(BLOCK, &[(&[], &[], &[(12, 0)])]),
        "a trigger has bits"
    );
    assert!(!parts_listed(BLOCK, &[(&[], &[], &[(12, 1), (12, 2)])]));
    assert!(owned_disjoint(&[FIRST, SECOND]));
    assert!(!owned_disjoint(&[FIRST, (&[0], &[], &[])]));
    assert!(triggers_disjoint(&[FIRST, SECOND]));
    assert!(!triggers_disjoint(&[FIRST, (&[], &[], &[(12, 0b0011)])]));
    assert!(
        !triggers_disjoint(&[FIRST, (&[12], &[], &[])]),
        "no part owns a trigger word"
    );
    assert!(owners_cover(BLOCK, &[0, 4], &[8, 12]));
    assert!(!owners_cover(BLOCK, &[0], &[4, 12]));
}

/// A shared block of two words.
struct Shared;
impl Registers for Shared {
    type Sharing = super::sharing::Shared;
    const WORDS: &'static [usize] = &[0, 4];
}

#[test]
fn a_shared_store_through_a_shared_borrow_writes_only_its_word() {
    let mut memory = [0x1111_1111, 0x2222_2222];
    let block = Block::<Host<'_>, Shared>::host(&mut memory).expect("two words");
    let (first, second) = (&block, &block);
    first.write_shared::<4>(0x4000_0000);
    second.write_shared::<0>(1);
    assert_eq!(first.read::<4>(), 0x4000_0000);
    drop(block);
    assert_eq!(memory, [1, 0x4000_0000]);
}

#[test]
fn a_shared_block_at_a_fixed_address_is_sync() {
    fn sync<T: Sync>() {}
    sync::<Block<Fixed<0x1000>, Shared>>();
}
