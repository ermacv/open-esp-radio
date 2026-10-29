use super::*;

fn write_bits(bytes: &mut [u8], bit_offset: u16, bit_width: u16, value: u32) {
    for input_bit in 0..bit_width {
        let destination_bit = bit_offset + input_bit;
        let destination = &mut bytes[usize::from(destination_bit / 8)];
        let mask = 1 << (destination_bit % 8);
        if value & (1 << input_bit) != 0 {
            *destination |= mask;
        } else {
            *destination &= !mask;
        }
    }
}

#[test]
fn iterates_three_he20_non_mimo_users_across_pair_crc_tail_gap() {
    let words = [
        (1 << 20) | (3 << 15) | 0x123,
        (1 << 19) | (5 << 15) | 0x456,
        (1 << 14) | (7 << 15) | 0x321,
    ];
    let mut complete = [0_u8; 13];
    write_bits(&mut complete, 18, 21, words[0]);
    write_bits(&mut complete, 39, 21, words[1]);
    write_bits(&mut complete, 70, 21, words[2]);

    // 18 common + 52 for the first pair + 21 for the final user + ten
    // final CRC/tail bits. The blob user-count expression sees three.
    let mut users = He20MuSigBNonMimoUsers::try_new(&complete, 101).unwrap();
    assert_eq!(users.user_count(), 3);
    assert_eq!(users.len(), 3);
    for (expected_index, expected_offset, expected_word) in
        [(0, 18, words[0]), (1, 39, words[1]), (2, 70, words[2])]
    {
        let entry = users.next().unwrap();
        assert_eq!(entry.index, expected_index);
        assert_eq!(entry.bit_offset, expected_offset);
        assert_eq!(entry.raw, expected_word);
        assert_eq!(entry.user, HeMuSigBNonMimoUser::decode(expected_word));
    }
    assert_eq!(users.next(), None);
    assert_eq!(users.len(), 0);
}

#[test]
fn rejects_truncated_or_out_of_domain_he20_complete_streams() {
    assert_eq!(
        He20MuSigBNonMimoUsers::try_new(&[], 17),
        Err(He20MuSigBNonMimoStreamError::BitLengthBeforeFirstUser)
    );
    assert_eq!(
        He20MuSigBNonMimoUsers::try_new(&[0; 11], 101),
        Err(He20MuSigBNonMimoStreamError::CompleteBytesTooShort)
    );
    assert_eq!(
        He20MuSigBNonMimoUsers::try_new(&[0; 12], 90),
        Err(He20MuSigBNonMimoStreamError::IncompleteUserField)
    );
    assert_eq!(
        He20MuSigBNonMimoUsers::try_new(&[0; 35], 278),
        Err(He20MuSigBNonMimoStreamError::TooManyUsers)
    );
}

#[test]
fn binds_he20_common_ru_allocation_to_the_complete_user_count() {
    let mut four_users = [0_u8; 16];
    four_users[0] = 112;
    let users = He20MuSigBNonMimoUsers::try_new(&four_users, 122).unwrap();
    let allocation = users.ru_allocation().unwrap();
    assert_eq!(allocation.encoding(), 112);
    assert_eq!(allocation.user_count(), 4);

    four_users[0] = 0;
    assert_eq!(
        He20MuSigBNonMimoUsers::try_new(&four_users, 122)
            .unwrap()
            .ru_allocation(),
        Err(He20MuSigBRuStreamError::UserCountMismatch {
            stream_users: 4,
            allocation_users: 9,
        })
    );
    four_users[0] = 200;
    assert_eq!(
        He20MuSigBNonMimoUsers::try_new(&four_users, 122)
            .unwrap()
            .ru_allocation(),
        Err(He20MuSigBRuStreamError::Allocation(
            He20MuSigBRuAllocationError::UnsupportedRuType
        ))
    );
}

#[test]
fn iterates_the_four_non_linear_compressed_mimo_user_offsets() {
    let words = [
        (1 << 20) | (1 << 15) | (4 << 11) | 0x111,
        (2 << 15) | (4 << 11) | 0x222,
        (1 << 20) | (3 << 15) | (4 << 11) | 0x333,
        (4 << 15) | (4 << 11) | 0x444,
    ];
    let mut complete = [0_u8; 17];
    for (offset, word) in [0, 21, 52, 105].into_iter().zip(words) {
        write_bits(&mut complete, offset, 21, word);
    }

    let mut users = He20MuSigBMimoUsers::try_new(&complete, 136, 4).unwrap();
    let spatial = users.spatial_configuration().unwrap();
    assert_eq!(spatial.user_count(), 4);
    assert_eq!(spatial.encoding(), 4);
    assert_eq!(
        [0, 1, 2, 3].map(|index| spatial.nsts_for_user(index).unwrap()),
        [2, 2, 1, 1]
    );
    assert_eq!(spatial.total_nsts(), 6);
    assert_eq!(users.len(), 4);
    for (expected_offset, expected_word) in [0, 21, 52, 105].into_iter().zip(words) {
        let entry = users.next().unwrap();
        assert_eq!(entry.bit_offset, expected_offset);
        assert_eq!(entry.raw, expected_word);
        assert_eq!(entry.user, HeMuSigBMimoUser::decode(expected_word));
    }
    assert_eq!(users.next(), None);

    write_bits(&mut complete, 21, 21, words[1] ^ (1 << 11));
    assert_eq!(
        He20MuSigBMimoUsers::try_new(&complete, 136, 4)
            .unwrap()
            .spatial_configuration(),
        Err(He20MuSigBMimoSpatialError::InconsistentEncoding)
    );
}

#[test]
fn rejects_invalid_compressed_mimo_counts_lengths_and_storage() {
    assert_eq!(
        He20MuSigBMimoUsers::try_new(&[0; 17], 136, 0),
        Err(He20MuSigBMimoStreamError::UserCountOutOfRange)
    );
    assert_eq!(
        He20MuSigBMimoUsers::try_new(&[0; 17], 136, 5),
        Err(He20MuSigBMimoStreamError::UserCountOutOfRange)
    );
    assert_eq!(
        He20MuSigBMimoUsers::try_new(&[0; 16], 136, 4),
        Err(He20MuSigBMimoStreamError::CompleteBytesTooShort)
    );
    assert_eq!(
        He20MuSigBMimoUsers::try_new(&[0; 17], 135, 4),
        Err(He20MuSigBMimoStreamError::IncompleteUserField)
    );
    assert_eq!(
        He20MuSigBMimoUsers::try_new(&[0; 9], 72, 3),
        Err(He20MuSigBMimoStreamError::IncompleteUserField)
    );
    assert_eq!(HeMuMimoSpatialConfiguration::try_new(1, 0), None);
    assert_eq!(HeMuMimoSpatialConfiguration::try_new(4, 11), None);
    assert_eq!(
        HeMuMimoSpatialConfiguration::try_new(8, 0)
            .unwrap()
            .total_nsts(),
        8
    );
}
