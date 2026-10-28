//! Allocation-free parsing for the bounded 2.4-GHz HE20 capability prefix.
//!
//! This module does not enable HE transmission. It owns the stateless peer
//! representation recovered from reviewed vendor behavior. Register
//! programming belongs to the chip MAC/PAC boundary.
//!
//! Received bytes reach the HE Capabilities and HE Operation element parsers
//! (stored scan elements, parsed at association); nothing in them panics on
//! those bytes, and they deny the panic lints item by item.

pub const HE_CAPABILITIES_EXTENSION_ID: u8 = 35;
pub const HE_OPERATION_EXTENSION_ID: u8 = 36;
pub const HE_CAPABILITIES_IE_MIN_LEN: usize = 24;
pub const HE_OPERATION_IE_MIN_LEN: usize = 9;

/// Resource-unit widths representable in the HE20 bandwidth.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum HeResourceUnit {
    Ru26,
    Ru52,
    Ru106,
    #[default]
    Ru242,
}

/// One decoded 21-bit non-MU-MIMO HE-SIG-B user field.
///
/// SOURCE(esp32s31)\[BLOB_LIBPP_DBG_DUMP_MUSIGB_NON_MIMO]: complete
/// `libpp[hal_debug.o]::dbg_dump_musigb_non_mimo`, size `0x6e`.
/// The blob loads one caller-owned word and names bits 10:0 STA-ID, 13:11
/// NSTS, 14 beamformed, 18:15 MCS, 19 DCM and 20 coding. STA-ID `0x7fe`
/// takes a terminal special branch and none of the remaining bits are read.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum HeMuSigBNonMimoUser {
    NonMuMimo,
    Scheduled {
        station_id: u16,
        nsts: u8,
        beamformed: bool,
        mcs: u8,
        dcm: bool,
        ldpc: bool,
    },
}

impl HeMuSigBNonMimoUser {
    pub const fn decode(word: u32) -> Self {
        let station_id = SigBNonMimoUserFields::from_bytes(word.to_le_bytes()).station_id();
        if station_id == 0x07fe {
            Self::NonMuMimo
        } else {
            Self::Scheduled {
                station_id,
                nsts: SigBNonMimoUserFields::from_bytes(word.to_le_bytes()).nsts(),
                beamformed: SigBNonMimoUserFields::from_bytes(word.to_le_bytes()).beamformed(),
                mcs: SigBNonMimoUserFields::from_bytes(word.to_le_bytes()).mcs(),
                dcm: SigBNonMimoUserFields::from_bytes(word.to_le_bytes()).dcm(),
                ldpc: SigBNonMimoUserFields::from_bytes(word.to_le_bytes()).ldpc(),
            }
        }
    }
}

/// One decoded 21-bit MU-MIMO HE-SIG-B user field.
///
/// SOURCE(esp32s31)\[BLOB_LIBPP_DBG_DUMP_MUSIGB_MIMO]: complete
/// `libpp[hal_debug.o]::dbg_dump_musigb_mimo`, size `0x4a`.
/// The blob names bits 10:0 STA-ID, 14:11 spatial configuration, 18:15 MCS,
/// bit 19 reserved and bit 20 coding. The reserved bit is deliberately not
/// exposed as a writable semantic field.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct HeMuSigBMimoUser {
    pub station_id: u16,
    pub spatial_configuration: u8,
    pub mcs: u8,
    pub ldpc: bool,
}

impl HeMuSigBMimoUser {
    pub const fn decode(word: u32) -> Self {
        Self {
            station_id: SigBMimoUserFields::from_bytes(word.to_le_bytes()).station_id(),
            spatial_configuration: SigBMimoUserFields::from_bytes(word.to_le_bytes())
                .spatial_configuration(),
            mcs: SigBMimoUserFields::from_bytes(word.to_le_bytes()).mcs(),
            ldpc: SigBMimoUserFields::from_bytes(word.to_le_bytes()).ldpc(),
        }
    }
}

