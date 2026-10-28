extern crate std;

use std::vec::Vec;

use super::sort;

fn sorted_by_std<T: Ord + Clone>(values: &[T]) -> Vec<T> {
    let mut expected = values.to_vec();
    expected.sort();
    expected
}

#[test]
fn sorts_every_small_permutation_of_repeated_values() {
    // All sequences of length 0..=6 over three values, so duplicates meet in
    // every position.
    for length in 0..=6_u32 {
        for code in 0..3_u32.pow(length) {
            let mut values: Vec<u32> = (0..length)
                .map(|digit| code / 3_u32.pow(digit) % 3)
                .collect();
            let expected = sorted_by_std(&values);
            sort(&mut values);
            assert_eq!(values, expected, "code {code} length {length}");
        }
    }
}

#[test]
fn sorted_reversed_and_constant_inputs_sort_correctly() {
    for length in [1_usize, 2, 3, 63, 64, 65, 1000] {
        let ascending: Vec<i64> = (0..length as i64).collect();
        let mut values = ascending.clone();
        sort(&mut values);
        assert_eq!(values, ascending);
        let mut values: Vec<i64> = ascending.iter().rev().copied().collect();
        sort(&mut values);
        assert_eq!(values, ascending);
        let mut values = std::vec![7_i64; length];
        sort(&mut values);
        assert_eq!(values, std::vec![7_i64; length]);
    }
}

#[test]
fn extreme_values_and_pseudo_random_inputs_sort_correctly() {
    let mut values = std::vec![i64::MAX, i64::MIN, 0, -1, 1, i64::MAX, i64::MIN];
    let expected = sorted_by_std(&values);
    sort(&mut values);
    assert_eq!(values, expected);
    let mut state = 0x2545_f491_4f6c_dd1d_u64;
    for length in [10_usize, 64, 129] {
        let mut values: Vec<u32> = (0..length)
            .map(|_| {
                state ^= state << 13;
                state ^= state >> 7;
                state ^= state << 17;
                (state >> 32) as u32
            })
            .collect();
        let expected = sorted_by_std(&values);
        sort(&mut values);
        assert_eq!(values, expected);
    }
}

#[test]
fn sifting_a_leaf_in_a_maximum_length_zero_sized_heap_does_not_overflow() {
    // A zero-sized slice can exceed isize::MAX without an allocation. A leaf's
    // conceptual child index may exceed usize::MAX, but there is no child to
    // visit and no comparison to perform.
    #[derive(Clone, Copy, Eq, PartialEq)]
    struct Leaf;
    impl Ord for Leaf {
        fn cmp(&self, _: &Self) -> core::cmp::Ordering {
            panic!("a leaf has no children to compare");
        }
    }
    impl PartialOrd for Leaf {
        fn partial_cmp(&self, other: &Self) -> Option<core::cmp::Ordering> {
            Some(self.cmp(other))
        }
    }
    let mut values = [Leaf; usize::MAX];
    super::sift_down(&mut values, usize::MAX - 1);
}

#[test]
fn comparisons_stay_within_n_log_n_on_sorted_and_reversed_input() {
    use core::cell::Cell;
    use core::cmp::Ordering;

    #[derive(Clone, Copy, Eq, PartialEq)]
    struct Counted<'a>(u32, &'a Cell<usize>);
    impl Ord for Counted<'_> {
        fn cmp(&self, other: &Self) -> Ordering {
            self.1.set(self.1.get() + 1);
            self.0.cmp(&other.0)
        }
    }
    impl PartialOrd for Counted<'_> {
        fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
            Some(self.cmp(other))
        }
    }
    let length = 1024_u32;
    let bound = 4 * length as usize * 10;
    for reversed in [false, true] {
        let comparisons = Cell::new(0);
        let mut values: Vec<Counted<'_>> = (0..length)
            .map(|value| Counted(if reversed { length - value } else { value }, &comparisons))
            .collect();
        sort(&mut values);
        assert!(values.windows(2).all(|pair| pair[0].0 <= pair[1].0));
        assert!(
            comparisons.get() <= bound,
            "{} comparisons",
            comparisons.get()
        );
    }
}
