//! One information element: Element ID, Length and body.
//!
//! SOURCE: IEEE Std 802.11-2020, 9.4.2.1 (Elements: general).
//!
//! Received bytes reach this module; nothing here panics on them.
#![deny(
    clippy::indexing_slicing,
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::unreachable,
    clippy::arithmetic_side_effects
)]

/// One element borrowed from the caller's bytes.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct Element<'a> {
    pub id: u8,
    pub body: &'a [u8],
    /// The complete element: Element ID, Length and body.
    pub bytes: &'a [u8],
}

impl<'a> Element<'a> {
    /// The element at the front of `bytes` and what follows it; `None` when
    /// its Length runs past `bytes`.
    pub fn split(bytes: &'a [u8]) -> Option<(Self, &'a [u8])> {
        let (&[id, length], after_header) = bytes.split_first_chunk::<2>()?;
        let (body, rest) = after_header.split_at_checked(usize::from(length))?;
        let (element, _) = bytes.split_at_checked(usize::from(length).checked_add(2)?)?;
        Some((
            Self {
                id,
                body,
                bytes: element,
            },
            rest,
        ))
    }

    /// The one element `bytes` holds exactly; `None` for fewer or more
    /// octets.
    pub fn exact(bytes: &'a [u8]) -> Option<Self> {
        match Self::split(bytes)? {
            (element, []) => Some(element),
            _ => None,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn an_element_splits_at_its_length_and_an_exact_one_has_nothing_after() {
        let bytes = [48, 2, 1, 0, 244, 1, 0x20];
        let (rsn, rest) = Element::split(&bytes).unwrap();
        assert_eq!(
            (rsn.id, rsn.body, rsn.bytes),
            (48, &[1, 0][..], &bytes[..4])
        );
        assert_eq!(rest, &[244, 1, 0x20]);
        assert_eq!(
            Element::exact(rest),
            Some(Element {
                id: 244,
                body: &[0x20],
                bytes: rest
            })
        );
        assert_eq!(Element::exact(&bytes), None);
        assert_eq!(Element::split(&[48, 3, 1, 0]), None);
        assert_eq!(Element::split(&[48]), None);
    }
}