// SOURCE[ROM_REV0_MU_MIMO_SPECIAL_CFG]: qualified ESP32-S31 rev0 ROM image,
// symbols `mu_mimo_special_cfg_user_num_2` through `_8` at
// 0x2f84fee8, 0x2f84fe80, 0x2f84fe28, 0x2f84fdf0, 0x2f84fdd0,
// 0x2f84fdc0 and 0x2f84fdb8. Each configuration occupies the exact
// eight-byte ROM stride; zero tail bytes are retained below.
const HE_MU_MIMO_NSTS_2: [[u8; 8]; 10] = [
    [1, 1, 0, 0, 0, 0, 0, 0],
    [2, 1, 0, 0, 0, 0, 0, 0],
    [3, 1, 0, 0, 0, 0, 0, 0],
    [4, 1, 0, 0, 0, 0, 0, 0],
    [2, 2, 0, 0, 0, 0, 0, 0],
    [3, 2, 0, 0, 0, 0, 0, 0],
    [4, 2, 0, 0, 0, 0, 0, 0],
    [3, 3, 0, 0, 0, 0, 0, 0],
    [4, 3, 0, 0, 0, 0, 0, 0],
    [4, 4, 0, 0, 0, 0, 0, 0],
];
const HE_MU_MIMO_NSTS_3: [[u8; 8]; 13] = [
    [1, 1, 1, 0, 0, 0, 0, 0],
    [2, 1, 1, 0, 0, 0, 0, 0],
    [3, 1, 1, 0, 0, 0, 0, 0],
    [4, 1, 1, 0, 0, 0, 0, 0],
    [2, 2, 1, 0, 0, 0, 0, 0],
    [3, 2, 1, 0, 0, 0, 0, 0],
    [4, 2, 1, 0, 0, 0, 0, 0],
    [3, 3, 1, 0, 0, 0, 0, 0],
    [4, 3, 1, 0, 0, 0, 0, 0],
    [2, 2, 2, 0, 0, 0, 0, 0],
    [3, 2, 2, 0, 0, 0, 0, 0],
    [4, 2, 2, 0, 0, 0, 0, 0],
    [3, 3, 2, 0, 0, 0, 0, 0],
];
const HE_MU_MIMO_NSTS_4: [[u8; 8]; 11] = [
    [1, 1, 1, 1, 0, 0, 0, 0],
    [2, 1, 1, 1, 0, 0, 0, 0],
    [3, 1, 1, 1, 0, 0, 0, 0],
    [4, 1, 1, 1, 0, 0, 0, 0],
    [2, 2, 1, 1, 0, 0, 0, 0],
    [3, 2, 1, 1, 0, 0, 0, 0],
    [4, 2, 1, 1, 0, 0, 0, 0],
    [3, 3, 1, 1, 0, 0, 0, 0],
    [2, 2, 2, 1, 0, 0, 0, 0],
    [3, 2, 2, 1, 0, 0, 0, 0],
    [2, 2, 2, 2, 0, 0, 0, 0],
];
const HE_MU_MIMO_NSTS_5: [[u8; 8]; 7] = [
    [1, 1, 1, 1, 1, 0, 0, 0],
    [2, 1, 1, 1, 1, 0, 0, 0],
    [3, 1, 1, 1, 1, 0, 0, 0],
    [4, 1, 1, 1, 1, 0, 0, 0],
    [2, 2, 1, 1, 1, 0, 0, 0],
    [3, 2, 1, 1, 1, 0, 0, 0],
    [2, 2, 2, 1, 1, 0, 0, 0],
];
const HE_MU_MIMO_NSTS_6: [[u8; 8]; 4] = [
    [1, 1, 1, 1, 1, 1, 0, 0],
    [2, 1, 1, 1, 1, 1, 0, 0],
    [3, 1, 1, 1, 1, 1, 0, 0],
    [2, 2, 1, 1, 1, 1, 0, 0],
];
const HE_MU_MIMO_NSTS_7: [[u8; 8]; 2] = [[1, 1, 1, 1, 1, 1, 1, 0], [2, 1, 1, 1, 1, 1, 1, 0]];
const HE_MU_MIMO_NSTS_8: [[u8; 8]; 1] = [[1, 1, 1, 1, 1, 1, 1, 1]];

/// A validated HE MU-MIMO spatial-configuration encoding.
///
/// SOURCE(esp32s31)\[BLOB_LIBPP_MUMIMO_SPATIAL_CFG_GET_NSTS]: complete
/// `libpp[test_hal_rx_mu.o]::mumimo_spatial_cfg_get_nsts,
/// libpp[test_hal_rx_mu.o]::mumimo_spatial_cfg_get_nsts_tot`, sizes `0x10e` and `0x44`. The first
/// function selects one of the seven ROM tables above with an eight-byte
/// stride; the second sums exactly `user_count` entries.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct HeMuMimoSpatialConfiguration {
    user_count: u8,
    encoding: u8,
    nsts: &'static [u8; 8],
    total_nsts: u8,
}

impl HeMuMimoSpatialConfiguration {
    pub fn try_new(user_count: u8, encoding: u8) -> Option<Self> {
        let nsts = configuration_record(user_count, encoding)?;
        let total_nsts = nsts
            .iter()
            .take(usize::from(user_count))
            .try_fold(0_u8, |total, value| total.checked_add(*value))?;
        Some(Self {
            user_count,
            encoding,
            nsts,
            total_nsts,
        })
    }

    pub const fn user_count(self) -> u8 {
        self.user_count
    }

    pub const fn encoding(self) -> u8 {
        self.encoding
    }

    #[deny(
        clippy::indexing_slicing,
        clippy::unwrap_used,
        clippy::expect_used,
        clippy::panic,
        clippy::unreachable,
        clippy::arithmetic_side_effects
    )]
    pub fn nsts_for_user(self, user_index: u8) -> Option<u8> {
        if user_index >= self.user_count {
            return None;
        }
        self.nsts.get(usize::from(user_index)).copied()
    }

    #[deny(
        clippy::indexing_slicing,
        clippy::unwrap_used,
        clippy::expect_used,
        clippy::panic,
        clippy::unreachable,
        clippy::arithmetic_side_effects
    )]
    pub fn total_nsts(self) -> u8 {
        self.total_nsts
    }
}

