//! Small arithmetic state shared by the whole-image and stepped publishers.

#[derive(Clone, Copy, Debug, Default)]
pub(crate) struct Generator {
    pub target: i16,
    pub interpolation: i8,
    pub table_index: GainPosition,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) struct Value {
    pub gain_72: u16,
    pub gain_64: u16,
    pub gain_32: u8,
}

/// A position in a transmit gain table.
pub(crate) type GainPosition = crate::ordinal::Ordinal<18>;

/// Walk `table` from `index` to the entry bracketing `target`.
pub(crate) const fn select_gain_position(
    table: &[u16; 18],
    mut index: GainPosition,
    target: i16,
) -> GainPosition {
    let mut iteration = 0;
    while iteration != table.len() {
        if target < index.at(table) as i16 {
            let Some(next) = index.next() else {
                break;
            };
            index = next;
            if target >= next.at(table) as i16 {
                break;
            }
        } else {
            let Some(previous) = index.previous() else {
                break;
            };
            if target < previous.at(table) as i16 {
                break;
            }
            index = previous;
        }
        iteration += 1;
    }
    index
}
