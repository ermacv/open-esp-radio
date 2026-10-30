//! Hardware EDCA and negotiated HT/HE publication parameters.

use oer_ieee80211_contracts_port::{EdcaParameterSet, VifRole};
use oer_ieee80211_wire::qos::WmmAccessCategory;

use crate::mac::{
    edca::{EdcaAccessPolicy, EdcaContentionParameters, EdcaParametersError, EdcaQueues},
    tx::{HeEdcaTxopLimit, HtPeerAmpduParameters, LegacyTxQueue},
};

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
pub struct WifiTxRuntimePolicy {
    ht_ampdu: HtPeerAmpduParameters,
    he_bss_color: u8,
    /// Parameter tables by VIF; contention belongs to the four AC queues.
    edca: [EdcaParameterSet; 2],
    queues: EdcaQueues,
    heads: [Option<VifRole>; 4],
}

impl WifiTxRuntimePolicy {
    /// Standard non-AP EDCA defaults.
    pub const fn vendor_defaults() -> Self {
        Self {
            ht_ampdu: HtPeerAmpduParameters::from_capability_byte(0),
            he_bss_color: 0,
            edca: [EdcaParameterSet::STANDARD_DEFAULT; 2],
            queues: EdcaQueues::standard_defaults(),
            heads: [None; 4],
        }
    }

    const fn edca(&self, role: VifRole) -> &EdcaParameterSet {
        let [station, access_point] = &self.edca;
        match role {
            VifRole::Station => station,
            VifRole::AccessPoint => access_point,
        }
    }
    /// The diagnostic composition observes the installed table without a copy.
    #[cfg(oer_debug_control)]
    pub const fn debug_edca(&self, role: VifRole) -> &EdcaParameterSet {
        self.edca(role)
    }

    const fn edca_mut(&mut self, role: VifRole) -> &mut EdcaParameterSet {
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

    /// Atomically validate and install all four WMM access categories of
    /// the interface of `role`; the other interface keeps its own.
    pub fn install_edca(
        &mut self,
        role: VifRole,
        parameters: EdcaParameterSet,
    ) -> Result<(), EdcaParametersError> {
        for category in WmmAccessCategory::ALL {
            EdcaAccessPolicy::from_edca(parameters.access_category(category))?;
        }
        *self.edca_mut(role) = parameters;
        Ok(())
    }

    pub fn contention_parameters(
        &self,
        role: VifRole,
        queue: LegacyTxQueue,
    ) -> EdcaContentionParameters {
        self.access_policy(role, queue).contention()
    }

    pub const fn access_policy(&self, role: VifRole, queue: LegacyTxQueue) -> EdcaAccessPolicy {
        match EdcaAccessPolicy::from_edca(self.edca(role).access_category(queue.access_category()))
        {
            Ok(policy) => policy,
            Err(_) => panic!("installed EDCA was validated"),
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

    #[cfg(any(test, feature = "test-support", oer_debug_control))]
    pub fn contention_exponent(&self, queue: LegacyTxQueue) -> u8 {
        self.queues.queue(queue).current_exponent()
    }

    /// Select one hardware backoff slot from platform-provided entropy.
    pub fn select_backoff(&mut self, role: VifRole, queue: LegacyTxQueue, entropy: u32) -> u16 {
        self.select_head(role, queue);
        self.queues.select_slot(queue, entropy)
    }

    fn select_head(&mut self, role: VifRole, queue: LegacyTxQueue) {
        self.queues
            .select_policy(queue, self.access_policy(role, queue));
        if self.heads[queue as usize] != Some(role) {
            self.queues.reset_terminal_exchange(queue);
            self.heads[queue as usize] = Some(role);
        }
    }

    pub fn record_retry_failure(&mut self, role: VifRole, queue: LegacyTxQueue) {
        self.select_head(role, queue);
        self.queues.record_retry_failure(queue);
    }

    pub fn record_success(&mut self, _role: VifRole, queue: LegacyTxQueue) {
        self.queues.record_success(queue);
        self.heads[queue as usize] = None;
    }

    pub fn reset_terminal_exchange(&mut self, _role: VifRole, queue: LegacyTxQueue) {
        self.queues.reset_terminal_exchange(queue);
        self.heads[queue as usize] = None;
    }
}

impl Default for WifiTxRuntimePolicy {
    fn default() -> Self {
        Self::vendor_defaults()
    }
}
