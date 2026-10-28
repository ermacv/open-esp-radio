//! HE-SIG-B user streams of the chip's RX report: the complete HE20 SIG-B
//! bytes the report carries, cut into the user fields the vendor parsers
//! retain. The per-field codecs (`oer_ieee80211_wire::he`) stay air formats;
//! the stream bounds and user limits here are those of the pinned `libpp`
//! RX parsers.

use oer_ieee80211_wire::he::{
    HE_MU_SIG_B_USER_BITS, HE20_MU_SIG_B_COMMON_BITS, HE20_MU_SIG_B_USER_PAIR_BITS,
    He20MuSigBRuAllocation, He20MuSigBRuAllocationError, HeMuMimoSpatialConfiguration,
    HeMuSigBMimoUser, HeMuSigBNonMimoUser, read_sig_b_user_field,
};

/// Maximum number of users in the blob's complete non-MU-MIMO HE20 parser.
pub const HE20_MU_SIG_B_MAX_USERS: u8 = 9;
/// Maximum number of users retained by the blob's compressed/MU-MIMO parser.
pub const HE20_MU_SIG_B_MIMO_MAX_USERS: u8 = 4;

/// A failure to construct a bounded HE20 non-MU-MIMO complete-SIG-B view.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum He20MuSigBNonMimoStreamError {
    BitLengthBeforeFirstUser,
    CompleteBytesTooShort,
    IncompleteUserField,
    TooManyUsers,
}

/// Why the common RU Allocation and complete HE20 user stream disagree.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum He20MuSigBRuStreamError {
    Allocation(He20MuSigBRuAllocationError),
    UserCountMismatch {
        stream_users: u8,
        allocation_users: u8,
    },
}

/// One user recovered from the complete HE20 non-MU-MIMO HE-SIG-B stream.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct He20MuSigBNonMimoEntry {
    pub index: u8,
    pub bit_offset: u16,
    pub raw: u32,
    pub user: HeMuSigBNonMimoUser,
}

/// Allocation-free iterator over an HE20 non-MU-MIMO complete HE-SIG-B stream.
///
/// SOURCE(esp32s31)\[BLOB_LIBPP_TEST_RX_PARSE_NONMUMIMO_COMPLETE_SIGB]: complete
/// `libpp[test_hal_rx_mu_sigb.o]::test_rx_parse_nonmumimo_complete_sigb`, size `0x3e4`.
/// The RISC-V body copies the complete bytes from RX offset `0x38`, calls
/// `libpp[test_hal_rx_mu_sigb.o]::test_get_nonmumimo_common`, and extracts the tested HE20 user words at
/// absolute bit offsets `18,39,70,91,122,143,174,195,226`. These are exactly
/// `18 + pair * 52 + {0,21}`. Its user-count expression is
/// `(remaining / 52) * 2 + (remaining % 52 != 0)`, and the unrolled body stops
/// after user eight.
///
/// This type intentionally says HE20 and non-MU-MIMO. The same oracle proves
/// different common lengths for wider bandwidth selectors, while
/// `libpp[test_hal_rx_mu_sigb.o]::test_rx_parse_mumimo_complete_sigb` uses a configuration-dependent layout.
/// Neither is silently folded onto this iterator.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct He20MuSigBNonMimoUsers<'a> {
    complete_bytes: &'a [u8],
    bit_length: u16,
    user_count: u8,
    next_user: u8,
}

impl<'a> He20MuSigBNonMimoUsers<'a> {
    pub fn try_new(
        complete_bytes: &'a [u8],
        bit_length: u16,
    ) -> Result<Self, He20MuSigBNonMimoStreamError> {
        let remaining = bit_length
            .checked_sub(HE20_MU_SIG_B_COMMON_BITS)
            .ok_or(He20MuSigBNonMimoStreamError::BitLengthBeforeFirstUser)?;
        // bit_length is u16: its rounded byte count fits usize.
        let required_bytes = (usize::from(bit_length) / 8)
            .saturating_add(usize::from(!bit_length.is_multiple_of(8)));
        if complete_bytes.len() < required_bytes {
            return Err(He20MuSigBNonMimoStreamError::CompleteBytesTooShort);
        }

        let complete_pairs = remaining / HE20_MU_SIG_B_USER_PAIR_BITS;
        let partial_pair_bits = remaining % HE20_MU_SIG_B_USER_PAIR_BITS;
        if partial_pair_bits != 0 && partial_pair_bits < HE_MU_SIG_B_USER_BITS {
            return Err(He20MuSigBNonMimoStreamError::IncompleteUserField);
        }
        let user_count = complete_pairs
            .checked_mul(2)
            .and_then(|count| count.checked_add(u16::from(partial_pair_bits != 0)))
            .ok_or(He20MuSigBNonMimoStreamError::TooManyUsers)?;
        if user_count > u16::from(HE20_MU_SIG_B_MAX_USERS) {
            return Err(He20MuSigBNonMimoStreamError::TooManyUsers);
        }

        Ok(Self {
            complete_bytes,
            bit_length,
            user_count: user_count as u8,
            next_user: 0,
        })
    }

