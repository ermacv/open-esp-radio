//! Driver observation points. Counters belong to the image-selected observer.
use crate::mac::tx::{LegacyTxQueue, runtime::WifiTxRuntimePolicy};
use oer_esp32s31_instrumentation::wifi::*;
use oer_ieee80211_port::VifRole;
use oer_ieee80211_wire::qos::WmmAccessCategory;
#[inline(never)]
pub(crate) fn contention(
    observer: &impl oer_trace::Observer,
    policy: &WifiTxRuntimePolicy,
    head: VifRole,
    ac: WmmAccessCategory,
    backoff: u16,
) {
    for role in [VifRole::Station, VifRole::AccessPoint] {
        let set = policy.observed_edca(role);
        let wmm = set.wmm();
        oer_trace::observe!(
            observer,
            EdcaTable {
                role,
                count: wmm.parameter_set_count,
                uapsd: wmm.uapsd,
            }
        );
        for category in WmmAccessCategory::ALL {
            oer_trace::observe!(
                observer,
                EdcaRecord {
                    role,
                    category,
                    // CheckedEdca caps the lifetime at DEFAULT_MSDU_LIFETIME (512 ms).
                    lifetime_micros: set.access_category(category).msdu_lifetime.as_micros() as u32,
                    wmm: set.access_category(category).wmm,
                }
            );
        }
    }
    oer_trace::observe!(
        observer,
        EdcaDraw {
            role: head,
            category: ac,
            exponent: policy.contention_exponent(LegacyTxQueue::from_access_category(ac)),
            backoff,
        }
    );
}
