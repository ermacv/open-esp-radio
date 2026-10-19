use super::*;

const BASE: DescriptorPolicyInput = DescriptorPolicyInput {
    role: DataInterfaceRole::Station,
    priority: 5,
    existing_flags: 0x0200_2001,
    requested_flags: 8,
    existing_security: 0xffff_ffff,
    existing_he_control: 0xffff,
    node_descriptor_nibble_0x2b8: 0x1a,
    node_rate_context_index_0x26: 9,
    interface_state_0x154: 4,
    node_policy_word_0x348: 0x18,
    adopted_config_byte_0x44a: 0x03,
    node_word_0x35c: 0,
    node_byte_0x38c: 0,
    node_byte_0x45a: 1,
    node_byte_0x4f3: 1,
};

#[test]
fn station_policy_reproduces_masks_role_and_default_rate_context() {
    let policy = ordinary_descriptor_policy(BASE).unwrap();
    assert_eq!(policy.flags, 0x0200_2009);
    assert_eq!(policy.priority_byte, 0x15);
    assert_eq!(policy.security, 0xfff3_afff);
    assert_eq!(policy.he_control, 0x6007);
    assert!(policy.byte_0x2f_bit_2);
    assert_eq!(policy.byte_0x38, 1);
    assert_eq!(policy.rate_context_selector, 0);
    assert_eq!(policy.rate_context_index, 0);
}

#[test]
fn ap_policy_uses_peer_rate_context_and_clears_station_only_fields() {
    let policy = ordinary_descriptor_policy(DescriptorPolicyInput {
        role: DataInterfaceRole::AccessPoint,
        interface_state_0x154: 0,
        node_word_0x35c: 0x0400,
        ..BASE
    })
    .unwrap();
    assert_eq!(policy.security, 0xfff7_afff);
    assert!(!policy.byte_0x2f_bit_2);
    assert_eq!(policy.byte_0x38, 0);
    assert_eq!(policy.rate_context_selector, 1);
    assert_eq!(policy.rate_context_index, 9);
}

#[test]
fn descriptor_policy_rejects_he_unknown_flags_and_invalid_priority() {
    assert_eq!(
        ordinary_descriptor_policy(DescriptorPolicyInput {
            existing_flags: 1 << 31,
            ..BASE
        }),
        None
    );
    assert_eq!(
        ordinary_descriptor_policy(DescriptorPolicyInput {
            requested_flags: 0x20,
            ..BASE
        }),
        None
    );
    assert_eq!(
        ordinary_descriptor_policy(DescriptorPolicyInput {
            priority: 8,
            ..BASE
        }),
        None
    );
}

#[test]
fn ap_cold_start_request_flag_is_a_bounded_descriptor_or() {
    let policy = ordinary_descriptor_policy(DescriptorPolicyInput {
        role: DataInterfaceRole::AccessPoint,
        requested_flags: 0x10,
        ..BASE
    })
    .unwrap();
    assert_eq!(policy.flags, BASE.existing_flags | 0x10);
}

#[test]
fn opaque_node_bits_are_kept_as_exact_bounded_transforms() {
    let policy = ordinary_descriptor_policy(DescriptorPolicyInput {
        existing_he_control: 0x9ffb,
        node_policy_word_0x348: 0x08,
        adopted_config_byte_0x44a: 0x02,
        node_byte_0x38c: 1,
        node_byte_0x45a: 0,
        node_byte_0x4f3: 0xff,
        ..BASE
    })
    .unwrap();
    assert_eq!(policy.he_control, 0x0000);
    assert_eq!(policy.byte_0x38, 1);
    assert!(policy.byte_0x2f_bit_2);
}

