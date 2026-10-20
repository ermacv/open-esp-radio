//! Hardware BSS, EDCA and WMM state with a statically selected transmit profile.
//!
//! The profile owns retry/protection algorithms above the backend. This adapter
//! retains queue contention and installed BSS facts; detached completions apply
//! the core's requested contention effects through the port.

use oer_ieee80211_port::VifRole;
use oer_ieee80211_wire::extensions::wmm::WmmParameterSet;
use oer_ieee80211_wire::qos::{
    WmmAccessCategory, WmmTrafficClass, WmmUserPriority, classify_ethernet_wmm,
};

use oer_ieee80211_port::backend::{BackendContention, BackendTxProfile};

use crate::{
    edca::{EdcaAccessPolicy, EdcaContentionParameters, EdcaParametersError, EdcaQueues},
    tx::protection::{BssProtection, RtsLengthThreshold, WifiTxProtectionPolicy},
    tx::{HeEdcaTxopLimit, HtPeerAmpduParameters, LegacyTxQueue},
};

/// Result of applying the peer's negotiated ACM policy to one classification.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum WmmAdmissionDisposition {
    /// The selected AC does not require an admission.
    AdmissionNotRequired,
    Downgraded {
        requested: WmmAccessCategory,
    },
    /// A non-QoS association owns only the legacy best-effort sequence space.
    NonQosBestEffort,
}

/// Complete station-side queue/TID policy selected for one network frame.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct WifiTxTraffic {
    pub requested: WmmTrafficClass,
    pub user_priority: WmmUserPriority,
    pub access_category: WmmAccessCategory,
    pub admission: WmmAdmissionDisposition,
    txop_limit_units_32_us: u16,
}

impl WifiTxTraffic {
    pub const fn queue(self) -> LegacyTxQueue {
        LegacyTxQueue::from_access_category(self.access_category)
    }

    pub const fn tid(self) -> u8 {
        self.user_priority.value()
    }

    pub const fn txop_limit_units_32_us(self) -> u16 {
        self.txop_limit_units_32_us
    }

    /// Resolve the peer's negotiated HE duration budget and an optional
    /// integration ceiling without widening either value.
    pub const fn he_txop_limit(
        self,
        configured_ceiling: HeEdcaTxopLimit,
    ) -> Result<HeEdcaTxopLimit, WmmTxopUnsupported> {
        he_txop_limit(self.txop_limit_units_32_us, configured_ceiling)
    }

    /// HT aggregation has no reviewed negotiated-TXOP duration calculator.
    pub const fn require_ht_txop_support(self) -> Result<(), WmmTxopUnsupported> {
        require_ht_txop_support(self.txop_limit_units_32_us)
    }
}

/// The HE duration budget of an access category whose advertised TXOP
/// limit is `units_32_us`, under an optional integration ceiling, widening
/// neither.
pub const fn he_txop_limit(
    units_32_us: u16,
    configured_ceiling: HeEdcaTxopLimit,
) -> Result<HeEdcaTxopLimit, WmmTxopUnsupported> {
    let Some(negotiated) = HeEdcaTxopLimit::from_units_32_us(units_32_us) else {
        return Err(WmmTxopUnsupported::AdvertisedLimitTooWide { units_32_us });
    };
    if negotiated.is_default() {
        return Ok(configured_ceiling);
    }
    if configured_ceiling.is_default()
        || negotiated.units_32_us() <= configured_ceiling.units_32_us()
    {
        Ok(negotiated)
    } else {
        Ok(configured_ceiling)
    }
}

/// An HT aggregate keeps no TXOP limit: it is sent only in an access
/// category without one.
pub const fn require_ht_txop_support(units_32_us: u16) -> Result<(), WmmTxopUnsupported> {
    if units_32_us == 0 {
        Ok(())
    } else {
        Err(WmmTxopUnsupported::HtAggregateDurationBudget { units_32_us })
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum WifiTxTrafficError {
    /// Every possible AC at or below the requested priority requires an
    /// admission, while no TSPEC/ADDTS owner exists in this driver.
    AdmissionControlRequired { requested: WmmAccessCategory },
}

/// Unreviewed boundary which must not be inferred from WMM parsing alone.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum WmmTxopUnsupported {
    AdvertisedLimitTooWide { units_32_us: u16 },
    HtAggregateDurationBudget { units_32_us: u16 },
    RtsCtsProtection,
    MultiPpduMediumOwnership,
}

/// Requests which would require an unproven hardware medium-ownership path.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum WmmHardwareMediumRequest {
    RtsCtsProtection,
    MultiPpduTxop,
}