fn configuration_record(user_count: u8, encoding: u8) -> Option<&'static [u8; 8]> {
    let encoding = usize::from(encoding);
    match user_count {
        2 => HE_MU_MIMO_NSTS_2.get(encoding),
        3 => HE_MU_MIMO_NSTS_3.get(encoding),
        4 => HE_MU_MIMO_NSTS_4.get(encoding),
        5 => HE_MU_MIMO_NSTS_5.get(encoding),
        6 => HE_MU_MIMO_NSTS_6.get(encoding),
        7 => HE_MU_MIMO_NSTS_7.get(encoding),
        8 => HE_MU_MIMO_NSTS_8.get(encoding),
        _ => None,
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum HeMuSigBUser {
    NonMimo(HeMuSigBNonMimoUser),
    Mimo(HeMuSigBMimoUser),
}

impl HeMuSigBUser {
    pub const fn decode(word: u32, compressed_sig_b: bool) -> Self {
        if compressed_sig_b {
            Self::Mimo(HeMuSigBMimoUser::decode(word))
        } else {
            Self::NonMimo(HeMuSigBNonMimoUser::decode(word))
        }
    }
}

// SOURCE[ROM_REV0_SIGB_COMMON_RU_ALLOCATION]:
// qualified ESP32-S31 rev0 ROM image,
// symbols `sigb_common_ru_allocation` at 0x2f84ff38 (144 bytes) and
// `sigb_ru_allocation_user_num` at 0x2f84ffc8 (16 bytes). Each row is the
// exact nine-byte ROM stride; zero tail entries are retained.
const HE_MU_SIG_B_COMMON_RU_TONES: [[u8; 9]; 16] = [
    [26, 26, 26, 26, 26, 26, 26, 26, 26],
    [26, 26, 26, 26, 26, 26, 26, 52, 0],
    [26, 26, 26, 26, 26, 52, 26, 26, 0],
    [26, 26, 26, 26, 26, 52, 52, 0, 0],
    [26, 26, 52, 26, 26, 26, 26, 26, 0],
    [26, 26, 52, 26, 26, 26, 52, 0, 0],
    [26, 26, 52, 26, 52, 26, 26, 0, 0],
    [26, 26, 52, 26, 52, 52, 0, 0, 0],
    [52, 26, 26, 26, 26, 26, 26, 26, 26],
    [52, 26, 26, 26, 26, 26, 52, 0, 0],
    [52, 26, 26, 26, 52, 26, 26, 0, 0],
    [52, 26, 26, 26, 52, 52, 0, 0, 0],
    [52, 52, 26, 26, 26, 26, 26, 0, 0],
    [52, 52, 26, 26, 26, 52, 0, 0, 0],
    [52, 52, 26, 52, 26, 26, 0, 0, 0],
    [52, 52, 26, 52, 52, 0, 0, 0, 0],
];
const HE_MU_SIG_B_COMMON_RU_USER_COUNTS: [u8; 16] =
    [9, 8, 8, 7, 8, 7, 7, 6, 8, 7, 7, 6, 7, 6, 6, 5];

/// A failure to decode the complete blob's HE-SIG-B RU Allocation domain.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum He20MuSigBRuAllocationError {
    ReservedEncoding,
    UnsupportedRuType,
}

/// One user's RU view selected by an HE-SIG-B RU Allocation encoding.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct He20MuSigBRuUser {
    pub zero_based_position: u8,
    pub resource_unit: HeResourceUnit,
    /// The exact numeric `multiplexed` output produced by the blob helper.
    ///
    /// It is deliberately not collapsed to `bool`: several allocation
    /// classes return values through seven.
    pub multiplexed: u8,
}

/// A validated HE20 HE-SIG-B RU Allocation encoding.
///
/// SOURCE(esp32s31)\[BLOB_LIBPP_GET_USER_NUM]: complete `libpp.a
/// [test_hal_rx_mu.o]::get_user_num`, size `0x2e2`.
/// Complete caller `libpp[test_hal_rx_mu.o]::test_nonmimo_update_user_info` passes the RU Allocation
/// byte and zero-based user position, then logs the two output bytes as
/// `ru_size` and `multiplexed`. The helper indexes the two exact ROM objects
/// above only for encodings 0 through 15 and computes every other class with
/// bounded branches. Wider RUs cannot be represented by this HE20-specific
/// type; this does not impose a bandwidth limit on the backend.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct He20MuSigBRuAllocation {
    encoding: u8,
    user_count: u8,
}

impl He20MuSigBRuAllocation {
    pub const fn try_new(encoding: u8) -> Result<Self, He20MuSigBRuAllocationError> {
        macro_rules! count {
            ($value:expr) => {
                match $value {
                    Some(value) => value,
                    None => return Err(He20MuSigBRuAllocationError::ReservedEncoding),
                }
            };
        }
        let user_count = match encoding {
            0..=15 => *count!(const_get(
                &HE_MU_SIG_B_COMMON_RU_USER_COUNTS,
                encoding as usize
            )),
            16..=39 => count!(
                SingleGroupRuAllocationCode::from_bytes([encoding])
                    .low3()
                    .checked_add(3)
            ),
            40..=47 | 64..=79 => count!(
                SingleGroupRuAllocationCode::from_bytes([encoding])
                    .low3()
                    .checked_add(6)
            ),
            48..=63 | 80..=95 => count!(
                SingleGroupRuAllocationCode::from_bytes([encoding])
                    .low3()
                    .checked_add(5)
            ),
            96..=111 => count!(
                SingleGroupRuAllocationCode::from_bytes([encoding])
                    .low3()
                    .checked_add(4)
            ),
            112 => 4,
            128..=191 => {
                count!(
                    count!(
                        TwoGroupRuAllocationCode::from_bytes([encoding])
                            .first_group()
                            .checked_add(
                                TwoGroupRuAllocationCode::from_bytes([encoding]).second_group()
                            )
                    )
                    .checked_add(2)
                )
            }
            192..=199 => count!(
                SingleGroupRuAllocationCode::from_bytes([encoding])
                    .low3()
                    .checked_add(1)
            ),
            200..=216 => return Err(He20MuSigBRuAllocationError::UnsupportedRuType),
            _ => return Err(He20MuSigBRuAllocationError::ReservedEncoding),
        };
        Ok(Self {
            encoding,
            user_count,
        })
    }

