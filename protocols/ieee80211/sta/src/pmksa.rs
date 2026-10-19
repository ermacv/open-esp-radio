//! The station's private PMKSA cache, retained across connections.
//!
//! An SAE authentication yields a PMK and its PMKID. This cache retains both
//! per access point. Join resumes an SAE association with the same access
//! point and SSID using Open System authentication and the cached PMKID in
//! its RSN element. The cache holds at most ten entries; adding to a
//! full cache replaces the oldest, and an entry does not expire, as the
//! vendor supplicant's `dot11RSNAConfigPMKLifetime` is `INT32_MAX`. Only SAE
//! associations are cached: the vendor caches no PSK association
//! (`wpa_key_mgmt_supports_caching`).
//!
//! SOURCE: ESP-IDF `7b9cc1ac79f865983f59bb8ff3ff43eb74ff1dbe`
//! `components/wpa_supplicant/src/rsn_supp/pmksa_cache.c`
//! (`pmksa_cache_max_entries`, `dot11RSNAConfigPMKLifetime`,
//! `pmksa_cache_add`) and `src/rsn_supp/wpa.c` (`wpa_set_bss`, which uses an
//! entry only for the same BSSID, SSID and AKM and flushes it otherwise).

use oer_ieee80211_mac::{address::Bssid, management::MAX_SSID_LEN};
use oer_ieee80211_rsn::Pmk;

/// Entries the vendor's PMKSA cache holds.
pub const STA_PMKSA_CAPACITY: usize = 10;
/// The PMKID length.
pub const STA_PMKID_LEN: usize = 16;

/// One cached SAE security association.
pub struct StaPmksa {
    bssid: Bssid,
    ssid: heapless::Vec<u8, MAX_SSID_LEN>,
    pmk: Pmk,
    pmkid: [u8; STA_PMKID_LEN],
}

impl StaPmksa {
    /// Retain an authenticated SAE association with its checked SSID.
    pub fn new(
        bssid: Bssid,
        ssid: heapless::Vec<u8, MAX_SSID_LEN>,
        pmk: Pmk,
        pmkid: [u8; STA_PMKID_LEN],
    ) -> Self {
        Self {
            bssid,
            ssid,
            pmk,
            pmkid,
        }
    }

    /// The SAE-derived pairwise master key borrowed for this association.
    pub const fn pmk(&self) -> &Pmk {
        &self.pmk
    }

    /// The identifier advertised in the association request's RSN element.
    pub const fn pmkid(&self) -> [u8; STA_PMKID_LEN] {
        self.pmkid
    }

    fn has_ssid(&self, ssid: &[u8]) -> bool {
        self.ssid.as_slice() == ssid
    }
}

/// The station's cached SAE security associations, oldest first.
///
/// The station owns this cache by value and retains it across station
/// connections. Access requires a short exclusive borrow; no shared mutex
/// owner or implicit synchronization sits in the protocol crate.
pub struct StaPmksaCache {
    entries: [Option<StaPmksa>; STA_PMKSA_CAPACITY],
}

impl StaPmksaCache {
    /// An empty cache, owned across successive connections.
    pub const fn new() -> Self {
        Self {
            entries: [const { None }; STA_PMKSA_CAPACITY],
        }
    }

    /// The association to resume with `bssid` of `ssid`. An entry of the
    /// same access point under another SSID is flushed, as the vendor
    /// flushes a cached SAE association whose SSID changed.
    pub fn resume(&mut self, bssid: Bssid, ssid: &[u8]) -> Option<&StaPmksa> {
        let mut remaining = self.entries.iter_mut();
        let slot =
            remaining.find(|slot| slot.as_ref().is_some_and(|entry| entry.bssid == bssid))?;
        if slot.as_ref().is_some_and(|entry| entry.has_ssid(ssid)) {
            return slot.as_ref();
        }
        Self::forget(slot, remaining);
        None
    }

    /// Cache the association `entry` names, replacing an entry of the same
    /// access point; a full cache drops its oldest entry first.
    pub fn insert(&mut self, entry: StaPmksa) {
        self.remove(entry.bssid);
        match self.entries.iter_mut().find(|slot| slot.is_none()) {
            Some(slot) => *slot = Some(entry),
            // Full: the oldest goes, the new entry takes the last place.
            None => {
                self.entries.rotate_left(1);
                let [_, _, _, _, _, _, _, _, _, last] = &mut self.entries;
                *last = Some(entry);
            }
        }
    }

    /// Forget the association with `bssid`, keeping the others in order.
    pub fn remove(&mut self, bssid: Bssid) {
        let mut remaining = self.entries.iter_mut();
        if let Some(slot) =
            remaining.find(|slot| slot.as_ref().is_some_and(|entry| entry.bssid == bssid))
        {
            Self::forget(slot, remaining);
        }
    }

    fn forget<'a>(
        mut empty: &'a mut Option<StaPmksa>,
        remaining: impl Iterator<Item = &'a mut Option<StaPmksa>>,
    ) {
        *empty = None;
        for next in remaining {
            *empty = next.take();
            empty = next;
        }
    }
}

impl Default for StaPmksaCache {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests;
