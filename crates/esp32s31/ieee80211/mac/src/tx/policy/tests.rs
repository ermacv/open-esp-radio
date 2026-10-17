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
