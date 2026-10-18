//! Bounded first-occurrence evidence; records never backpressure RX delivery.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct Sample {
    pub observed_us: u64,
    pub udp_sequence: Option<i32>,
    pub ip_bytes: usize,
    /// QoS TID and MAC sequence number of the frame, when present.
    pub qos: Option<(u8, oer_ieee80211_mac::sequence::SequenceNumber)>,
    pub format: u8,
    pub rate: u8,
    pub signal_words: [u32; 2],
    /// 0 unavailable; 1/2 hardware false/true; 3/4 protocol false/true.
    pub ampdu: u8,
}

pub struct Records<const N: usize> {
    session: Option<u64>,
    pub samples: [Option<Sample>; N],
    pub total: u32,
}
impl<const N: usize> Records<N> {
    pub const fn new() -> Self {
        Self {
            session: None,
            samples: [None; N],
            total: 0,
        }
    }
    pub fn begin(&mut self, session: u64) {
        self.session = Some(session);
        self.samples = [None; N];
        self.total = 0;
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
impl<const N: usize> Default for Records<N> {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn bounded_records_preserve_first_packets_and_do_not_cross_sessions() {
        let mut r = Records::<2>::new();
        let mut s = Sample {
            observed_us: 1,
            udp_sequence: Some(-1),
            ip_bytes: 36,
            qos: None,
            format: 0,
            rate: 1,
            signal_words: [0; 2],
            ampdu: 3,
        };
        r.observe(s);
        assert_eq!(r.total, 0);
        r.begin(7);
        for i in 0..4 {
            s.udp_sequence = Some(i);
            r.observe(s);
        }
        assert_eq!(r.end(8), None);
        assert_eq!(r.end(7), Some(4));
        assert_eq!(r.samples[0].unwrap().udp_sequence, Some(0));
        assert_eq!(r.samples[1].unwrap().udp_sequence, Some(1));
        r.observe(s);
        assert_eq!(r.total, 4);
        r.begin(8);
        assert_eq!(r.total, 0);
        assert_eq!(r.samples, [None; 2]);
    }
}
