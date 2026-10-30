//! A frame's immutable parts are its sizing and writing authority.

use crate::data::{AMSDU_SUBFRAME_HEADER_LEN, MsduParts};
mod fixed;
pub(crate) use fixed::write_array;

/// Admit both ranges before moving overlapping bytes. The borrowed enclosing
/// region proves every read/write boundary; each split separates one output
/// byte from its still-unmodified source. No scratch payload is needed.
pub(crate) fn move_within(
    storage: &mut [u8],
    source: core::ops::Range<usize>,
    destination: usize,
) -> Result<(), oer_wire::Error> {
    let length = source
        .end
        .checked_sub(source.start)
        .ok_or(oer_wire::Error::ValueOutOfRange)?;
    let destination_end = destination
        .checked_add(length)
        .ok_or(oer_wire::Error::LengthOverflow)?;
    let required = source.end.max(destination_end);
    let remaining = storage.len();
    let mut region = storage
        .get_mut(source.start.min(destination)..required)
        .ok_or(oer_wire::Error::Truncated {
            required,
            remaining,
        })?;
    if destination > source.start {
        // Positive separation; subtracting one selects a source in `rest`,
        // which excludes the destination byte just split off.
        let gap = destination
            .abs_diff(source.start)
            .checked_sub(1)
            .ok_or(oer_wire::Error::ValueOutOfRange)?;
        while let Some((output, rest)) = region.split_last_mut() {
            let Some(&input) = rest.iter().rev().nth(gap) else {
                break;
            };
            *output = input;
            region = rest;
        }
    } else if destination < source.start {
        let gap = destination
            .abs_diff(source.start)
            .checked_sub(1)
            .ok_or(oer_wire::Error::ValueOutOfRange)?;
        while let Some((output, rest)) = region.split_first_mut() {
            let Some(&input) = rest.get(gap) else { break };
            *output = input;
            region = rest;
        }
    }
    Ok(())
}

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
    /// A big-endian 16-bit field: a subframe's MSDU length or EtherType.
    Word([u8; 2]),
}

impl Fragment<'_> {
    fn bytes(&self) -> &[u8] {
        match self {
            Self::Borrowed(bytes) => bytes,
            Self::Word(bytes) => bytes,
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
        let mut writer = oer_wire::Writer::new(frame);
        for part in self.0 {
            writer.write(part.bytes()).map_err(|_| required)?;
        }
        Ok(frame.len())
    }
}

/// An immutable aggregate whose MSDU lengths and wire limit were checked
/// before borrowing output. Its fragments define both size and bytes.
///
/// SOURCE: IEEE Std 802.11-2020, 9.3.2.2 (A-MSDU format): each subframe is
/// DA, SA, the big-endian MSDU length, the RFC 1042 LLC/SNAP MSDU and, but
/// for the last, padding to four octets.
pub(crate) struct AmsduBody<'a> {
    frames: &'a [MsduParts<'a>],
}

pub(crate) enum AmsduError {
    Empty,
    TooLong(usize),
}

impl<'a> AmsduBody<'a> {
    pub(crate) fn checked(frames: &'a [MsduParts<'a>], maximum: usize) -> Result<Self, AmsduError> {
        use crate::data::LLC_SNAP_HEADER_LEN;
        if frames.is_empty() {
            return Err(AmsduError::Empty);
        }
        for frame in frames {
            let length = frame
                .payload
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
        use crate::data::{LLC_SNAP_HEADER_LEN, RFC1042_LLC_SNAP_PREFIX};
        let count = self.frames.len();
        self.frames
            .iter()
            .enumerate()
            .flat_map(move |(index, frame)| {
                // Admission proves payload + LLC <= u16::MAX. Neither this
                // sum nor the subframe sum below can saturate.
                const {
                    if !(usize::MAX - AMSDU_SUBFRAME_HEADER_LEN >= u16::MAX as usize) {
                        panic!("invalid fixed wire geometry");
                    };
                }
                let msdu_length = frame.payload.len().saturating_add(LLC_SNAP_HEADER_LEN);
                let padding: &[u8] = if Some(index) == count.checked_sub(1) {
                    &[]
                } else {
                    match AMSDU_SUBFRAME_HEADER_LEN.saturating_add(msdu_length) % 4 {
                        0 => &[],
                        1 => &[0; 3],
                        2 => &[0; 2],
                        _ => &[0; 1],
                    }
                };
                [
                    Fragment::Borrowed(&frame.destination),
                    Fragment::Borrowed(&frame.source),
                    Fragment::Word((msdu_length as u16).to_be_bytes()),
                    Fragment::Borrowed(&RFC1042_LLC_SNAP_PREFIX),
                    Fragment::Word(frame.ether_type.to_be_bytes()),
                    Fragment::Borrowed(frame.payload),
                    Fragment::Borrowed(padding),
                ]
            })
    }
}