    pub const fn encoding(self) -> u8 {
        self.encoding
    }

    pub const fn user_count(self) -> u8 {
        self.user_count
    }

    pub const fn user(self, zero_based_position: u8) -> Option<He20MuSigBRuUser> {
        if zero_based_position >= self.user_count {
            return None;
        }

        let resource_unit = match self.encoding {
            0..=15 => {
                let Some(row) = const_get(&HE_MU_SIG_B_COMMON_RU_TONES, self.encoding as usize)
                else {
                    return None;
                };
                let Some(tones) = const_get(row, zero_based_position as usize) else {
                    return None;
                };
                match *tones {
                    26 => HeResourceUnit::Ru26,
                    52 => HeResourceUnit::Ru52,
                    _ => return None,
                }
            }
            16..=23 => {
                if zero_based_position > 1 {
                    HeResourceUnit::Ru106
                } else {
                    HeResourceUnit::Ru52
                }
            }
            24..=39 => {
                if zero_based_position == 0 {
                    HeResourceUnit::Ru106
                } else {
                    HeResourceUnit::Ru52
                }
            }
            40..=47 => {
                if zero_based_position == 5 {
                    HeResourceUnit::Ru106
                } else {
                    HeResourceUnit::Ru26
                }
            }
            48..=55 => match zero_based_position {
                2 => HeResourceUnit::Ru52,
                4 => HeResourceUnit::Ru106,
                _ => HeResourceUnit::Ru26,
            },
            56..=63 => match zero_based_position {
                0 => HeResourceUnit::Ru52,
                4 => HeResourceUnit::Ru106,
                _ => HeResourceUnit::Ru26,
            },
            64..=79 => {
                if zero_based_position == 0 {
                    HeResourceUnit::Ru106
                } else {
                    HeResourceUnit::Ru26
                }
            }
            80..=87 => {
                if zero_based_position == 0 || zero_based_position == 4 {
                    HeResourceUnit::Ru106
                } else {
                    HeResourceUnit::Ru26
                }
            }
            88..=95 => {
                if zero_based_position == 0 || zero_based_position == 2 {
                    HeResourceUnit::Ru106
                } else {
                    HeResourceUnit::Ru26
                }
            }
            96..=103 => match zero_based_position {
                0 | 1 => HeResourceUnit::Ru52,
                3 => HeResourceUnit::Ru106,
                _ => HeResourceUnit::Ru26,
            },
            104..=111 => match zero_based_position {
                0 => HeResourceUnit::Ru106,
                1 => HeResourceUnit::Ru26,
                _ => HeResourceUnit::Ru52,
            },
            112 => HeResourceUnit::Ru52,
            128..=191 => HeResourceUnit::Ru106,
            192..=199 => HeResourceUnit::Ru242,
            _ => return None,
        };

        let multiplexed = match self.encoding {
            0..=15 | 112 => 0,
            16..=111 | 192..=199 => SingleGroupRuAllocationCode::from_bytes([self.encoding]).low3(),
            128..=191 => {
                let Some(first_group_users) = TwoGroupRuAllocationCode::from_bytes([self.encoding])
                    .first_group()
                    .checked_add(1)
                else {
                    return None;
                };
                let users_in_group = if zero_based_position < first_group_users {
                    first_group_users
                } else {
                    let Some(users) = TwoGroupRuAllocationCode::from_bytes([self.encoding])
                        .second_group()
                        .checked_add(1)
                    else {
                        return None;
                    };
                    users
                };
                if users_in_group >= 2 { 1 } else { 0 }
            }
            _ => return None,
        };

        Some(He20MuSigBRuUser {
            zero_based_position,
            resource_unit,
            multiplexed,
        })
    }
}

/// Number of common-information bits before the first user in an HE20
/// non-MU-MIMO complete HE-SIG-B stream.
pub const HE20_MU_SIG_B_COMMON_BITS: u16 = 18;
/// Number of bits in one HE-SIG-B user field.
pub const HE_MU_SIG_B_USER_BITS: u16 = 21;
/// Number of bits occupied by a complete pair of user fields and its
/// intervening CRC/tail.
pub const HE20_MU_SIG_B_USER_PAIR_BITS: u16 = 52;
/// Maximum number of users in the blob's complete non-MU-MIMO HE20 parser.
pub const HE20_MU_SIG_B_MAX_USERS: u8 = 9;
/// Maximum number of users retained by the blob's compressed/MU-MIMO parser.
pub const HE20_MU_SIG_B_MIMO_MAX_USERS: u8 = 4;

