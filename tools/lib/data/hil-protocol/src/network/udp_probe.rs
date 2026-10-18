//! Unmeasured challenge/response on the exact UDP TX flow, before Start.

/// A probe is distinct from the nonnegative sequence numbers of measured UDP.
/// Receiving a matching response proves both directions; sending a request alone
/// proves neither neighbor resolution nor the reverse firewall/forwarding path.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct UdpProbe {
    pub nonce: u64,
    pub response: bool,
}

impl UdpProbe {
    pub const LENGTH: usize = 16;

    pub fn encode(self) -> [u8; Self::LENGTH] {
        let mut bytes = [0xff; Self::LENGTH];
        bytes[3] = if self.response { 0xfe } else { 0xff };
        bytes[4..8].copy_from_slice(b"OERP");
        bytes[8..].copy_from_slice(&self.nonce.to_be_bytes());
        bytes
    }

    pub fn decode(bytes: &[u8]) -> Option<Self> {
        if bytes.len() != Self::LENGTH || bytes[..3] != [0xff; 3] || &bytes[4..8] != b"OERP" {
            return None;
        }
        let response = match bytes[3] {
            0xff => false,
            0xfe => true,
            _ => return None,
        };
        Some(Self {
            nonce: u64::from_be_bytes(bytes[8..].try_into().ok()?),
            response,
        })
    }
}

#[cfg(test)]
mod tests;
