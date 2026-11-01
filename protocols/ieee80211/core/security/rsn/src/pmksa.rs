//! The PMKSA cache of the supplicant and of the authenticator.
//!
//! An SAE authentication yields a PMK and its PMKID. The cache retains both
//! per peer: the station per access point and SSID, the access point per
//! station. A later association that names the cached PMKID resumes that PMK
//! without a new exchange. The cache holds ten entries; adding to a full
//! cache drops the oldest, an entry for the same peer replaces the earlier
//! one and becomes the newest, and an entry does not expire, as the vendor's
//! `dot11RSNAConfigPMKLifetime` is `INT32_MAX`. Only SAE associations are
//! cached: the vendor caches no PSK association
//! (`wpa_key_mgmt_supports_caching`).
//!
//! The owner keeps the cache by value and retains it across connections.
//! Access requires a short exclusive borrow; no shared mutex owner or
//! implicit synchronization sits in the protocol crate.
//!
//! SOURCE: ESP-IDF `7b9cc1ac79f865983f59bb8ff3ff43eb74ff1dbe`
//! `components/wpa_supplicant/src/rsn_supp/pmksa_cache.c`
//! (`pmksa_cache_max_entries`, `dot11RSNAConfigPMKLifetime`,
//! `pmksa_cache_add`), `src/rsn_supp/wpa.c` (`wpa_set_bss`, which uses an
//! entry only for the same BSSID, SSID and AKM and flushes it otherwise),
//! `src/ap/pmksa_cache_auth.c` (`pmksa_cache_max_entries`,
//! `dot11RSNAConfigPMKLifetime`, `pmksa_cache_auth_add_entry`,
//! `pmksa_cache_auth_get`) and `src/ap/wpa_auth.c` (`wpa_auth_pmksa_add_sae`).

use crate::Pmk;
use oer_ieee80211_wire::security::rsn::RSN_PMKID_LEN;

/// Entries the vendor's supplicant and authenticator PMKSA caches hold.
pub const PMKSA_CAPACITY: usize = 10;

/// One cached security association with the peer `P`. `C` is what else
/// selects it: the SSID for a station, nothing for an access point.
pub struct Pmksa<P, C = ()> {
    peer: P,
    context: C,
    pmk: Pmk,
    pmkid: [u8; RSN_PMKID_LEN],
}

impl<P, C> Pmksa<P, C> {
    /// Retain an authenticated SAE association.
    pub const fn new(peer: P, context: C, pmk: Pmk, pmkid: [u8; RSN_PMKID_LEN]) -> Self {
        Self {
            peer,
            context,
            pmk,
            pmkid,
        }
    }

    /// The SAE-derived pairwise master key borrowed for this association.
    pub const fn pmk(&self) -> &Pmk {
        &self.pmk
    }

    /// The identifier an association's RSN element names.
    pub const fn pmkid(&self) -> [u8; RSN_PMKID_LEN] {
        self.pmkid
    }
}

/// The cached security associations, oldest first.
pub struct PmksaCache<P, C = ()> {
    entries: [Option<Pmksa<P, C>>; PMKSA_CAPACITY],
}

impl<P: Copy + Eq, C> PmksaCache<P, C> {
    /// An empty cache, owned across successive connections.
    pub const fn new() -> Self {
        Self {
            entries: [const { None }; PMKSA_CAPACITY],
        }
    }

    /// The association `peer` resumes by naming `pmkid`.
    pub fn find(&self, peer: P, pmkid: [u8; RSN_PMKID_LEN]) -> Option<&Pmksa<P, C>> {
        self.entries
            .iter()
            .flatten()
            .find(|entry| entry.peer == peer && entry.pmkid == pmkid)
    }

    /// The association to resume with `peer` in `context`. An entry of the
    /// same peer in another context is flushed, as the vendor supplicant
    /// flushes a cached SAE association whose SSID changed.
    pub fn resume<Q: ?Sized>(&mut self, peer: P, context: &Q) -> Option<&Pmksa<P, C>>
    where
        C: PartialEq<Q>,
    {
        let mut remaining = self.entries.iter_mut();
        let slot = remaining.find(|slot| slot.as_ref().is_some_and(|entry| entry.peer == peer))?;
        if slot.as_ref().is_some_and(|entry| entry.context == *context) {
            return slot.as_ref();
        }
        Self::forget(slot, remaining);
        None
    }

    /// Cache `entry`, replacing the peer's earlier entry; a full cache drops
    /// its oldest entry first.
    pub fn insert(&mut self, entry: Pmksa<P, C>) {
        self.remove(entry.peer);
        if let Some(slot) = self.entries.iter_mut().find(|slot| slot.is_none()) {
            *slot = Some(entry);
            return;
        }
        // Array geometry proves there is an oldest entry. Move the others
        // through its hole and put the new association at the end.
        let [oldest, rest @ ..] = &mut self.entries;
        Self::forget(oldest, rest.iter_mut());
        let [.., newest] = &mut self.entries;
        *newest = Some(entry);
    }

    /// Forget the association with `peer`, keeping the others in order.
    pub fn remove(&mut self, peer: P) {
        let mut remaining = self.entries.iter_mut();
        if let Some(slot) =
            remaining.find(|slot| slot.as_ref().is_some_and(|entry| entry.peer == peer))
        {
            Self::forget(slot, remaining);
        }
    }

    /// Forget every association cached in `context`, keeping the others in
    /// order; their PMKs erase on drop.
    pub fn forget_context<Q: ?Sized>(&mut self, context: &Q)
    where
        C: PartialEq<Q>,
    {
        let entries = core::mem::replace(&mut self.entries, [const { None }; PMKSA_CAPACITY]);
        let mut places = self.entries.iter_mut();
        for entry in entries.into_iter().flatten() {
            if entry.context != *context
                && let Some(place) = places.next()
            {
                *place = Some(entry);
            }
        }
    }

    /// Empty `hole` and move every later entry one place towards it.
    fn forget<'a>(
        mut hole: &'a mut Option<Pmksa<P, C>>,
        remaining: impl Iterator<Item = &'a mut Option<Pmksa<P, C>>>,
    ) {
        *hole = None;
        for next in remaining {
            *hole = next.take();
            hole = next;
        }
    }
}

impl<P: Copy + Eq, C> Default for PmksaCache<P, C> {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests;
