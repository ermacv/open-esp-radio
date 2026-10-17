//! Executor-independent infrastructure-STA Authentication and Association.
//!
//! Wire parsing and response values live in `oer-ieee80211-mac`. This
//! module owns Authentication/Association state and retry policy as state
//! machines, which the station ([`crate::machine`]) drives.

pub mod association;
pub mod authentication;
pub mod sae;

#[cfg(test)]
pub(crate) mod test_support;

/// Vendor state timer used by ordinary Authentication and Association.
///
/// SOURCE(esp32s31): complete `libnet80211.a[ieee80211_sta.o]::
/// ieee80211_sta_new_state`, ordinary non-mesh auth branch `.L350` and
/// association branch `.L356`, both arm their software timer with immediate
/// `0x3e8`.
pub const STA_RESPONSE_TIMEOUT: oer_time::Duration = oer_time::Duration::from_secs(1);