    pub const fn bit_length(&self) -> u16 {
        self.bit_length
    }

    pub const fn user_count(&self) -> u8 {
        self.user_count
    }

    /// Decodes the first HE20 common-information RU Allocation byte and
    /// requires it to describe the same number of users as this stream.
    ///
    /// SOURCE(esp32s31)\[BLOB_LIBPP_TEST_GET_NONMUMIMO_COMMON]: complete
    /// `libpp[test_hal_rx_mu_sigb.o]::test_get_nonmumimo_common`, size `0xf6`.
    /// Bandwidth selector zero loads byte zero as the sole RU Allocation and
    /// returns an 18-bit common prefix.
    #[deny(
        clippy::indexing_slicing,
        clippy::unwrap_used,
        clippy::expect_used,
        clippy::panic,
        clippy::unreachable,
        clippy::arithmetic_side_effects
    )]
    pub fn ru_allocation(self) -> Result<He20MuSigBRuAllocation, He20MuSigBRuStreamError> {
        let common =
            self.complete_bytes
                .first()
                .copied()
                .ok_or(He20MuSigBRuStreamError::Allocation(
                    He20MuSigBRuAllocationError::ReservedEncoding,
                ))?;
        let allocation =
            He20MuSigBRuAllocation::try_new(common).map_err(He20MuSigBRuStreamError::Allocation)?;
        if allocation.user_count() != self.user_count {
            return Err(He20MuSigBRuStreamError::UserCountMismatch {
                stream_users: self.user_count,
                allocation_users: allocation.user_count(),
            });
        }
        Ok(allocation)
    }

    #[deny(
        clippy::indexing_slicing,
        clippy::unwrap_used,
        clippy::expect_used,
        clippy::panic,
        clippy::unreachable,
        clippy::arithmetic_side_effects
    )]
    fn read_user_word(&self, index: u8) -> Option<(u16, u32)> {
        let pair = u16::from(index / 2);
        let within_pair = if index & 1 == 0 {
            0
        } else {
            HE_MU_SIG_B_USER_BITS
        };
        let bit_offset = pair
            .checked_mul(HE20_MU_SIG_B_USER_PAIR_BITS)?
            .checked_add(HE20_MU_SIG_B_COMMON_BITS)?
            .checked_add(within_pair)?;
        let raw = read_sig_b_user_field(self.complete_bytes, bit_offset)?;
        Some((bit_offset, raw))
    }
}

impl Iterator for He20MuSigBNonMimoUsers<'_> {
    type Item = He20MuSigBNonMimoEntry;

    #[deny(
        clippy::indexing_slicing,
        clippy::unwrap_used,
        clippy::expect_used,
        clippy::panic,
        clippy::unreachable,
        clippy::arithmetic_side_effects
    )]
    fn next(&mut self) -> Option<Self::Item> {
        if self.next_user >= self.user_count {
            return None;
        }
        let index = self.next_user;
        let (bit_offset, raw) = self.read_user_word(index)?;
        self.next_user = self.next_user.saturating_add(1);
        Some(He20MuSigBNonMimoEntry {
            index,
            bit_offset,
            raw,
            user: HeMuSigBNonMimoUser::decode(raw),
        })
    }

    fn size_hint(&self) -> (usize, Option<usize>) {
        let remaining = (self.next_user..self.user_count).len();
        (remaining, Some(remaining))
    }
}

impl ExactSizeIterator for He20MuSigBNonMimoUsers<'_> {}
impl core::iter::FusedIterator for He20MuSigBNonMimoUsers<'_> {}

/// A failure to construct the bounded HE20 compressed/MU-MIMO SIG-B view.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum He20MuSigBMimoStreamError {
    UserCountOutOfRange,
    CompleteBytesTooShort,
    IncompleteUserField,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum He20MuSigBMimoSpatialError {
    UnsupportedUserCountOrEncoding,
    InconsistentEncoding,
}

/// One MU-MIMO user recovered from a complete compressed HE20 SIG-B stream.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct He20MuSigBMimoEntry {
    pub index: u8,
    pub bit_offset: u16,
    pub raw: u32,
    pub user: HeMuSigBMimoUser,
}

