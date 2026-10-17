//! The Espressif LMAC's retry limits, lifetimes and default contention.
//!
//! The values are those `libpp[lmac.o]::lmacInit` installs in the pinned
//! ESP32-S31 library. They parameterize the portable algorithms of
//! `oer-ieee80211-upper-mac`, which a backend running the Espressif policy
//! instantiates with them.

use oer_ieee80211_lower_mac::Ieee80211Duration;
use oer_ieee80211_mac::qos::WmmAccessCategory;
use oer_ieee80211_softmac::EdcaContention;
use oer_ieee80211_upper_mac::{AckFailureAccounting, AmpduRetryPolicy, RetryLimits};

/// Short retry limit: `lmacConfMib[0x15]` of `lmacInit`.
pub const SHORT_RETRY_LIMIT: u8 = 0x20;
/// Long retry limit: `lmacConfMib[0x14]` of `lmacInit`.
pub const LONG_RETRY_LIMIT: u8 = 0x20;
/// dot11RTSThreshold in PSDU octets: `lmacInit` stores 0x092a at
/// `lmacConfMib+0x16`, and `lmacIsLongFrame` requests RTS for a longer
/// individual PSDU.
pub const RTS_THRESHOLD_BYTES: u16 = 0x092a;

/// The LMAC's retry counting.
///
/// SOURCE(esp32s31): `libpp[lmac.o]::lmacProcessAckTimeout`. Only a frame inside a
/// granted TXOP (descriptor word-0 bit 8) reaches `libpp[lmac.o]::lmacProcessLongRetryFail`;
/// every other frame, whatever its length, reaches
/// `libpp[lmac.o]::lmacProcessShortRetryFail(context, 0, 0, _)`, which counts the MPDU and
/// short retries against the short limit. A policy that requests no TXOP
/// therefore counts every ACK timeout as short.
pub const RETRY_LIMITS: RetryLimits = RetryLimits {
    short: SHORT_RETRY_LIMIT,
    long: LONG_RETRY_LIMIT,
    ack_failure: AckFailureAccounting::Short,
};

/// Lifetime of an A-MPDU member MSDU, in microseconds: 1536 lifetime units
/// of 1024 µs.
///
/// SOURCE(esp32s31): `libpp[lmac.o]::lmacMSDUAged` compares the time since the
/// MSDU's enqueue timestamp with the `lmacConfMib` lifetime that `lmacInit`
/// installs, 1536 units for an aggregate member (descriptor bit 22) and 1024
/// for an ordinary MPDU, each unit `<< 10` microseconds; executed in an
/// A-MPDU-resort comparison with the vendor library.
pub const AMPDU_MSDU_LIFETIME_MICROS: u32 = 1536 << 10;

/// An MSDU with less than one lifetime unit left is aged.
///
/// SOURCE(esp32s31): `libpp[lmac.o]::lmacMSDUAged`, as for
/// [`AMPDU_MSDU_LIFETIME_MICROS`].
pub const MSDU_AGED_MARGIN_MICROS: u32 = 1 << 10;

/// The A-MPDU retry policy of the LMAC with a caller-chosen lifetime.
///
/// SOURCE(esp32s31): `libpp[pp.o]::ppResortTxAMPDU` keeps a partial BlockAck's
/// missing MPDUs in the aggregate without counting publications; only their
/// MSDU lifetime bounds those retries. `libpp[lmac.o]::lmacProcessAckTimeout`
/// enters `libpp[lmac.o]::lmacProcessShortRetryFail`/`libpp[lmac.o]::lmacProcessLongRetryFail`, which
/// republish an aggregate without any BlockAck until the `lmacConfMib` retry
/// limit; executed, both descriptor lengths end on the 32nd timeout through
/// `libpp[lmac.o]::lmacEndFrameExchangeSequence` (executed). The `libpp[trc.o]::rcReachRetryLimit`
/// cap of 11 attempts applies only while ESP-WIFI-MESH runs
/// (`g_mesh_is_started`). `libpp[lmac.o]::lmacProcessCtsTimeout` counts a failed protection
/// exchange against the same short limit, and `libpp[lmac.o]::lmacEndRetryAMPDUFail` then
/// keeps the aggregate and sends a BlockAckReq (executed).
pub const fn ampdu_retry_policy(
    lifetime: Ieee80211Duration,
    retain_single_mpdu: bool,
) -> AmpduRetryPolicy {
    AmpduRetryPolicy {
        lifetime,
        aged_margin: Ieee80211Duration::from_micros(MSDU_AGED_MARGIN_MICROS as u64),
        retry_limit: SHORT_RETRY_LIMIT,
        retain_single_mpdu,
    }
}

/// One access category's default contention: AIFSN and the CW exponents.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct DefaultContention {
    pub aifsn: u8,
    pub ecw_min: u8,
    pub ecw_max: u8,
}

/// The contention an access category starts with before a BSS advertises
/// EDCA parameters.
///
/// SOURCE(esp32s31): complete `libpp[lmac.o]::lmacInit` and `lmacInitAc`. The five
/// arguments are queue, AIFSN, ECWmin, ECWmax, and TXOP. Ordinary queues
/// are VO=(2,2,3), VI=(2,3,4), BE=(3,4,10), and BK=(7,4,10).
pub const fn default_contention(access_category: WmmAccessCategory) -> DefaultContention {
    match access_category {
        WmmAccessCategory::Voice => DefaultContention {
            aifsn: 2,
            ecw_min: 2,
            ecw_max: 3,
        },
        WmmAccessCategory::Video => DefaultContention {
            aifsn: 2,
            ecw_min: 3,
            ecw_max: 4,
        },
        WmmAccessCategory::BestEffort => DefaultContention {
            aifsn: 3,
            ecw_min: 4,
            ecw_max: 10,
        },
        WmmAccessCategory::Background => DefaultContention {
            aifsn: 7,
            ecw_min: 4,
            ecw_max: 10,
        },
    }
}

/// The default contention of the four access categories, indexed by ACI,
/// as the transmit planner holds it.
pub const fn default_edca() -> [EdcaContention; 4] {
    const fn contention(access_category: WmmAccessCategory) -> EdcaContention {
        let defaults = default_contention(access_category);
        EdcaContention::new(defaults.ecw_min, defaults.ecw_max)
    }
    [
        contention(WmmAccessCategory::BestEffort),
        contention(WmmAccessCategory::Background),
        contention(WmmAccessCategory::Video),
        contention(WmmAccessCategory::Voice),
    ]
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn defaults_are_indexed_by_aci() {
        let edca = default_edca();
        assert_eq!(edca[WmmAccessCategory::Voice as usize].cw(), 3);
        assert_eq!(edca[WmmAccessCategory::BestEffort as usize].cw(), 15);
        assert_eq!(
            default_contention(WmmAccessCategory::Background),
            DefaultContention {
                aifsn: 7,
                ecw_min: 4,
                ecw_max: 10
            }
        );
        let policy = ampdu_retry_policy(
            Ieee80211Duration::from_micros(u64::from(AMPDU_MSDU_LIFETIME_MICROS)),
            false,
        );
        assert_eq!(policy.retry_limit, 32);
        assert_eq!(policy.aged_margin, Ieee80211Duration::from_micros(1024));
    }
}