#[deny(
    clippy::indexing_slicing,
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::unreachable,
    clippy::arithmetic_side_effects
)]
fn read_complete_sig_b_user(complete_bytes: &[u8], bit_offset: u16) -> Option<u32> {
    oer_wire::Reader::new(complete_bytes)
        .bits::<21>(usize::from(bit_offset))
        .ok()
        .map(|word| word as u32)
}

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
        let raw = read_complete_sig_b_user(self.complete_bytes, bit_offset)?;
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
        let raw = read_complete_sig_b_user(self.complete_bytes, bit_offset)?;
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

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum HeMcsNssSupport {
    Mcs0To7,
    Mcs0To9,
    Mcs0To11,
    NotSupported,
}

impl HeMcsNssSupport {
    /// The support the MCS/NSS `map` advertises for one spatial stream.
    #[deny(
        clippy::indexing_slicing,
        clippy::unwrap_used,
        clippy::expect_used,
        clippy::panic,
        clippy::unreachable,
        clippy::arithmetic_side_effects
    )]
    const fn one_stream_from_map(map: u16) -> Self {
        match HeMcsMap::from_bytes(map.to_le_bytes()).nss1() {
            0 => Self::Mcs0To7,
            1 => Self::Mcs0To9,
            2 => Self::Mcs0To11,
            _ => Self::NotSupported,
        }
    }

    pub const fn supports_mcs9(self) -> bool {
        matches!(self, Self::Mcs0To9 | Self::Mcs0To11)
    }
}

/// Maximum modulation constellation advertised for HE DCM.
///
/// The values are the two-bit HE PHY capability encoding, not an S31-private
/// enum. Keeping `NotSupported` distinct is important: a HE peer is not
/// necessarily able to receive DCM merely because it supports ordinary HE SU.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq, Ord, PartialOrd)]
#[repr(u8)]
pub enum HeDcmConstellation {
    #[default]
    NotSupported = 0,
    Bpsk = 1,
    Qpsk = 2,
    Qam16 = 3,
}

