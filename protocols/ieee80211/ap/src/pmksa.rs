//! The access point's PMKSA cache.
//!
//! An accepted SAE exchange leaves its PMK and PMKID here, keyed by the
//! station. An association that names the cached PMKID resumes that PMK
//! without a new exchange. The cache holds ten entries; adding to a full
//! cache drops the oldest, an entry for the same station replaces the
//! earlier one, and an entry does not expire, as the vendor's
//! `dot11RSNAConfigPMKLifetime` is `INT32_MAX`.
//!
//! SOURCE: ESP-IDF `7b9cc1ac79f865983f59bb8ff3ff43eb74ff1dbe`
//! `components/wpa_supplicant/src/ap/pmksa_cache_auth.c`
//! (`pmksa_cache_max_entries`, `dot11RSNAConfigPMKLifetime`,
//! `pmksa_cache_auth_add_entry`, `pmksa_cache_auth_get`) and
//! `src/ap/wpa_auth.c` (`wpa_auth_pmksa_add_sae`).

use oer_ieee80211_rsn::Pmk;

/// Entries the vendor's authenticator PMKSA cache holds.
pub const AP_PMKSA_CAPACITY: usize = 10;
/// The PMKID length.
pub const AP_PMKID_LEN: usize = 16;

/// One cached security association of a station.
pub struct ApPmksa {
    station: [u8; 6],
    pmk: Pmk,
    pmkid: [u8; AP_PMKID_LEN],
}

impl ApPmksa {
    pub const fn new(station: [u8; 6], pmk: Pmk, pmkid: [u8; AP_PMKID_LEN]) -> Self {
        Self {
            station,
            pmk,
            pmkid,
        }
    }

    pub const fn pmk(&self) -> &Pmk {
        &self.pmk
    }

    pub const fn pmkid(&self) -> [u8; AP_PMKID_LEN] {
        self.pmkid
    }
}

/// The cached security associations, oldest first.
pub struct ApPmksaCache {
    entries: [Option<ApPmksa>; AP_PMKSA_CAPACITY],
}

impl ApPmksaCache {
    pub const fn new() -> Self {
        Self {
            entries: [const { None }; AP_PMKSA_CAPACITY],
        }
    }

    /// The association `station` resumes by naming `pmkid`.
    pub fn find(&self, station: [u8; 6], pmkid: [u8; AP_PMKID_LEN]) -> Option<&ApPmksa> {
        self.entries
            .iter()
            .flatten()
            .find(|entry| entry.station == station && entry.pmkid == pmkid)
    }

    /// Cache `entry`, replacing the station's earlier entry; a full cache
    /// drops its oldest entry first.
    pub fn insert(&mut self, entry: ApPmksa) {
        self.remove(entry.station);
        if self.entries.iter().all(Option::is_some) {
            self.entries.rotate_left(1);
            self.entries[AP_PMKSA_CAPACITY - 1] = None;
        }
        let slot = self
            .entries
            .iter_mut()
            .find(|slot| slot.is_none())
            .expect("a slot is free after dropping the oldest entry");
        *slot = Some(entry);
    }

    /// Forget the association of `station`, keeping the others in order.
    pub fn remove(&mut self, station: [u8; 6]) {
        let Some(index) = self
            .entries
            .iter()
            .position(|entry| entry.as_ref().is_some_and(|entry| entry.station == station))
        else {
            return;
        };
        self.entries[index] = None;
        self.entries[index..].rotate_left(1);
    }
}

impl Default for ApPmksaCache {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests;
