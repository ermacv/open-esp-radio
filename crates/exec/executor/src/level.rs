//! The compile-time checks of interrupt placements, on plain level and core
//! numbers so that the host tests them.

/// A source's level and core, as their numbers.
pub(crate) type Placement = (u8, u8);

/// Panic unless `radio` and `bell` share one level and one core. Evaluated
/// in a constant, the panic is a compile error that carries the message.
pub(crate) const fn same_level(radio: Placement, bell: Placement) {
    assert!(
        radio.0 == bell.0,
        "the doorbell rings at the radio interrupt's level"
    );
    assert!(
        radio.1 == bell.1,
        "the doorbell rings on the radio interrupt's core"
    );
}

#[cfg(test)]
mod tests {
    use super::*;

    // Evaluated when the tests compile, as in an image.
    const _: () = same_level((3, 0), (3, 0));

    #[test]
    fn one_level_on_one_core_is_accepted() {
        same_level((3, 1), (3, 1));
    }

    #[test]
    #[should_panic(expected = "the doorbell rings at the radio interrupt's level")]
    fn a_doorbell_at_another_level_is_refused() {
        same_level((3, 0), (2, 0));
    }

    #[test]
    #[should_panic(expected = "the doorbell rings on the radio interrupt's core")]
    fn a_doorbell_on_another_core_is_refused() {
        same_level((3, 0), (3, 1));
    }
}