// Independent pre-G2 packing reference.
fn old_descriptor_policy(input: DescriptorPolicyInput) -> Option<DescriptorPolicy> {
    const DESCRIPTOR_INTERFACE_SHIFT: u32 = 18;
    const DESCRIPTOR_NODE_NIBBLE_SHIFT: u32 = 12;
    const DESCRIPTOR_SECURITY_REPLACED_MASK: u32 = 0x000c_f000;
    const DESCRIPTOR_STA_STATE_BIT: u32 = 1 << 16;
    const HE_DESCRIPTOR_FLAG: u32 = 1 << 31;
    const OBSERVED_REQUEST_FLAGS: u32 = 0x08 | 0x10;
    // The pinned vendor leaf only branches on bit 3 and otherwise ORs the
    // request into the descriptor. Strict STA data observes 0x08; strict AP
    // cold-start management TX additionally observes 0x10. Keep all other
    // request bits fail-closed until they are seen and qualified.
    if input.requested_flags & !OBSERVED_REQUEST_FLAGS != 0 {
        return None;
    }
    let flags = input.existing_flags | input.requested_flags;
    if flags & HE_DESCRIPTOR_FLAG != 0 {
        return None;
    }
    let priority_byte = match crate::tx::metadata::queue_class(input.priority)
        .map(|class| (class << 4) | input.priority)
    {
        Some(value) => value,
        None => return None,
    };
    let (selector, index) = match input.role {
        DataInterfaceRole::Station => (0, 0),
        DataInterfaceRole::AccessPoint => (1, input.node_rate_context_index_0x26),
    };

    let mut security = input.existing_security & !DESCRIPTOR_SECURITY_REPLACED_MASK;
    security |=
        ((input.node_descriptor_nibble_0x2b8 & 0x0f) as u32) << DESCRIPTOR_NODE_NIBBLE_SHIFT;
    security |= (selector as u32) << DESCRIPTOR_INTERFACE_SHIFT;
    if matches!(input.role, DataInterfaceRole::Station) {
        if input.interface_state_0x154 == 4 {
            security |= DESCRIPTOR_STA_STATE_BIT;
        } else {
            security &= !DESCRIPTOR_STA_STATE_BIT;
        }
    }

    // The stock non-HE prefix first retains only descriptor byte 0x30 bits
    // 0..2 and byte 0x31 bits 5..7. Its later halfword mask drops bits 0, 1
    // and 15, then merges two configuration bits. Combined, that is 0x6004.
    let adopted_policy =
        input.adopted_config_byte_0x44a & ((input.node_policy_word_0x348 >> 3) as u8 & 0x03);
    let he_control = (input.existing_he_control & 0x6004) | adopted_policy as u16;

    let byte_0x38 = match input.role {
        DataInterfaceRole::Station if input.node_byte_0x38c != 0 => 1,
        DataInterfaceRole::Station => input.node_byte_0x45a & 1,
        DataInterfaceRole::AccessPoint => 0,
    };
    let byte_0x2f_bit_2 = input.node_word_0x35c & 0x0400 == 0 && input.node_byte_0x4f3 & 1 != 0;

    Some(DescriptorPolicy {
        flags,
        priority_byte,
        security,
        he_control,
        byte_0x2f_bit_2,
        byte_0x38,
        rate_context_selector: selector,
        rate_context_index: index,
    })
}

#[test]
fn modeled_ordinary_descriptor_words_match_the_old_policy() {
    use oer_esp32s31_hal::layouts::{
        WifiTxDescriptorPrefix, WifiTxDescriptorPrefixFlags, WifiTxDescriptorPrefixMut,
        WifiTxDescriptorPrefixPriority, WifiTxDescriptorPrefixSecurity,
    };
    for role in [DataInterfaceRole::Station, DataInterfaceRole::AccessPoint] {
        for priority in 0..=9 {
            for requested_flags in [0, 8, 0x10, 0x18, 0x20, 1 << 31] {
                for raw in [0, 1, 0x1234_5678, 0x7fff_ffff, u32::MAX] {
                    let input = DescriptorPolicyInput {
                        role,
                        priority,
                        requested_flags,
                        existing_flags: raw,
                        existing_security: raw,
                        existing_he_control: raw as u16,
                        node_descriptor_nibble_0x2b8: raw as u8,
                        interface_state_0x154: if raw & 1 == 0 { 4 } else { 0 },
                        node_policy_word_0x348: raw,
                        adopted_config_byte_0x44a: raw as u8,
                        node_word_0x35c: raw as u16,
                        node_byte_0x38c: raw as u8,
                        node_byte_0x45a: raw as u8,
                        node_byte_0x4f3: raw as u8,
                        ..BASE
                    };
                    let expected = old_descriptor_policy(input);
                    assert_eq!(ordinary_descriptor_policy(input), expected);
                    if let Some(policy) = expected {
                        let mut bytes = [0xa5; WifiTxDescriptorPrefix::SIZE];
                        let mut writer = WifiTxDescriptorPrefixMut::new(&mut bytes);
                        writer.set_flags(WifiTxDescriptorPrefixFlags::from_bits(policy.flags));
                        writer.set_priority(WifiTxDescriptorPrefixPriority::from_bits(
                            policy.priority_byte,
                        ));
                        writer.set_security(WifiTxDescriptorPrefixSecurity::from_bits(
                            policy.security,
                        ));
                        // Compare the published bytes with the old packing's output.
                        assert_eq!(&bytes[0..4], &policy.flags.to_le_bytes());
                        assert_eq!(bytes[4], policy.priority_byte);
                        assert_eq!(&bytes[16..20], &policy.security.to_le_bytes());
                        assert!(bytes[5..16].iter().all(|byte| *byte == 0xa5));
                    }
                }
            }
        }
    }
}
