//! Checked A-MPDU delimiter and padding geometry shared by admission and
//! encoding. A final MPDU has neither alignment padding nor empty delimiters;
//! those bytes become part of the aggregate only when another MPDU follows.

/// Empty four-octet delimiters needed after an aligned PSDU and its delimiter
/// to meet `minimum_subframe_bytes`. The PSDU includes its MIC and FCS.
pub const fn empty_delimiters(psdu_bytes: u32, minimum_subframe_bytes: u32) -> Option<u32> {
    if psdu_bytes == 0 {
        return None;
    }
    let Some(rounded) = psdu_bytes.checked_add(3) else {
        return None;
    };
    let Some(complete) = (rounded & !3).checked_add(4) else {
        return None;
    };
    Some(minimum_subframe_bytes.saturating_sub(complete).div_ceil(4))
}

/// Length after appending a PSDU. `previous` is the previous final PSDU's
/// length and its number of empty delimiters. `length` includes that PSDU's
/// delimiter but excludes its trailing padding and empty delimiters.
/// Zero PSDUs and arithmetic overflow are refused without a partial length.
pub const fn length_after_append(
    length: u32,
    previous: Option<(u32, u32)>,
    psdu_bytes: u32,
) -> Option<u32> {
    if psdu_bytes == 0 {
        return None;
    }
    let mut next = length;
    if let Some((previous_bytes, delimiters)) = previous {
        if previous_bytes == 0 {
            return None;
        }
        let Some(empty_bytes) = delimiters.checked_mul(4) else {
            return None;
        };
        let Some(padded) = next.checked_add((4 - (previous_bytes & 3)) & 3) else {
            return None;
        };
        let Some(spaced) = padded.checked_add(empty_bytes) else {
            return None;
        };
        next = spaced;
    }
    let Some(delimited) = next.checked_add(4) else {
        return None;
    };
    delimited.checked_add(psdu_bytes)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn encoded_positions_include_only_nonfinal_padding_and_delimiters() {
        for previous_psdu in 1..=20 {
            for minimum in 0..=64 {
                let delimiters = empty_delimiters(previous_psdu, minimum).unwrap();
                // An independent encoder: delimiter, PSDU, byte padding,
                // then whole empty delimiters until the required position.
                let mut encoded = 4 + previous_psdu;
                while encoded % 4 != 0 {
                    encoded += 1;
                }
                let aligned = encoded;
                while encoded < minimum {
                    encoded += 4;
                }
                assert_eq!(delimiters, (encoded - aligned) / 4);
                let one = length_after_append(0, None, previous_psdu).unwrap();
                assert_eq!(one, 4 + previous_psdu);
                for final_psdu in 1..=8 {
                    assert_eq!(
                        length_after_append(one, Some((previous_psdu, delimiters)), final_psdu),
                        Some(encoded + 4 + final_psdu)
                    );
                }
            }
        }
    }

    #[test]
    fn invalid_lengths_and_overflow_have_no_encoded_length() {
        assert_eq!(empty_delimiters(0, 100), None);
        assert_eq!(empty_delimiters(u32::MAX, 100), None);
        assert_eq!(length_after_append(0, None, 0), None);
        assert_eq!(length_after_append(0, None, u32::MAX), None);
        assert_eq!(length_after_append(u32::MAX, None, 1), None);
        assert_eq!(length_after_append(5, Some((0, 0)), 1), None);
        assert_eq!(length_after_append(5, Some((1, u32::MAX)), 1), None);
        assert_eq!(length_after_append(u32::MAX - 1, Some((1, 0)), 1), None);
    }
}
