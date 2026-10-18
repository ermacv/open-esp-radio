//! Whether a UDP receive session compares its payload fill.
use oer_hil_protocol::network::UdpSessionPayloadIdentity;

/// Decided once per session: only a session with an identified flow reads the
/// fill verdict, so an unidentified benchmark never scans received payloads.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct PayloadFillCheck {
    identified: bool,
}

impl PayloadFillCheck {
    /// The check a session with these flow identities needs.
    pub fn for_flows(
        identities: impl IntoIterator<Item = Option<UdpSessionPayloadIdentity>>,
    ) -> Self {
        Self {
            identified: identities.into_iter().any(|identity| identity.is_some()),
        }
    }

    /// Whether `payload` carries the benchmark fill. Always `false` without
    /// an identified flow, where no consumer reads it.
    pub fn fill_matches(self, payload: &[u8]) -> bool {
        self.identified && UdpSessionPayloadIdentity::fill_matches(payload)
    }
}

#[cfg(test)]
mod tests;
