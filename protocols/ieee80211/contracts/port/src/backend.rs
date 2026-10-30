//! Values a backend shares with the family policy and the retry owner: the
//! format family of a rate-schedule record and a frame's retry counters.

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
#[repr(u8)]
/// Format family of one rate-schedule record supplied by the family policy.
pub enum RateScheduleKind {
    // Deliberately sparse internal tags keep LLVM from materializing a
    // nine-pointer jump table in internal SRAM.  These values never cross the
    // source-owned schedule identity.
    /// OFDM control exchange record.
    BarOfdm = 0x03,
    /// Basic OFDM record.
    BasicOfdm = 0x19,
    /// HE record.
    Dot11Ax = 0x2f,
    /// DSSS/CCK record.
    Dot11B = 0x44,
    /// Non-HT OFDM record.
    Dot11G = 0x62,
    /// HT record.
    Dot11N = 0x83,
    /// Espressif LR record.
    Lora = 0xa1,
    /// Dedicated P2P OFDM record.
    P2pDot11G = 0xc8,
    /// Dedicated P2P HT record.
    P2pDot11N = 0xf1,
}

/// The counters of one frame's failed attempts.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct RetryCounters {
    /// Unacknowledged transmissions of the frame itself.
    pub mpdu: u8,
    /// Failures charged to the short retry counter.
    pub short: u8,
    /// Failures charged to the long retry counter.
    pub long: u8,
}