/// Allocation-free iterator over the blob's HE20 compressed/MU-MIMO layout.
///
/// SOURCE(esp32s31)\[BLOB_LIBPP_TEST_RX_PARSE_MUMIMO_COMPLETE_SIGB]: complete
/// `libpp[test_hal_rx_mu_sigb.o]::test_rx_parse_mumimo_complete_sigb`, size `0x20c`.
/// The body derives a one-based user count from HE-SIG-A1 bits 21:18, copies
/// 16 complete-SIG-B bytes and extracts at most four 21-bit words at exact
/// bit offsets `0,21,52,105`. The third and fourth paths require total bit
/// lengths above 72 and 135 respectively. The non-linear fourth offset is
/// retained deliberately instead of imposing the non-MIMO pair geometry.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct He20MuSigBMimoUsers<'a> {
    complete_bytes: &'a [u8],
    bit_length: u16,
    user_count: u8,
    next_user: u8,
}

impl<'a> He20MuSigBMimoUsers<'a> {
    const USER_BIT_OFFSETS: [u16; HE20_MU_SIG_B_MIMO_MAX_USERS as usize] = [0, 21, 52, 105];
    // The first two bounds are the safe completion of the fields the blob
    // reads. The last two are the blob's explicit `> 72` and `> 135` guards.
    const REQUIRED_BIT_LENGTHS: [u16; HE20_MU_SIG_B_MIMO_MAX_USERS as usize] = [21, 42, 73, 136];

    #[deny(
        clippy::indexing_slicing,
        clippy::unwrap_used,
        clippy::expect_used,
        clippy::panic,
        clippy::unreachable,
        clippy::arithmetic_side_effects
    )]
    pub fn try_new(
        complete_bytes: &'a [u8],
        bit_length: u16,
        user_count: u8,
    ) -> Result<Self, He20MuSigBMimoStreamError> {
        if user_count == 0 || user_count > HE20_MU_SIG_B_MIMO_MAX_USERS {
            return Err(He20MuSigBMimoStreamError::UserCountOutOfRange);
        }
        // bit_length is u16: its rounded byte count fits usize.
        let required_bytes = (usize::from(bit_length) / 8)
            .saturating_add(usize::from(!bit_length.is_multiple_of(8)));
        if complete_bytes.len() < required_bytes {
            return Err(He20MuSigBMimoStreamError::CompleteBytesTooShort);
        }
        let required_bit_length = usize::from(user_count)
            .checked_sub(1)
            .and_then(|index| Self::REQUIRED_BIT_LENGTHS.get(index))
            .ok_or(He20MuSigBMimoStreamError::UserCountOutOfRange)?;
        if bit_length < *required_bit_length {
            return Err(He20MuSigBMimoStreamError::IncompleteUserField);
        }
        Ok(Self {
            complete_bytes,
            bit_length,
            user_count,
            next_user: 0,
        })
    }

    pub const fn bit_length(&self) -> u16 {
        self.bit_length
    }

    pub const fn user_count(&self) -> u8 {
        self.user_count
    }

    /// Validates the shared spatial configuration exactly as the blob test
    /// does after extracting the per-user words.
    pub fn spatial_configuration(
        self,
    ) -> Result<HeMuMimoSpatialConfiguration, He20MuSigBMimoSpatialError> {
        let mut users = self;
        let encoding = users
            .next()
            .map(|entry| entry.user.spatial_configuration)
            .ok_or(He20MuSigBMimoSpatialError::UnsupportedUserCountOrEncoding)?;
        if users.any(|entry| entry.user.spatial_configuration != encoding) {
            return Err(He20MuSigBMimoSpatialError::InconsistentEncoding);
        }
        HeMuMimoSpatialConfiguration::try_new(self.user_count, encoding)
            .ok_or(He20MuSigBMimoSpatialError::UnsupportedUserCountOrEncoding)
    }
}

impl Iterator for He20MuSigBMimoUsers<'_> {
    type Item = He20MuSigBMimoEntry;

    #[deny(
        clippy::indexing_slicing,
        clippy::unwrap_used,
        clippy::expect_used,
        clippy::panic,
        clippy::unreachable,
        clippy::arithmetic_side_effects
    )]
    fn next(&mut self) -> Option<Self::Item> {
        if self.next_user >= self.user_count {
            return None;
        }
        let index = self.next_user;
        let bit_offset = *Self::USER_BIT_OFFSETS.get(usize::from(index))?;
        let raw = read_sig_b_user_field(self.complete_bytes, bit_offset)?;
        self.next_user = self.next_user.saturating_add(1);
        Some(He20MuSigBMimoEntry {
            index,
            bit_offset,
            raw,
            user: HeMuSigBMimoUser::decode(raw),
        })
    }

    fn size_hint(&self) -> (usize, Option<usize>) {
        let remaining = (self.next_user..self.user_count).len();
        (remaining, Some(remaining))
    }
}

impl ExactSizeIterator for He20MuSigBMimoUsers<'_> {}
impl core::iter::FusedIterator for He20MuSigBMimoUsers<'_> {}

#[cfg(test)]
mod tests;