impl HeDcmConstellation {
    #[deny(
        clippy::indexing_slicing,
        clippy::unwrap_used,
        clippy::expect_used,
        clippy::panic,
        clippy::unreachable,
        clippy::arithmetic_side_effects
    )]
    const fn from_encoding(encoding: u8) -> Self {
        match HeMcsMap::from_bytes([encoding, 0]).nss1() {
            0 => Self::NotSupported,
            1 => Self::Bpsk,
            2 => Self::Qpsk,
            _ => Self::Qam16,
        }
    }

    pub const fn supports_bpsk(self) -> bool {
        !matches!(self, Self::NotSupported)
    }

    pub const fn supports_qpsk(self) -> bool {
        matches!(self, Self::Qpsk | Self::Qam16)
    }

    pub const fn supports_16qam(self) -> bool {
        matches!(self, Self::Qam16)
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct He20Capabilities {
    pub receive_mcs_map: u16,
    pub transmit_mcs_map: u16,
    pub receive_nss1: HeMcsNssSupport,
    pub transmit_nss1: HeMcsNssSupport,
    /// The peer can receive the optional HE SU 1x HE-LTF / 0.8-us GI form.
    ///
    /// SOURCE(esp32s31)\[LINUX_IEEE80211_HE_PHY_CAP1_GI_2026_07_29]: Linux v6.12
    /// `include/linux/ieee80211.h` names HE PHY capability byte 1 bit `0x40`
    /// `HE_LTF_AND_GI_FOR_HE_PPDUS_0_8US`. The S31 oracle's ordinary
    /// `libpp[pp_he.o]::ppSelectTxFormat` never emits GI/LTF selector zero,
    /// while hardware tests against a peer with this bit clear rejected
    /// selector zero for MCS0 through MCS9 and accepted selectors one through
    /// three.
    pub one_ltf_800ns_gi: bool,
    /// The peer can decode LDPC coding in an HE payload.
    ///
    /// SOURCE(esp32s31)\[BLOB_LIBNET80211_HE_CAP_LDPC]: complete
    /// `libnet80211[ieee80211_he.o]::ieee80211_parse_hecap`
    /// (size `0x2d8`) reads HE PHY capability byte one at element offset ten,
    /// shifts it by five and masks one before publishing the decoded field in
    /// its complete capability diagnostic. The same function subsequently
    /// copies the complete 24-byte bounded prefix into peer state. This is
    /// independent of the peer's DCM receive constellation.
    pub ldpc_coding_in_payload: bool,
    /// The peer can transmit HE STBC below 80 MHz.
    ///
    /// HE PHY capability byte 2 bit 2; local receive support is a separate
    /// backend capability.
    pub stbc_transmit_under_80_mhz: bool,
    /// The peer can receive HE STBC below 80 MHz.
    ///
    /// SOURCE(esp32s31)\[BLOB_LIBNET80211_HE_CAP_STBC]: complete
    /// `libnet80211[ieee80211_he.o]::ieee80211_add_hecap` copies
    /// `g_phy_cap_rx_stbc` into HE PHY capability byte 2 bit 3. Complete
    /// `esp_wifi_enable_rx_stbc` owns that one-byte capability flag and the
    /// corresponding interface-state bits.
    pub stbc_receive_under_80_mhz: bool,
    /// Maximum DCM constellation the peer can transmit.
    ///
    /// HE PHY capability byte 3 bits 1:0. This is useful for the open RX
    /// policy, but is deliberately separate from [`Self::dcm_receive`].
    pub dcm_transmit: HeDcmConstellation,
    /// Maximum DCM constellation the peer can receive.
    ///
    /// SOURCE(esp32s31)\[LINUX_IEEE80211_HE_PHY_CAP3_DCM_2026_07_29]: Linux
    /// `include/linux/ieee80211-he.h` names HE PHY capability byte 3 bits
    /// 4:3 `DCM_MAX_CONST_RX`. `libpp[trc.o]::rcGetDCMMaxRate`
    /// independently maps the same four capability levels to disabled,
    /// BPSK/MCS0, QPSK/MCS1 and 16-QAM/MCS3 for the vendor BCC path.
    pub dcm_receive: HeDcmConstellation,
    /// The peer can send SU beamforming feedback in a Trigger frame response.
    pub triggered_su_beamforming_feedback: bool,
    /// The peer can send partial-bandwidth MU feedback in a Trigger response.
    pub triggered_mu_beamforming_partial_bandwidth_feedback: bool,
    /// The peer can send CQI feedback in a Trigger frame response.
    ///
    /// HE PHY capability byte 6 bit 4. This is distinct from
    /// [`Self::non_triggered_cqi_feedback`].
    pub triggered_cqi_feedback: bool,
    /// The peer can send CQI feedback without a Trigger frame.
    ///
    /// HE PHY capability byte 9 bit 1.
    pub non_triggered_cqi_feedback: bool,
}

impl He20Capabilities {
    pub const fn supports_bidirectional_mcs9(self) -> bool {
        self.receive_nss1.supports_mcs9() && self.transmit_nss1.supports_mcs9()
    }

    pub const fn supports_one_ltf_800ns_gi(self) -> bool {
        self.one_ltf_800ns_gi
    }

    pub const fn supports_ldpc_coding_in_payload(self) -> bool {
        self.ldpc_coding_in_payload
    }

    pub const fn dcm_receive_constellation(self) -> HeDcmConstellation {
        self.dcm_receive
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct He20Operation {
    pub bss_color: u8,
    pub bss_color_enabled: bool,
    pub partial_bss_color: bool,
    pub basic_mcs_nss_map: u16,
}

impl He20Operation {
    /// Return the BSS color published in an HE-SIG-A1 transmit vector.
    ///
    /// SOURCE(esp32s31): complete `libnet80211[ieee80211_he.o]::ieee80211_parse_heopr` passes HE Operation byte-six bit seven inverted
    /// as the enable argument, bit six as the partial-color argument and bits
    /// 5:0 as the color to `hal_he_set_bss_color`. Complete
    /// `libpp[hal_mac_ctl.o]::hal_he_get_bss_color` returns zero
    /// whenever that enable bit is clear. Keeping the advertised numeric
    /// color separate from this effective transmit value prevents a disabled
    /// BSS color such as `0xae` from becoming active color 46.
    pub const fn effective_bss_color(self) -> u8 {
        if self.bss_color_enabled {
            self.bss_color
        } else {
            0
        }
    }
}

/// Bounded HE20 peer state consumed by a chip-specific register backend.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct He20PeerState {
    pub capability_prefix: [u8; HE_CAPABILITIES_IE_MIN_LEN],
    pub packet_padding_eight_us: u8,
    pub default_packet_extension_duration: u8,
    pub bss_color: u8,
    pub bss_color_enabled: bool,
    pub partial_bss_color: bool,
    pub basic_mcs_nss_map: u16,
    pub rts_threshold: Option<u16>,
    /// Raw HE Operation `ER-SU-Disable` bit.
    ///
    /// SOURCE(esp32s31): complete
    /// `libnet80211[ieee80211_he.o]::ieee80211_parse_heopr`
    /// logs complete-IE byte five bit zero as `ER-SU-Disable`, stores it at
    /// peer-state bit 10 and passes it unchanged to `libpp[hal_mac_ctl.o]::hal_he_set_ersu`.
    pub extended_range_single_user_disabled: bool,
}

impl He20PeerState {
    pub const fn extended_range_single_user_permitted(self) -> bool {
        !self.extended_range_single_user_disabled
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum HeElementError {
    WrongElement,
    LengthMismatch,
    TooShort,
    WrongExtension,
}

/// The extension element `element`, split into its fixed `MINIMUM_LEN`-byte
/// prefix (header included) and the optional bytes after it.
#[deny(
    clippy::indexing_slicing,
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::unreachable,
    clippy::arithmetic_side_effects
)]
fn validate_extension<const MINIMUM_LEN: usize>(
    element: &[u8],
    extension: u8,
) -> Result<(&[u8; MINIMUM_LEN], &[u8]), HeElementError> {
    if element.first().copied() != Some(255) {
        return Err(HeElementError::WrongElement);
    }
    let Some(declared_len) = element.get(1).copied() else {
        return Err(HeElementError::TooShort);
    };
    if usize::from(declared_len).checked_add(2) != Some(element.len()) {
        return Err(HeElementError::LengthMismatch);
    }
    let mut reader = oer_wire::Reader::new(element);
    let prefix = reader
        .array::<MINIMUM_LEN>()
        .map_err(|_| HeElementError::TooShort)?;
    let tail = reader.remaining();
    if prefix.get(2).copied() != Some(extension) {
        return Err(HeElementError::WrongExtension);
    }
    Ok((prefix, tail))
}

#[deny(
    clippy::indexing_slicing,
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::unreachable,
    clippy::arithmetic_side_effects
)]
pub fn parse_he20_capabilities(element: &[u8]) -> Result<He20Capabilities, HeElementError> {
    let (element, _) =
        validate_extension::<HE_CAPABILITIES_IE_MIN_LEN>(element, HE_CAPABILITIES_EXTENSION_ID)?;
    let mut reader = oer_wire::Reader::new(element);
    reader.take(9).map_err(|_| HeElementError::TooShort)?;
    let phy = HePhyCapabilities::from_bytes(reader.read().map_err(|_| HeElementError::TooShort)?);
    let receive_mcs_map = reader.u16_le().map_err(|_| HeElementError::TooShort)?;
    let transmit_mcs_map = reader.u16_le().map_err(|_| HeElementError::TooShort)?;
    Ok(He20Capabilities {
        receive_mcs_map,
        transmit_mcs_map,
        receive_nss1: HeMcsNssSupport::one_stream_from_map(receive_mcs_map),
        transmit_nss1: HeMcsNssSupport::one_stream_from_map(transmit_mcs_map),
        one_ltf_800ns_gi: phy.one_ltf_800ns_gi(),
        ldpc_coding_in_payload: phy.ldpc_coding_in_payload(),
        stbc_transmit_under_80_mhz: phy.stbc_transmit_under_80_mhz(),
        stbc_receive_under_80_mhz: phy.stbc_receive_under_80_mhz(),
        dcm_transmit: HeDcmConstellation::from_encoding(phy.dcm_transmit()),
        dcm_receive: HeDcmConstellation::from_encoding(phy.dcm_receive()),
        triggered_su_beamforming_feedback: phy.triggered_su_beamforming_feedback(),
        triggered_mu_beamforming_partial_bandwidth_feedback: phy
            .triggered_mu_beamforming_partial_bandwidth_feedback(),
        triggered_cqi_feedback: phy.triggered_cqi_feedback(),
        non_triggered_cqi_feedback: phy.non_triggered_cqi_feedback(),
    })
}

