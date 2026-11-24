//! The access point's side of SAE Authentication frame bodies: a station's
//! Commit as the access point receives it, and the leading fields it reads.
//!
//! This file is outside the build: no `mod` names it. Its one consumer, the
//! access point's SAE responder
//! (`oer-protocols-ieee80211-core-security-rsn`, `authenticator/sae.rs`), is
//! outside the build too, until the access point's role serves SAE. It is
//! written as a child of `crate::sae` (`use super::*`) and returns there with
//! the responder.

use super::*;

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
    pub fn parse(body: &'a [u8], pwe: SaePwe) -> Result<Self, SaeCommitRefusal> {
        let h2e = pwe == SaePwe::HashToElement;
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

/// The Finite Cyclic Group field of an SAE Commit body; `None` for a body
/// shorter than the field.
pub fn finite_cyclic_group(body: &[u8]) -> Option<u16> {
    body.first_chunk::<SAE_FIELD_LEN>()
        .map(|&field| SaeField::from_bytes(field).value())
}

/// The Send-Confirm field of an SAE Confirm body; `None` for a body shorter
/// than the field.
pub fn send_confirm(body: &[u8]) -> Option<u16> {
    body.first_chunk::<SAE_FIELD_LEN>()
        .map(|&field| SaeField::from_bytes(field).value())
}

/// The Finite Cyclic Group field of `group`, little-endian.
///
/// SOURCE: IEEE Std 802.11-2020, 9.4.1.38 (Finite Cyclic Group field).
pub const fn finite_cyclic_group_field(group: u16) -> [u8; SAE_FIELD_LEN] {
    SaeField::of(group).to_bytes()
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

#[cfg(test)]
mod tests {
    use super::*;

    const VALUES: SaeCommitValues = SaeCommitValues {
        scalar: [0x11; SAE_PRIME_LEN],
        element: [0x22; SAE_ELEMENT_LEN],
    };

    fn commit_body() -> [u8; SAE_COMMIT_LEN] {
        let mut body = [0; SAE_COMMIT_LEN];
        assert_eq!(
            VALUES.encode(None, SaePwe::HuntingAndPecking, &mut body),
            Ok(SAE_COMMIT_LEN)
        );
        body
    }

    #[test]
    fn leading_fields_read_little_endian_and_refuse_short_bodies() {
        assert_eq!(finite_cyclic_group(&[19, 0, 0xaa]), Some(19));
        assert_eq!(send_confirm(&[1, 2]), Some(0x0201));
        assert_eq!(finite_cyclic_group(&[19]), None);
        assert_eq!(send_confirm(&[]), None);
        assert_eq!(finite_cyclic_group_field(SAE_GROUP_P256), [19, 0]);
        assert_eq!(finite_cyclic_group_field(0x0201), [1, 2]);
    }

    #[test]
    fn an_access_point_parses_the_commit_a_station_writes() {
        let token = [0x5a; SAE_ANTI_CLOGGING_TOKEN_LEN];
        let mut body = [0; SAE_COMMIT_LEN + SAE_ANTI_CLOGGING_TOKEN_LEN + 3];

        let len = VALUES
            .encode(None, SaePwe::HuntingAndPecking, &mut body)
            .unwrap();
        let received = SaeReceivedCommit::parse(&body[..len], SaePwe::HuntingAndPecking).unwrap();
        assert_eq!(received.values, VALUES);
        assert_eq!(received.token, None);

        let len = VALUES
            .encode(Some(&token), SaePwe::HuntingAndPecking, &mut body)
            .unwrap();
        let received = SaeReceivedCommit::parse(&body[..len], SaePwe::HuntingAndPecking).unwrap();
        assert_eq!(received.values, VALUES);
        assert_eq!(received.token, Some(&token[..]));

        let len = VALUES
            .encode(Some(&token), SaePwe::HashToElement, &mut body)
            .unwrap();
        let received = SaeReceivedCommit::parse(&body[..len], SaePwe::HashToElement).unwrap();
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
        let received = SaeReceivedCommit::parse(&body, SaePwe::HashToElement).unwrap();
        assert_eq!(received.rejected_groups, Some(&[20, 0, 19, 0][..]));
        assert!(received.rejects_offered_group());
        assert_eq!(received.token, Some(&[7][..]));

        let mut odd = body;
        odd[SAE_COMMIT_LEN + 1] = 4;
        assert_eq!(
            SaeReceivedCommit::parse(&odd[..SAE_COMMIT_LEN + 6], SaePwe::HashToElement),
            Err(SaeCommitRefusal::Unspecified)
        );
        let mut other_group = commit;
        other_group[0] = 20;
        assert_eq!(
            SaeReceivedCommit::parse(&other_group, SaePwe::HuntingAndPecking),
            Err(SaeCommitRefusal::UnsupportedGroup)
        );
        assert_eq!(
            SaeReceivedCommit::parse(&commit[..SAE_COMMIT_LEN - 1], SaePwe::HuntingAndPecking),
            Err(SaeCommitRefusal::Unspecified)
        );
        // A password identifier is reported, not refused.
        let mut named = [0; SAE_COMMIT_LEN + 4];
        named[..SAE_COMMIT_LEN].copy_from_slice(&commit);
        named[SAE_COMMIT_LEN..].copy_from_slice(&[255, 2, 33, b'x']);
        assert!(
            SaeReceivedCommit::parse(&named, SaePwe::HuntingAndPecking)
                .unwrap()
                .password_identifier
        );
    }

    #[test]
    fn received_commits_never_panic_on_any_short_input() {
        let mut bytes = [0xff_u8; SAE_COMMIT_LEN + 8];
        bytes[0] = 19;
        bytes[1] = 0;
        for length in 0..=bytes.len() {
            for pwe in [SaePwe::HuntingAndPecking, SaePwe::HashToElement] {
                let _ = SaeReceivedCommit::parse(&bytes[..length], pwe);
            }
        }
    }
}
