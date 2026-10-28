//! SAE Authentication frame body fields and the extension elements an SAE
//! exchange carries.
//!
//! Received bytes reach this module; nothing here panics on them.

/// Element ID that introduces an Element ID Extension.
///
/// SOURCE: IEEE Std 802.11-2020, 9.4.2.1, Table 9-92 (Element IDs).
pub const ELEMENT_ID_EXTENSION: u8 = 255;
/// Element ID Extension of the Password Identifier element.
///
/// SOURCE: IEEE Std 802.11-2020, 9.4.2.1, Table 9-92 (Element IDs).
pub const PASSWORD_IDENTIFIER_EXTENSION_ID: u8 = 33;
/// Element ID Extension of the Rejected Groups element.
///
/// SOURCE: IEEE Std 802.11-2020, 9.4.2.1, Table 9-92 (Element IDs).
pub const REJECTED_GROUPS_EXTENSION_ID: u8 = 92;
/// Element ID Extension of the Anti-Clogging Token Container element.
///
/// SOURCE: IEEE Std 802.11-2020, 9.4.2.1, Table 9-92 (Element IDs).
pub const ANTI_CLOGGING_TOKEN_CONTAINER_EXTENSION_ID: u8 = 93;

/// Octets of the Finite Cyclic Group field that starts an SAE Commit body
/// and of the Send-Confirm field that starts an SAE Confirm body.
///
/// SOURCE: IEEE Std 802.11-2020, 9.3.3.12, Table 9-41 (Authentication
/// frame body: Finite Cyclic Group, Send-Confirm).
pub const SAE_FIELD_LEN: usize = 2;

/// The Finite Cyclic Group field of an SAE Commit body; `None` for a body
/// shorter than the field.
pub fn finite_cyclic_group(body: &[u8]) -> Option<u16> {
    body.first_chunk::<SAE_FIELD_LEN>()
        .copied()
        .map(u16::from_le_bytes)
}

/// The Send-Confirm field of an SAE Confirm body; `None` for a body shorter
/// than the field.
pub fn send_confirm(body: &[u8]) -> Option<u16> {
    body.first_chunk::<SAE_FIELD_LEN>()
        .copied()
        .map(u16::from_le_bytes)
}

/// The three header octets of an Anti-Clogging Token Container carrying a
/// `token_len`-octet token; `None` when the element cannot carry it.
///
/// SOURCE: IEEE Std 802.11-2020, 9.4.2.235 (Anti-Clogging Token Container
/// element: Element ID, Length, Element ID Extension, Anti-Clogging Token).
pub const fn anti_clogging_token_container_header(token_len: usize) -> Option<[u8; 3]> {
    let Some(length) = token_len.checked_add(1) else {
        return None;
    };
    if length > u8::MAX as usize {
        return None;
    }
    Some([
        ELEMENT_ID_EXTENSION,
        length as u8,
        ANTI_CLOGGING_TOKEN_CONTAINER_EXTENSION_ID,
    ])
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn leading_fields_read_little_endian_and_refuse_short_bodies() {
        assert_eq!(finite_cyclic_group(&[19, 0, 0xaa]), Some(19));
        assert_eq!(send_confirm(&[1, 2]), Some(0x0201));
        assert_eq!(finite_cyclic_group(&[19]), None);
        assert_eq!(send_confirm(&[]), None);
    }

    #[test]
    fn a_token_container_header_names_its_length_and_refuses_overflow() {
        assert_eq!(
            anti_clogging_token_container_header(32),
            Some([255, 33, 93])
        );
        assert_eq!(anti_clogging_token_container_header(255), None);
    }
}
