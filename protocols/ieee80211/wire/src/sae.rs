//! SAE Authentication frame bodies: the Commit and Confirm fields as byte
//! values and the extension elements an SAE exchange carries. What a scalar,
//! an element or a confirm value means is the cryptography of
//! `oer-ieee80211-core-security-rsn`.
//!
//! SOURCE: IEEE Std 802.11-2020, 9.3.3.12, Table 9-41 (Authentication frame
//! body), 12.4.7.4 (Encoding and decoding of SAE Commit messages) and 12.4.7.5
//! (Encoding and decoding of SAE Confirm messages); ESP-IDF
//! `7b9cc1ac79f865983f59bb8ff3ff43eb74ff1dbe`
//! `components/wpa_supplicant/src/common/sae.c` (`sae_write_commit`,
//! `sae_parse_commit`, `sae_write_confirm`).
//!
//! Received bytes reach this module; nothing here panics on them.

use oer_wire::{Reader, Writer};

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

/// The only finite cyclic group this stack offers and accepts: the 256-bit
/// random ECP group (NIST P-256).
///
/// SOURCE: IEEE Std 802.11-2020, 12.4.4.1 (the group numbers of the IANA
/// "Group Description" registry); RFC 5903, group 19.
pub const SAE_GROUP_P256: u16 = 19;
/// Octets of one group-19 scalar or field element: the length of the P-256
/// prime.
///
/// SOURCE: IEEE Std 802.11-2020, 12.4.7.2 (Data type conversion); RFC 5903,
/// 3.1 (256-bit random ECP group).
pub const SAE_PRIME_LEN: usize = 32;
/// Octets of a group-19 Element field: the x and y coordinates.
pub const SAE_ELEMENT_LEN: usize = 2 * SAE_PRIME_LEN;

/// The x and y coordinates of an ECC group element.
///
/// SOURCE: IEEE Std 802.11-2020, 12.4.7.2.4 (ECC element to octet string:
/// x then y, each the length of the prime).
pub const fn element_coordinates(
    element: &[u8; SAE_ELEMENT_LEN],
) -> Option<(&[u8; SAE_PRIME_LEN], &[u8; SAE_PRIME_LEN])> {
    match (
        element.first_chunk::<SAE_PRIME_LEN>(),
        element.last_chunk::<SAE_PRIME_LEN>(),
    ) {
        (Some(x), Some(y)) => Some((x, y)),
        _ => None,
    }
}

/// The Finite Cyclic Group field of `group`, little-endian.
///
/// SOURCE: IEEE Std 802.11-2020, 9.4.1.38 (Finite Cyclic Group field).
pub const fn finite_cyclic_group_field(group: u16) -> [u8; SAE_FIELD_LEN] {
    group.to_le_bytes()
}
/// An SAE Commit body of group 19 without an anti-clogging token or
/// optional elements: group, scalar and element.
pub const SAE_COMMIT_LEN: usize = SAE_FIELD_LEN + SAE_COMMIT_VALUES_LEN;
/// Octets of the Scalar and Element fields of group 19.
const SAE_COMMIT_VALUES_LEN: usize = SAE_PRIME_LEN + SAE_ELEMENT_LEN;
/// Octets of the Confirm field of group 19: an HMAC-SHA-256 output.
pub const SAE_CONFIRM_FIELD_LEN: usize = 32;
/// An SAE Confirm body: send-confirm and the confirm value.
pub const SAE_CONFIRM_LEN: usize = SAE_FIELD_LEN + SAE_CONFIRM_FIELD_LEN;
/// Octets of an anti-clogging token as the vendor's access point issues it:
/// its index and an HMAC-SHA256 tail. A hunting-and-pecking Commit carries
/// a token of at least this length between its group and its scalar.
///
/// SOURCE: ESP-IDF `7b9cc1ac79f865983f59bb8ff3ff43eb74ff1dbe`
/// `components/wpa_supplicant/src/ap/comeback_token.c`
/// (`auth_build_token_req`) and `src/common/sae.c` (`sae_parse_commit_token`).
pub const SAE_ANTI_CLOGGING_TOKEN_LEN: usize = 32;

/// Why an SAE body does not decode or encode.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum SaeBodyError {
    /// A body of the wrong length or with a malformed trailing element, or
    /// an output too short for the body.
    Malformed,
    /// A Commit of another group.
    UnsupportedGroup(u16),
}

/// One peer's commit: its scalar and element as byte values.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct SaeCommitValues {
    pub scalar: [u8; SAE_PRIME_LEN],
    pub element: [u8; SAE_ELEMENT_LEN],
}