/// Association-derived state used by all ordinary and aggregate TX paths.
///
/// The state is kept together so a caller cannot accidentally update a peer's
/// HT capability, BSS color and WMM contention policy through independent
/// ad-hoc fields. Entropy is supplied by the platform at the point where a
/// hardware queue is published.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct WifiTxRuntimePolicy<X: BackendTxProfile> {
    ht_ampdu: HtPeerAmpduParameters,
    he_bss_color: u8,
    /// Each interface's contention for the four queues: the station's,
    /// then the access point's.
    edca: [EdcaQueues; 2],
    protection: WifiTxProtectionPolicy<X>,
}

impl<X: BackendTxProfile> WifiTxRuntimePolicy<X> {
    /// Start with the same cold values as the complete vendor LMAC init.
    pub const fn vendor_defaults() -> Self {
        Self {
            ht_ampdu: HtPeerAmpduParameters::from_capability_byte(0),
            he_bss_color: 0,
            edca: [EdcaQueues::vendor_defaults::<X>(); 2],
            protection: WifiTxProtectionPolicy::new(Some(
                crate::tx::protection::RtsLengthThreshold::new(X::RTS_THRESHOLD_BYTES),
            )),
        }
    }

    const fn edca(&self, role: VifRole) -> &EdcaQueues {
        let [station, access_point] = &self.edca;
        match role {
            VifRole::Station => station,
            VifRole::AccessPoint => access_point,
        }
    }

    const fn edca_mut(&mut self, role: VifRole) -> &mut EdcaQueues {
        let [station, access_point] = &mut self.edca;
        match role {
            VifRole::Station => station,
            VifRole::AccessPoint => access_point,
        }
    }

    pub fn install_ht_ampdu(&mut self, parameters: HtPeerAmpduParameters) {
        self.ht_ampdu = parameters;
    }

    pub const fn ht_ampdu(&self) -> HtPeerAmpduParameters {
        self.ht_ampdu
    }

    /// Install the six-bit HE BSS color decoded from the peer's BSS Color IE.
    pub fn install_he_bss_color(&mut self, bss_color: u8) {
        self.he_bss_color =
            oer_ieee80211_wire::signal::BssColorCode::from_bytes([bss_color]).color();
    }

    pub const fn he_bss_color(&self) -> u8 {
        self.he_bss_color
    }

    /// Replace the BSS protection facts. Every later publication, including
    /// a retry of an active exchange, selects its protection from them.
    pub fn install_bss_protection(&mut self, bss: BssProtection) {
        self.protection.install_bss(bss);
    }

    /// Leave the BSS: only the local length threshold remains.
    pub fn clear_bss_protection(&mut self) {
        self.protection.clear_bss();
    }

    /// Replace the local dot11RTSThreshold; `None` disables the length rule.
    pub fn set_rts_length_threshold(&mut self, threshold: Option<RtsLengthThreshold>) {
        self.protection.set_rts_length_threshold(threshold);
    }

    pub const fn protection(&self) -> WifiTxProtectionPolicy<X> {
        self.protection
    }

    /// Atomically validate and install all four WMM access categories of
    /// the interface of `role`; the other interface keeps its own.
    pub fn install_wmm(
        &mut self,
        role: VifRole,
        parameters: WmmParameterSet,
    ) -> Result<(), EdcaParametersError> {
        self.edca_mut(role).configure_from_wmm(parameters)
    }

    pub fn contention_parameters(
        &self,
        role: VifRole,
        queue: LegacyTxQueue,
    ) -> EdcaContentionParameters {
        self.edca(role).queue(queue).parameters()
    }

    pub const fn access_policy(&self, role: VifRole, queue: LegacyTxQueue) -> EdcaAccessPolicy {
        self.edca(role).access_policy(queue)
    }