#[deny(
    clippy::indexing_slicing,
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::unreachable,
    clippy::arithmetic_side_effects
)]
pub fn parse_he20_operation(element: &[u8]) -> Result<He20Operation, HeElementError> {
    let (element, _) =
        validate_extension::<HE_OPERATION_IE_MIN_LEN>(element, HE_OPERATION_EXTENSION_ID)?;
    let mut reader = oer_wire::Reader::new(element);
    reader.take(3).map_err(|_| HeElementError::TooShort)?;
    let fields =
        HeOperationFields::from_bytes(reader.read().map_err(|_| HeElementError::TooShort)?);
    Ok(He20Operation {
        bss_color: fields.bss_color(),
        bss_color_enabled: !fields.bss_color_disabled(),
        partial_bss_color: fields.partial_bss_color(),
        basic_mcs_nss_map: fields.basic_mcs_nss(),
    })
}

/// Recover the peer fields installed by the pinned HE capability and
/// operation parsers.
///
/// Primary evidence: complete pinned
/// `libnet80211[ieee80211_he.o]::ieee80211_parse_hecap,
/// libnet80211[ieee80211_he.o]::ieee80211_parse_heopr` bodies and their format strings. This function is
/// deliberately pure; corresponding MMIO transforms are tracked separately
/// by the hardware backend.
#[deny(
    clippy::indexing_slicing,
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::unreachable,
    clippy::arithmetic_side_effects
)]
pub fn parse_he20_peer_state(
    capability: &[u8],
    operation: &[u8],
) -> Result<He20PeerState, HeElementError> {
    let (capability, thresholds) =
        validate_extension::<HE_CAPABILITIES_IE_MIN_LEN>(capability, HE_CAPABILITIES_EXTENSION_ID)?;
    let (operation, _) =
        validate_extension::<HE_OPERATION_IE_MIN_LEN>(operation, HE_OPERATION_EXTENSION_ID)?;

    let mut reader = oer_wire::Reader::new(capability);
    reader.take(9).map_err(|_| HeElementError::TooShort)?;
    let phy = HePhyCapabilities::from_bytes(reader.read().map_err(|_| HeElementError::TooShort)?);
    let mut reader = oer_wire::Reader::new(operation);
    reader.take(3).map_err(|_| HeElementError::TooShort)?;
    let fields =
        HeOperationFields::from_bytes(reader.read().map_err(|_| HeElementError::TooShort)?);
    let capability_prefix = *capability;
    let packet_padding_eight_us = if !phy.ppe_present() {
        phy.nominal_padding()
    } else {
        // The PPE Thresholds field follows the fixed prefix.
        let [ppe0, ppe1, ..] = *thresholds else {
            return Err(HeElementError::TooShort);
        };
        let ppe = PpeFields::from_bytes([ppe0, ppe1]);
        let ppet8 = ppe.ppet8();
        if ppe.ru0() && ppe.ppet16() == 7 && ppet8 == 0 {
            2
        } else {
            0
        }
    };

    let encoded_rts_threshold = fields.rts_threshold();
    let rts_threshold =
        (!matches!(encoded_rts_threshold, 0 | 0x03ff)).then_some(encoded_rts_threshold);

    Ok(He20PeerState {
        capability_prefix,
        packet_padding_eight_us,
        default_packet_extension_duration: fields.default_pe(),
        bss_color: fields.bss_color(),
        bss_color_enabled: !fields.bss_color_disabled(),
        partial_bss_color: fields.partial_bss_color(),
        basic_mcs_nss_map: fields.basic_mcs_nss(),
        rts_threshold,
        extended_range_single_user_disabled: fields.er_su_disabled(),
    })
}