impl SaeCommitValues {
    /// Parse an access point's group-19 commit body. Hunting and pecking
    /// admits nothing after the element. H2E admits a Rejected Groups
    /// element that does not reject group 19, as the vendor's
    /// `wpa3_check_sae_rejected_groups` does; a Password Identifier, which
    /// this station never sends, is refused.
    pub fn parse(body: &[u8], h2e: bool) -> Result<Self, SaeBodyError> {
        let mut reader = Reader::new(body);
        let group = reader.u16_le().map_err(|_| SaeBodyError::Malformed)?;
        if group != SAE_GROUP_P256 {
            return Err(SaeBodyError::UnsupportedGroup(group));
        }
        let values = Self::read(&mut reader).ok_or(SaeBodyError::Malformed)?;
        let mut rest = reader.remaining();
        while !rest.is_empty() {
            if !h2e {
                return Err(SaeBodyError::Malformed);
            }
            let (extension, value, tail) =
                extension_element_at(rest).ok_or(SaeBodyError::Malformed)?;
            match extension {
                REJECTED_GROUPS_EXTENSION_ID
                    if value.len().is_multiple_of(2) && !contains_offered_group(value) => {}
                _ => return Err(SaeBodyError::Malformed),
            }
            rest = tail;
        }
        Ok(values)
    }

    /// The Scalar and Element fields at `reader`.
    fn read(reader: &mut Reader<'_>) -> Option<Self> {
        let scalar = reader.read::<SAE_PRIME_LEN>().ok()?;
        let element = reader.read::<SAE_ELEMENT_LEN>().ok()?;
        Some(Self { scalar, element })
    }

    /// A station's commit body: group, the anti-clogging token of hunting
    /// and pecking, scalar and element, then the token container of H2E, as
    /// the vendor's `sae_write_commit` orders them. Returns the length.
    pub fn encode(
        &self,
        token: Option<&[u8]>,
        h2e: bool,
        output: &mut [u8],
    ) -> Result<usize, SaeBodyError> {
        let token = token.unwrap_or(&[]);
        let container = if h2e && !token.is_empty() {
            Some(anti_clogging_token_container_header(token.len()).ok_or(SaeBodyError::Malformed)?)
        } else {
            None
        };
        let length = SAE_COMMIT_LEN
            .checked_add(token.len())
            .and_then(|length| length.checked_add(if container.is_some() { 3 } else { 0 }))
            .ok_or(SaeBodyError::Malformed)?;
        let mut writer = Writer::new(output.get_mut(..length).ok_or(SaeBodyError::Malformed)?);
        let mut write = || -> Result<(), oer_wire::Error> {
            writer.u16_le(SAE_GROUP_P256)?;
            if !h2e {
                writer.write(token)?;
            }
            writer.write(&self.scalar)?;
            writer.write(&self.element)?;
            if let Some(container) = container {
                writer.write(&container)?;
                writer.write(token)?;
            }
            Ok(())
        };
        write().map_err(|_| SaeBodyError::Malformed)?;
        Ok(length)
    }
}

/// Why an access point refuses a station's commit before any computation.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum SaeCommitRefusal {
    /// A group other than 19: status 77.
    UnsupportedGroup,
    /// A truncated body, a scalar outside 1 < scalar < r, an element off the
    /// curve or a malformed trailing element: status 1.
    Unspecified,
}

/// A station's commit as the access point receives it.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct SaeReceivedCommit<'a> {
    pub values: SaeCommitValues,
    /// The anti-clogging token the station echoed.
    pub token: Option<&'a [u8]>,
    /// The station named a password identifier.
    pub password_identifier: bool,
    /// The Rejected Groups payload of an H2E commit: little-endian groups.
    pub rejected_groups: Option<&'a [u8]>,
}

