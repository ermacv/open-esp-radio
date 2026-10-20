//! A frame's immutable parts are its sizing and writing authority.

/// The parts retain every byte through capacity admission and publication.
/// There is no independent length or final-offset field to disagree with the
/// frame. Empty optional elements contribute neither bytes nor capacity.
pub(crate) struct FrameParts<'a, const N: usize>(pub(crate) [&'a [u8]; N]);

impl<'a, const N: usize> FrameParts<'a, N> {
    /// Reject a short destination before writing any byte. The error carries
    /// the required capacity; an unrepresentable total requires `usize::MAX`.
    pub(crate) fn write(self, output: &mut [u8]) -> Result<usize, usize> {
        FragmentPlan(self.0.into_iter().map(Fragment::Borrowed)).write(output)
    }

    pub(crate) fn write_amsdu(
        self,
        body: AmsduBody<'a>,
        output: &mut [u8],
    ) -> Result<usize, usize> {
        FragmentPlan(
            self.0
                .into_iter()
                .map(Fragment::Borrowed)
                .chain(body.fragments()),
        )
        .write(output)
    }
}

#[derive(Clone)]
enum Fragment<'a> {
    Borrowed(&'a [u8]),
    Length([u8; 2]),
}

impl Fragment<'_> {
    fn bytes(&self) -> &[u8] {
        match self {
            Self::Borrowed(bytes) => bytes,
            Self::Length(bytes) => bytes,
        }
    }
}

struct FragmentPlan<I>(I);

impl<'a, I: Clone + Iterator<Item = Fragment<'a>>> FragmentPlan<I> {
    fn length(&self) -> Result<usize, usize> {
        self.0.clone().try_fold(0_usize, |size, part| {
            size.checked_add(part.bytes().len()).ok_or(usize::MAX)
        })
    }

    fn write(self, output: &mut [u8]) -> Result<usize, usize> {
        let required = self.length()?;
        let frame = output.get_mut(..required).ok_or(required)?;
        let mut destination = frame.iter_mut();
        for part in self.0 {
            for (out, byte) in destination
                .by_ref()
                .take(part.bytes().len())
                .zip(part.bytes())
            {
                *out = *byte;
            }
        }
        Ok(frame.len())
    }
}

/// An immutable aggregate whose Ethernet boundaries and wire limit were
/// checked before borrowing output. Its fragments define both size and bytes.
pub(crate) struct AmsduBody<'a> {
    frames: &'a [&'a [u8]],
}

pub(crate) enum AmsduError {
    Empty,
    ShortEthernet,
    TooLong(usize),
}

impl<'a> AmsduBody<'a> {
    pub(crate) fn checked(frames: &'a [&'a [u8]], maximum: usize) -> Result<Self, AmsduError> {
        use crate::data::{ETHERNET_HEADER_LEN, LLC_SNAP_HEADER_LEN};
        if frames.is_empty() {
            return Err(AmsduError::Empty);
        }
        for frame in frames {
            let (_, payload) = frame
                .split_first_chunk::<ETHERNET_HEADER_LEN>()
                .ok_or(AmsduError::ShortEthernet)?;
            let length = payload
                .len()
                .checked_add(LLC_SNAP_HEADER_LEN)
                .ok_or(AmsduError::TooLong(usize::MAX))?;
            if length > usize::from(u16::MAX) {
                return Err(AmsduError::TooLong(length));
            }
        }
        let body = Self { frames };
        let length = body.length().map_err(AmsduError::TooLong)?;
        if length > maximum {
            return Err(AmsduError::TooLong(length));
        }
        Ok(body)
    }

    pub(crate) fn length(&self) -> Result<usize, usize> {
        FragmentPlan(self.fragments()).length()
    }

    fn fragments(&self) -> impl Clone + Iterator<Item = Fragment<'a>> + use<'a> {
        use crate::data::{ETHERNET_HEADER_LEN, LLC_SNAP_HEADER_LEN, RFC1042_LLC_SNAP_PREFIX};
        let count = self.frames.len();
        self.frames
            .iter()
            .copied()
            .enumerate()
            .flat_map(move |(index, frame)| {
                let payload = &frame[ETHERNET_HEADER_LEN..];
                let msdu_length = LLC_SNAP_HEADER_LEN + payload.len();
                let padding: &[u8] = if index + 1 == count {
                    &[]
                } else {
                    match (ETHERNET_HEADER_LEN + msdu_length) % 4 {
                        0 => &[],
                        1 => &[0; 3],
                        2 => &[0; 2],
                        _ => &[0; 1],
                    }
                };
                [
                    Fragment::Borrowed(&frame[..12]),
                    Fragment::Length((msdu_length as u16).to_be_bytes()),
                    Fragment::Borrowed(&RFC1042_LLC_SNAP_PREFIX),
                    Fragment::Borrowed(&frame[12..14]),
                    Fragment::Borrowed(payload),
                    Fragment::Borrowed(padding),
                ]
            })
    }
}

#[cfg(test)]
mod tests {
    use super::FrameParts;

    #[test]
    fn capacity_admission_precedes_writing_and_preserves_the_unused_tail() {
        let parts = [b"header".as_slice(), b"".as_slice(), b"body".as_slice()];
        let mut short = [0xa5; 9];
        assert_eq!(FrameParts(parts).write(&mut short), Err(10));
        assert_eq!(short, [0xa5; 9]);

        let mut output = [0xa5; 12];
        assert_eq!(FrameParts(parts).write(&mut output), Ok(10));
        assert_eq!(&output[..10], b"headerbody");
        assert_eq!(&output[10..], &[0xa5; 2]);
    }
}
