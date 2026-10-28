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
        let [a, b, c, d, e, f, g, h] = self.nonce.to_be_bytes();
        [
            0xff,
            0xff,
            0xff,
            if self.response { 0xfe } else { 0xff },
            b'O',
            b'E',
            b'R',
            b'P',
            a,
            b,
            c,
            d,
            e,
            f,
            g,
            h,
        ]
    }

    pub fn decode(bytes: &[u8]) -> Option<Self> {
        let &[
            0xff,
            0xff,
            0xff,
            response,
            b'O',
            b'E',
            b'R',
            b'P',
            a,
            b,
            c,
            d,
            e,
            f,
            g,
            h,
        ] = bytes
        else {
            return None;
        };
        let response = match response {
            0xff => false,
            0xfe => true,
            _ => return None,
        };
        Some(Self {
            nonce: u64::from_be_bytes([a, b, c, d, e, f, g, h]),
            response,
        })
    }
}

#[cfg(test)]
mod tests;
