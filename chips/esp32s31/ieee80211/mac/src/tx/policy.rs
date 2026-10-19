//! Stateless ordinary STA/AP portion of `ieee80211_set_tx_desc`.
//!
//! Names containing an offset are intentional. The pinned blob exposes the
//! effect of those node bytes on the TX descriptor, but not a reliable
//! semantic name. Recording the observed transformation is safer than
//! inventing one while the fixed C node layout is still being migrated.

use oer_ieee80211_mac::data::DataInterfaceRole;

use crate::tx::metadata::descriptor_priority_byte;

const DESCRIPTOR_INTERFACE_SHIFT: u32 = 18;
const DESCRIPTOR_NODE_NIBBLE_SHIFT: u32 = 12;
const DESCRIPTOR_SECURITY_REPLACED_MASK: u32 = 0x000c_f000;
const DESCRIPTOR_STA_STATE_BIT: u32 = 1 << 16;
const HE_DESCRIPTOR_FLAG: u32 = 1 << 31;
const OBSERVED_REQUEST_FLAGS: u32 = 0x08 | 0x10;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct DescriptorPolicyInput {
    pub role: DataInterfaceRole,
    pub priority: u8,
    pub existing_flags: u32,
    pub requested_flags: u32,
    pub existing_security: u32,
    pub existing_he_control: u16,
    pub node_descriptor_nibble_0x2b8: u8,
    pub node_rate_context_index_0x26: u8,
    pub interface_state_0x154: u8,
    pub node_policy_word_0x348: u32,
    pub adopted_config_byte_0x44a: u8,
    pub node_word_0x35c: u16,
    pub node_byte_0x38c: u8,
    pub node_byte_0x45a: u8,
    pub node_byte_0x4f3: u8,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct DescriptorPolicy {
    pub flags: u32,
    pub priority_byte: u8,
    pub security: u32,
    pub he_control: u16,
    pub byte_0x2f_bit_2: bool,
    pub byte_0x38: u8,
    pub rate_context_selector: u8,
    pub rate_context_index: u8,
}

/// Reproduce the non-HE branch used by strict WPA2 STA/AP data and management
/// frames. A descriptor already marked as HE is rejected: the pinned branch
/// calls two additional HE helpers and has not yet been qualified for TX.
pub const fn ordinary_descriptor_policy(input: DescriptorPolicyInput) -> Option<DescriptorPolicy> {
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
    let priority_byte = match descriptor_priority_byte(input.priority) {
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

#[cfg(test)]
mod tests;
