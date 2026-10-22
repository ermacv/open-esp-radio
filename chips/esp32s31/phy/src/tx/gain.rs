//! Small arithmetic state shared by the whole-image and stepped publishers.

#[derive(Clone, Copy, Debug, Default)]
pub(crate) struct Generator {
    pub target: i16,
    pub interpolation: i8,
    pub table_index: u8,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) struct Value {
    pub gain_72: u16,
    pub gain_64: u16,
    pub gain_32: u8,
}