    /// Classify and admit one network frame under the active peer WMM policy.
    ///
    /// With no QoS peer, all markings collapse to legacy BE. With QoS, an ACM
    /// bit is never treated as an admission: the request walks to a lower AC
    /// and rewrites its TID to that AC's canonical value. If even BK requires
    /// admission, the request fails closed.
    pub fn select_network_traffic(
        &self,
        role: VifRole,
        ethernet: &[u8],
        peer_qos: bool,
    ) -> Result<WifiTxTraffic, WifiTxTrafficError> {
        let requested = classify_ethernet_wmm(ethernet);
        if !peer_qos {
            let selected = self.edca(role).access_policy(LegacyTxQueue::BestEffort);
            return Ok(WifiTxTraffic {
                requested,
                user_priority: WmmUserPriority::UP0,
                access_category: WmmAccessCategory::BestEffort,
                admission: WmmAdmissionDisposition::NonQosBestEffort,
                txop_limit_units_32_us: selected.txop_limit_units_32_us(),
            });
        }

        let mut category = requested.access_category;
        loop {
            let selected = self
                .edca(role)
                .access_policy(LegacyTxQueue::from_access_category(category));
            if !selected.admission_control_mandatory() {
                let downgraded = category != requested.access_category;
                return Ok(WifiTxTraffic {
                    requested,
                    user_priority: if downgraded {
                        category.canonical_user_priority()
                    } else {
                        requested.user_priority
                    },
                    access_category: category,
                    admission: if downgraded {
                        WmmAdmissionDisposition::Downgraded {
                            requested: requested.access_category,
                        }
                    } else {
                        WmmAdmissionDisposition::AdmissionNotRequired
                    },
                    txop_limit_units_32_us: selected.txop_limit_units_32_us(),
                });
            }
            let Some(lower) = category.downgrade() else {
                return Err(WifiTxTrafficError::AdmissionControlRequired {
                    requested: requested.access_category,
                });
            };
            category = lower;
        }
    }

    /// Explicit frontier for hardware operations not established by the
    /// recovered ordinary/A-MPDU publication contract.
    pub const fn request_hardware_medium(
        &self,
        request: WmmHardwareMediumRequest,
    ) -> Result<(), WmmTxopUnsupported> {
        match request {
            WmmHardwareMediumRequest::RtsCtsProtection => Err(WmmTxopUnsupported::RtsCtsProtection),
            WmmHardwareMediumRequest::MultiPpduTxop => {
                Err(WmmTxopUnsupported::MultiPpduMediumOwnership)
            }
        }
    }

    #[cfg(any(test, feature = "test-support", oer_probe))]
    pub fn contention_exponent(&self, role: VifRole, queue: LegacyTxQueue) -> u8 {
        self.edca(role).queue(queue).current_exponent()
    }

    /// Select one hardware backoff slot from platform-provided entropy.
    pub fn select_backoff(&self, role: VifRole, queue: LegacyTxQueue, entropy: u32) -> u16 {
        self.edca(role).select_slot(queue, entropy)
    }

    pub fn record_retry_failure(&mut self, role: VifRole, queue: LegacyTxQueue) {
        self.edca_mut(role).record_retry_failure(queue);
    }

    pub fn record_success(&mut self, role: VifRole, queue: LegacyTxQueue) {
        self.edca_mut(role).record_success(queue);
    }

    pub fn reset_terminal_exchange(&mut self, role: VifRole, queue: LegacyTxQueue) {
        self.edca_mut(role).reset_terminal_exchange(queue);
    }
}

impl<X: BackendTxProfile> Default for WifiTxRuntimePolicy<X> {
    fn default() -> Self {
        Self::vendor_defaults()
    }
}

impl<X: BackendTxProfile> BackendContention<LegacyTxQueue> for WifiTxRuntimePolicy<X> {
    fn record_success(&mut self, role: VifRole, queue: LegacyTxQueue) {
        self.record_success(role, queue)
    }
    fn reset_terminal_exchange(&mut self, role: VifRole, queue: LegacyTxQueue) {
        self.reset_terminal_exchange(role, queue)
    }
    fn record_retry_failure(&mut self, role: VifRole, queue: LegacyTxQueue) {
        self.record_retry_failure(role, queue)
    }
}