impl<'a> SaeReceivedCommit<'a> {
    /// Parse a station's group-19 commit as the vendor's `sae_parse_commit`
    /// does: under hunting and pecking, octets between the group and the
    /// scalar and element are the anti-clogging token when at least
    /// [`SAE_ANTI_CLOGGING_TOKEN_LEN`]; under H2E, a Password Identifier, a
    /// Rejected Groups element and an Anti-Clogging Token Container may follow
    /// the element. Whether the scalar and element are valid is for the
    /// cryptography to decide.
    pub fn parse(body: &'a [u8], h2e: bool) -> Result<Self, SaeCommitRefusal> {
        let mut reader = Reader::new(body);
        let group = reader.u16_le().map_err(|_| SaeCommitRefusal::Unspecified)?;
        if group != SAE_GROUP_P256 {
            return Err(SaeCommitRefusal::UnsupportedGroup);
        }
        let mut token = None;
        if !h2e
            && let Some(extra) = reader.remaining().len().checked_sub(SAE_COMMIT_VALUES_LEN)
            && extra > 0
            && extra >= SAE_ANTI_CLOGGING_TOKEN_LEN
            && let Ok(echoed) = reader.take(extra)
        {
            token = Some(echoed);
        }
        let values = SaeCommitValues::read(&mut reader).ok_or(SaeCommitRefusal::Unspecified)?;
        let mut commit = Self {
            values,
            token,
            password_identifier: false,
            rejected_groups: None,
        };
        let mut rest = reader.remaining();
        if let Some((_, tail)) = extension_element(rest, PASSWORD_IDENTIFIER_EXTENSION_ID, 1) {
            commit.password_identifier = true;
            rest = tail;
        }
        if h2e {
            if let Some((value, tail)) = extension_element(rest, REJECTED_GROUPS_EXTENSION_ID, 2) {
                if !value.len().is_multiple_of(2) {
                    return Err(SaeCommitRefusal::Unspecified);
                }
                commit.rejected_groups = Some(value);
                rest = tail;
            }
            if let Some((value, _)) =
                extension_element(rest, ANTI_CLOGGING_TOKEN_CONTAINER_EXTENSION_ID, 1)
            {
                commit.token = Some(value);
            }
        }
        Ok(commit)
    }

    /// Whether the station rejected group 19, the only group offered.
    pub fn rejects_offered_group(&self) -> bool {
        self.rejected_groups.is_some_and(contains_offered_group)
    }
}

/// The anti-clogging token of a commit refused with status 76: after the
/// group under hunting and pecking, inside its container element under H2E.
pub fn anti_clogging_token(body: &[u8], h2e: bool) -> Result<&[u8], SaeBodyError> {
    let mut reader = Reader::new(body);
    reader.u16_le().map_err(|_| SaeBodyError::Malformed)?;
    let rest = reader.remaining();
    if !h2e {
        return Ok(rest);
    }
    match extension_element_at(rest) {
        Some((ANTI_CLOGGING_TOKEN_CONTAINER_EXTENSION_ID, token, _)) => Ok(token),
        _ => Err(SaeBodyError::Malformed),
    }
}

/// An SAE Confirm body: its send-confirm and confirm value.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct SaeConfirm<'a> {
    pub send_confirm: u16,
    pub confirm: &'a [u8; SAE_CONFIRM_FIELD_LEN],
}

impl<'a> SaeConfirm<'a> {
    /// Parse a group-19 Confirm body of exactly [`SAE_CONFIRM_LEN`] octets.
    pub fn parse(body: &'a [u8]) -> Result<Self, SaeBodyError> {
        let mut reader = Reader::new(body);
        let send_confirm = reader.u16_le().map_err(|_| SaeBodyError::Malformed)?;
        let confirm = reader
            .array::<SAE_CONFIRM_FIELD_LEN>()
            .map_err(|_| SaeBodyError::Malformed)?;
        if !reader.is_empty() {
            return Err(SaeBodyError::Malformed);
        }
        Ok(Self {
            send_confirm,
            confirm,
        })
    }

    /// The Confirm body of `send_confirm` and `confirm`.
    pub fn encode(self) -> [u8; SAE_CONFIRM_LEN] {
        let mut body = [0; SAE_CONFIRM_LEN];
        for (target, source) in body.iter_mut().zip(
            self.send_confirm
                .to_le_bytes()
                .iter()
                .chain(self.confirm.iter()),
        ) {
            *target = *source;
        }
        body
    }
}

/// The extension element at the start of `bytes`: its Element ID Extension,
/// its value and the bytes after it; `None` when no complete extension
/// element starts there.
///
/// SOURCE: IEEE Std 802.11-2020, 9.4.2.1 (General: Element ID, Length,
/// Element ID Extension).
fn extension_element_at(bytes: &[u8]) -> Option<(u8, &[u8], &[u8])> {
    let mut reader = Reader::new(bytes);
    if reader.u8().ok()? != ELEMENT_ID_EXTENSION {
        return None;
    }
    let length = reader.u8().ok()?;
    let mut element = Reader::new(reader.take(usize::from(length)).ok()?);
    let extension = element.u8().ok()?;
    Some((extension, element.remaining(), reader.remaining()))
}

