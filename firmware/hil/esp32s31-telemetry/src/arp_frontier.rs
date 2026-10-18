//! ARP identity preserved across observed radio and network admission edges.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct Identity {
    pub operation: u16,
    pub source: [u8; 4],
    pub target: [u8; 4],
}
impl Identity {
    pub fn parse(ether_type: u16, payload: &[u8]) -> Option<Self> {
        if ether_type != 0x0806 || payload.get(..6)? != [0, 1, 8, 0, 6, 4] {
            return None;
        }
        Some(Self {
            operation: u16::from_be_bytes(payload.get(6..8)?.try_into().ok()?),
            source: payload.get(14..18)?.try_into().ok()?,
            target: payload.get(24..28)?.try_into().ok()?,
        })
    }
}
#[derive(Clone, Copy, Debug)]
pub enum Stage {
    Radio,
    Admitted,
    StackReceived,
    TxAccepted,
    TxRejected,
    QueueFull,
    PoolExhausted,
    LinkDown,
    InvalidLength,
}
#[derive(Clone, Copy, Debug)]
pub struct Sample {
    pub at_us: u64,
    pub stage: Stage,
    pub identity: Identity,
    pub ethernet_destination: [u8; 6],
}
pub struct Records {
    session: Option<u64>,
    pub total: u32,
    pub samples: [Option<Sample>; 32],
}
impl Records {
    pub const fn new() -> Self {
        Self {
            session: None,
            total: 0,
            samples: [None; 32],
        }
    }
    pub fn begin(&mut self, session: u64) {
        *self = Self::new();
        self.session = Some(session);
    }
    pub fn observe(&mut self, sample: Sample) {
        if self.session.is_none() {
            return;
        }
        if let Some(slot) = self.samples.get_mut(self.total as usize) {
            *slot = Some(sample);
        }
        self.total = self.total.saturating_add(1);
    }
    pub fn end(&mut self, session: u64) -> Option<u32> {
        if self.session != Some(session) {
            return None;
        }
        self.session = None;
        Some(self.total)
    }
}
impl Default for Records {
    fn default() -> Self {
        Self::new()
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn records_are_bounded_frozen_and_reset_by_session() {
        let sample = Sample {
            at_us: 1,
            stage: Stage::TxRejected,
            identity: Identity {
                operation: 2,
                source: [1; 4],
                target: [2; 4],
            },
            ethernet_destination: [3; 6],
        };
        let mut records = Records::new();
        records.observe(sample);
        assert_eq!(records.total, 0);
        records.begin(7);
        for _ in 0..40 {
            records.observe(sample);
        }
        assert_eq!(records.samples.iter().flatten().count(), 32);
        assert_eq!(records.end(8), None);
        assert_eq!(records.end(7), Some(40));
        records.observe(sample);
        assert_eq!(records.total, 40);
        records.begin(8);
        assert_eq!(records.total, 0);
        assert_eq!(records.samples.iter().flatten().count(), 0);
    }

    #[test]
    fn arp_identity_requires_complete_ethernet_ipv4_arp_and_preserves_addresses() {
        let packet = [
            0, 1, 8, 0, 6, 4, 0, 1, 1, 2, 3, 4, 5, 6, 192, 168, 178, 129, 0, 0, 0, 0, 0, 0, 192,
            168, 178, 130,
        ];
        assert_eq!(
            Identity::parse(0x0806, &packet),
            Some(Identity {
                operation: 1,
                source: [192, 168, 178, 129],
                target: [192, 168, 178, 130]
            })
        );
        for n in 0..packet.len() {
            assert_eq!(Identity::parse(0x0806, &packet[..n]), None);
        }
        assert_eq!(Identity::parse(0x0800, &packet), None);
        let mut ipv6 = packet;
        ipv6[3] = 0xdd;
        assert_eq!(Identity::parse(0x0806, &ipv6), None);
    }
}