oer_wire::bitfield! {
    /// SOURCE: IEEE Std 802.11ax-2021, 9.4.2.248.3 (HE PHY Capabilities Information field).
    pub struct HePhyCapabilities[11] (little) {
        pub one_ltf_800ns_gi, with_one_ltf_800ns_gi: bool = 14..15;
        pub ldpc_coding_in_payload, with_ldpc_coding_in_payload: bool = 13..14;
        pub stbc_transmit_under_80_mhz, with_stbc_transmit_under_80_mhz: bool = 18..19;
        pub stbc_receive_under_80_mhz, with_stbc_receive_under_80_mhz: bool = 19..20;
        pub dcm_transmit, with_dcm_transmit: u8 = 24..26;
        pub dcm_receive, with_dcm_receive: u8 = 27..29;
        pub triggered_su_beamforming_feedback, with_triggered_su_beamforming_feedback: bool = 50..51;
        pub triggered_mu_beamforming_partial_bandwidth_feedback, with_triggered_mu_beamforming_partial_bandwidth_feedback: bool = 51..52;
        pub triggered_cqi_feedback, with_triggered_cqi_feedback: bool = 52..53;
        pub ppe_present, with_ppe_present: bool = 55..56;
        pub non_triggered_cqi_feedback, with_non_triggered_cqi_feedback: bool = 73..74;
        pub nominal_padding, with_nominal_padding: u8 = 78..80;
    }
}
oer_wire::bitfield! {
    /// SOURCE: IEEE Std 802.11ax-2021, 9.4.2.249 (HE Operation element).
    pub struct HeOperationFields[6] (little) {
        pub default_pe, with_default_pe: u8 = 0..3;
        pub rts_threshold, with_rts_threshold: u16 = 4..14;
        pub er_su_disabled, with_er_su_disabled: bool = 16..17;
        pub bss_color, with_bss_color: u8 = 24..30;
        pub partial_bss_color, with_partial_bss_color: bool = 30..31;
        pub bss_color_disabled, with_bss_color_disabled: bool = 31..32;
        pub basic_mcs_nss, with_basic_mcs_nss: u16 = 32..48;
    }
}
oer_wire::bitfield! {
    /// Non-MU-MIMO HE-SIG-B User field.
    /// SOURCE: IEEE Std 802.11ax-2021, 27.3.11.8 (HE-SIG-B field).
    pub struct SigBNonMimoUserFields[4] (little) {
        pub station_id, with_station_id: u16 = 0..11;
        pub nsts, with_nsts: u8 = 11..14;
        pub beamformed, with_beamformed: bool = 14..15;
        pub mcs, with_mcs: u8 = 15..19;
        pub dcm, with_dcm: bool = 19..20;
        pub ldpc, with_ldpc: bool = 20..21;
    }
}
oer_wire::bitfield! {
    /// MU-MIMO HE-SIG-B User field; bit 19 is reserved.
    /// SOURCE: IEEE Std 802.11ax-2021, 27.3.11.8 (HE-SIG-B field).
    pub struct SigBMimoUserFields[4] (little) {
        pub station_id, with_station_id: u16 = 0..11;
        pub spatial_configuration, with_spatial_configuration: u8 = 11..15;
        pub mcs, with_mcs: u8 = 15..19;
        pub ldpc, with_ldpc: bool = 20..21;
    }
}
oer_wire::bitfield! {
    /// SOURCE: IEEE Std 802.11ax-2021, 9.4.2.248.4 (Supported HE-MCS And NSS Set field).
    pub struct HeMcsMap[2] (little) {
        pub nss1, with_nss1: u8 = 0..2;
    }
}
oer_wire::bitfield! {
    /// SOURCE: IEEE Std 802.11ax-2021, 9.4.2.248.5 (PPE Thresholds field).
    pub struct PpeFields[2] (little) {
        pub ru0, with_ru0: bool = 3..4;
        pub ppet8, with_ppet8: u8 = 7..10;
        pub ppet16, with_ppet16: u8 = 10..13;
    }
}

oer_wire::bitfield! {
    /// RU Allocation encoding with one MU-MIMO group count.
    /// SOURCE: IEEE Std 802.11ax-2021, 27.3.11.8 (HE-SIG-B RU Allocation subfield).
    pub struct SingleGroupRuAllocationCode[1] (little) {
        pub low3, with_low3: u8 = 0..3;
    }
}
oer_wire::bitfield! {
    /// RU Allocation encoding with two MU-MIMO group counts.
    /// SOURCE: IEEE Std 802.11ax-2021, 27.3.11.8 (HE-SIG-B RU Allocation subfield).
    pub struct TwoGroupRuAllocationCode[1] (little) {
        pub first_group, with_first_group: u8 = 2..4;
        pub second_group, with_second_group: u8 = 0..2;
    }
}

#[cfg(test)]
mod tests;

// `slice::get` is not const-stable in the pinned toolchain. Checked splitting
// retains the same refused-index semantics without indexing or a panic.
const fn const_get<T>(values: &[T], index: usize) -> Option<&T> {
    match values.split_at_checked(index) {
        Some((_, tail)) => match tail.split_first() {
            Some((value, _)) => Some(value),
            None => None,
        },
        None => None,
    }
}
