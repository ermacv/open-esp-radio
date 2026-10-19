use super::{LinkStateEventWord, item_with_le_1m_power, item_with_power};
use crate::LeTxPower;

fn power(dbm: i8) -> LeTxPower {
    LeTxPower::from_dbm(dbm).expect("provider level")
}

#[test]
fn priority_and_power_replace_only_their_own_byte() {
    let word = LinkStateEventWord::from_word(0xa5a5_a5a5);
    let low = word.with_power(power(-24)).with_priority(1);
    let high = word.with_power(power(21)).with_priority(1);
    assert_eq!(low.power_index(), power(-24).index());
    assert_eq!(high.power_index(), power(21).index());
    // The priority and the upper half are untouched by the power.
    assert_eq!(
        low.word() ^ high.word(),
        ((low.power_index() ^ high.power_index()) as u32) << 8
    );
    assert_eq!(
        low.with_priority(13).power_index(),
        low.power_index(),
        "the priority leaves the power alone"
    );
    assert_eq!(low.word() >> 16, word.word() >> 16);
}

#[test]
fn an_item_takes_the_power_and_an_le_1m_item_also_clears_its_rate() {
    let index = power(9).index();
    let kept = item_with_power(u32::MAX, index);
    let le_1m = item_with_le_1m_power(u32::MAX, index);
    // Both carry the same power; only the LE 1M form clears the rate lanes.
    assert_eq!(kept & 0x000f_ffff, 0x000f_ffff);
    assert_eq!((kept ^ le_1m).count_ones(), 4);
    assert_eq!(
        item_with_power(0, index),
        item_with_le_1m_power(0, index),
        "a zero rate is the LE 1M rate"
    );
}
