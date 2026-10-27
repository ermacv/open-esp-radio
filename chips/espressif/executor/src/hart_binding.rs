//! The executor route and its affine execution token name the same hart.

pub(crate) fn check<const H: u8, W: oer_espressif_interrupt_table::Entry>() {
    const { assert!(W::HART == H, "executor route is on this hart") };
}