#[cfg(test)]
mod tests {
    use super::FrameParts;

    #[test]
    fn overlapping_moves_match_memmove_and_refuse_invalid_ranges_without_writes() {
        use super::move_within;
        // Every overlap, direction, empty range and endpoint in a small owner.
        let original = [0, 1, 2, 3, 4, 5, 6, 7];
        for start in 0..=9 {
            for end in 0..=9 {
                for destination in 0..=9 {
                    let mut actual = original;
                    let result = move_within(&mut actual, start..end, destination);
                    if start <= end
                        && end <= original.len()
                        && destination + (end - start) <= original.len()
                    {
                        let mut expected = original;
                        expected.copy_within(start..end, destination);
                        assert_eq!(result, Ok(()));
                        assert_eq!(actual, expected, "{start}..{end} -> {destination}");
                    } else {
                        assert!(result.is_err(), "{start}..{end} -> {destination}");
                        assert_eq!(actual, original);
                    }
                }
            }
        }
        let mut storage = original;
        assert_eq!(
            move_within(&mut storage, 0..1, usize::MAX),
            Err(oer_wire::Error::LengthOverflow)
        );
        assert_eq!(
            move_within(&mut storage, core::ops::Range { start: 5, end: 3 }, 0),
            Err(oer_wire::Error::ValueOutOfRange)
        );
        assert_eq!(
            move_within(&mut storage, 0..9, 0),
            Err(oer_wire::Error::Truncated {
                required: 9,
                remaining: 8
            })
        );
        assert_eq!(
            move_within(&mut storage, 0..2, 7),
            Err(oer_wire::Error::Truncated {
                required: 9,
                remaining: 8
            })
        );
        assert_eq!(storage, original);
    }

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

    fn msdu(destination: [u8; 6], source: [u8; 6], payload: &[u8]) -> super::MsduParts<'_> {
        super::MsduParts {
            destination,
            source,
            ether_type: 0x0800,
            payload,
        }
    }

    #[test]
    fn oversized_and_empty_aggregates_are_refused_before_writing() {
        use super::{AmsduBody, AmsduError};
        let too_long = std::vec![0; 65528];
        assert!(matches!(
            AmsduBody::checked(&[msdu([0; 6], [0; 6], &too_long)], usize::MAX),
            Err(AmsduError::TooLong(65536))
        ));
        let fits = std::vec![0; 3839 - 14 - 8 + 1];
        assert!(matches!(
            AmsduBody::checked(&[msdu([0; 6], [0; 6], &fits)], 3839),
            Err(AmsduError::TooLong(3840))
        ));
        assert!(matches!(
            AmsduBody::checked(&[], 3839),
            Err(AmsduError::Empty)
        ));
    }

    #[test]
    fn admitted_msdus_preserve_every_length_and_padding_class() {
        use super::AmsduBody;
        // Exercise all alignments and the largest u16 MSDU admitted by the
        // body proof, independently of the station/AP's smaller wire limit.
        for length in [0, 1, 2, 3, 4, 15, 1_500, 3_800, 65_527] {
            let payload = std::vec![0x5a; length];
            let frames = [
                msdu([0, 1, 2, 3, 4, 5], [6, 7, 8, 9, 10, 11], &payload),
                super::MsduParts {
                    destination: [0xa5; 6],
                    source: [0xa5; 6],
                    ether_type: 0xa5a5,
                    payload: &[],
                },
            ];
            let Ok(body) = AmsduBody::checked(&frames, usize::MAX) else {
                panic!("valid MSDUs within the u16 MSDU limit");
            };
            // DA, SA, length, RFC 1042 LLC/SNAP and EtherType, payload, pad.
            let mut expected = std::vec![0, 1, 2, 3, 4, 5, 6, 7, 8, 9, 10, 11];
            expected.extend_from_slice(&((length + 8) as u16).to_be_bytes());
            expected.extend_from_slice(&[0xaa, 0xaa, 0x03, 0, 0, 0, 0x08, 0x00]);
            expected.extend_from_slice(&payload);
            expected.resize(expected.len().next_multiple_of(4), 0);
            expected.extend_from_slice(&[0xa5; 12]);
            expected.extend_from_slice(&[0, 8]);
            expected.extend_from_slice(&[0xaa, 0xaa, 0x03, 0, 0, 0, 0xa5, 0xa5]);
            let mut output = std::vec![0xcc; expected.len()];
            assert_eq!(
                FrameParts([]).write_amsdu(body, &mut output),
                Ok(expected.len())
            );
            assert_eq!(output, expected);
        }
    }
}