/// The extension element `extension` at the start of `bytes`, whose length
/// field is at least `minimum_length`, and the bytes after it; `None` when
/// another element or nothing starts there.
fn extension_element(bytes: &[u8], extension: u8, minimum_length: u8) -> Option<(&[u8], &[u8])> {
    let (found, value, tail) = extension_element_at(bytes)?;
    // The length field counts the Element ID Extension and the value.
    (found == extension && value.len() >= usize::from(minimum_length).saturating_sub(1))
        .then_some((value, tail))
}

/// Whether a Rejected Groups payload of little-endian groups names group 19.
fn contains_offered_group(mut groups: &[u8]) -> bool {
    while let Some((group, rest)) = groups.split_first_chunk::<SAE_FIELD_LEN>() {
        if *group == SAE_GROUP_P256.to_le_bytes() {
            return true;
        }
        groups = rest;
    }
    false
}

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

    const VALUES: SaeCommitValues = SaeCommitValues {
        scalar: [0x11; SAE_PRIME_LEN],
        element: [0x22; SAE_ELEMENT_LEN],
    };

    fn commit_body() -> [u8; SAE_COMMIT_LEN] {
        let mut body = [0; SAE_COMMIT_LEN];
        assert_eq!(VALUES.encode(None, false, &mut body), Ok(SAE_COMMIT_LEN));
        body
    }

    #[test]
    fn a_commit_is_group_scalar_and_element_and_refuses_other_groups() {
        let mut body = commit_body();
        assert_eq!(&body[..2], &[19, 0]);
        assert_eq!(&body[2..34], &VALUES.scalar);
        assert_eq!(&body[34..], &VALUES.element);
        assert_eq!(SaeCommitValues::parse(&body, false), Ok(VALUES));
        body[0] = 20;
        assert_eq!(
            SaeCommitValues::parse(&body, false),
            Err(SaeBodyError::UnsupportedGroup(20))
        );
        body[0] = 19;
        assert_eq!(
            SaeCommitValues::parse(&body[..10], false),
            Err(SaeBodyError::Malformed)
        );
        assert_eq!(
            VALUES.encode(None, false, &mut [0; SAE_COMMIT_LEN - 1]),
            Err(SaeBodyError::Malformed)
        );
    }

    #[test]
    fn tokens_sit_where_the_vendor_writes_them() {
        let token = [0xab; 5];
        let mut hunting = [0; 128];
        let length = VALUES.encode(Some(&token), false, &mut hunting).unwrap();
        assert_eq!(length, SAE_COMMIT_LEN + 5);
        assert_eq!(&hunting[2..7], &token);
        assert_eq!(&hunting[7..39], &VALUES.scalar);

        let mut h2e = [0; 128];
        let length = VALUES.encode(Some(&token), true, &mut h2e).unwrap();
        assert_eq!(length, SAE_COMMIT_LEN + 3 + 5);
        assert_eq!(&h2e[2..34], &VALUES.scalar);
        assert_eq!(&h2e[SAE_COMMIT_LEN..SAE_COMMIT_LEN + 3], &[255, 6, 93]);

        // A status-76 refusal carries the token after the group.
        assert_eq!(
            anti_clogging_token(&[19, 0, 1, 2, 3], false),
            Ok(&[1, 2, 3][..])
        );
        assert_eq!(
            anti_clogging_token(&[19, 0, 255, 4, 93, 1, 2, 3], true),
            Ok(&[1, 2, 3][..])
        );
        assert!(anti_clogging_token(&[19, 0, 1, 2, 3], true).is_err());
        assert!(anti_clogging_token(&[19, 0, 255, 0, 93], true).is_err());
        assert!(anti_clogging_token(&[19], false).is_err());
    }

    #[test]
    fn an_h2e_commit_admits_only_a_rejected_groups_element_without_19() {
        let mut body = [0; SAE_COMMIT_LEN + 5];
        VALUES.encode(None, true, &mut body).unwrap();
        body[SAE_COMMIT_LEN..].copy_from_slice(&[255, 3, 92, 20, 0]);
        assert_eq!(SaeCommitValues::parse(&body, true), Ok(VALUES));
        assert_eq!(
            SaeCommitValues::parse(&body, false),
            Err(SaeBodyError::Malformed)
        );
        body[SAE_COMMIT_LEN + 3] = 19;
        assert_eq!(
            SaeCommitValues::parse(&body, true),
            Err(SaeBodyError::Malformed)
        );
    }

    #[test]
    fn an_access_point_parses_the_commit_a_station_writes() {
        let token = [0x5a; SAE_ANTI_CLOGGING_TOKEN_LEN];
        let mut body = [0; SAE_COMMIT_LEN + SAE_ANTI_CLOGGING_TOKEN_LEN + 3];

        let len = VALUES.encode(None, false, &mut body).unwrap();
        let received = SaeReceivedCommit::parse(&body[..len], false).unwrap();
        assert_eq!(received.values, VALUES);
        assert_eq!(received.token, None);

        let len = VALUES.encode(Some(&token), false, &mut body).unwrap();
        let received = SaeReceivedCommit::parse(&body[..len], false).unwrap();
        assert_eq!(received.values, VALUES);
        assert_eq!(received.token, Some(&token[..]));

        let len = VALUES.encode(Some(&token), true, &mut body).unwrap();
        let received = SaeReceivedCommit::parse(&body[..len], true).unwrap();
        assert_eq!(received.values, VALUES);
        assert_eq!(received.token, Some(&token[..]));
        assert!(!received.rejects_offered_group());
    }

    #[test]
    fn an_access_point_reads_rejected_groups_and_refuses_bad_bodies() {
        let commit = commit_body();
        let mut body = [0; SAE_COMMIT_LEN + 12];
        body[..SAE_COMMIT_LEN].copy_from_slice(&commit);
        body[SAE_COMMIT_LEN..].copy_from_slice(&[255, 5, 92, 20, 0, 19, 0, 255, 2, 93, 7, 0]);
        let received = SaeReceivedCommit::parse(&body, true).unwrap();
        assert_eq!(received.rejected_groups, Some(&[20, 0, 19, 0][..]));
        assert!(received.rejects_offered_group());
        assert_eq!(received.token, Some(&[7][..]));

        let mut odd = body;
        odd[SAE_COMMIT_LEN + 1] = 4;
        assert_eq!(
            SaeReceivedCommit::parse(&odd[..SAE_COMMIT_LEN + 6], true),
            Err(SaeCommitRefusal::Unspecified)
        );
        let mut other_group = commit;
        other_group[0] = 20;
        assert_eq!(
            SaeReceivedCommit::parse(&other_group, false),
            Err(SaeCommitRefusal::UnsupportedGroup)
        );
        assert_eq!(
            SaeReceivedCommit::parse(&commit[..SAE_COMMIT_LEN - 1], false),
            Err(SaeCommitRefusal::Unspecified)
        );
        // A password identifier is reported, not refused.
        let mut named = [0; SAE_COMMIT_LEN + 4];
        named[..SAE_COMMIT_LEN].copy_from_slice(&commit);
        named[SAE_COMMIT_LEN..].copy_from_slice(&[255, 2, 33, b'x']);
        assert!(
            SaeReceivedCommit::parse(&named, false)
                .unwrap()
                .password_identifier
        );
    }

    #[test]
    fn a_confirm_is_send_confirm_and_its_value_of_exact_length() {
        let confirm = [0x5c; SAE_CONFIRM_FIELD_LEN];
        let body = SaeConfirm {
            send_confirm: 0x0201,
            confirm: &confirm,
        }
        .encode();
        assert_eq!(&body[..2], &[1, 2]);
        assert_eq!(&body[2..], &confirm);
        assert_eq!(
            SaeConfirm::parse(&body),
            Ok(SaeConfirm {
                send_confirm: 0x0201,
                confirm: &confirm,
            })
        );
        assert_eq!(
            SaeConfirm::parse(&body[..SAE_CONFIRM_LEN - 1]),
            Err(SaeBodyError::Malformed)
        );
        let mut long = [0; SAE_CONFIRM_LEN + 1];
        long[..SAE_CONFIRM_LEN].copy_from_slice(&body);
        assert_eq!(SaeConfirm::parse(&long), Err(SaeBodyError::Malformed));
    }

    #[test]
    fn sae_bodies_never_panic_on_any_short_input() {
        let mut bytes = [0xff_u8; SAE_COMMIT_LEN + 8];
        bytes[0] = 19;
        bytes[1] = 0;
        for length in 0..=bytes.len() {
            for h2e in [false, true] {
                let body = &bytes[..length];
                let _ = SaeCommitValues::parse(body, h2e);
                let _ = SaeReceivedCommit::parse(body, h2e);
                let _ = anti_clogging_token(body, h2e);
                let _ = SaeConfirm::parse(body);
            }
        }
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
