//! Convergence retained by a completed RX DC-calibration product.

/// Results of all searches in one completed common RX DC calibration.
///
/// False means the bounded outer search exhausted its iteration limit.
/// Baseband searches then publish their initial coefficient pair; fine
/// radio searches publish their last correction, following the vendor policy.
/// This is measurement quality, not a transport error or RF admission decision.
/// An absent RX calibration is represented by the parent's `Option`.
/// The compact private representation stays within the existing SRAM budget.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct PhyRxGainDcQuality {
    converged: u32,
}

impl PhyRxGainDcQuality {
    pub(crate) const EMPTY: Self = Self { converged: 0 };
    const ALL: u32 = (1 << (19 + super::FINE_CODES)) - 1;
    const BASEBAND: u32 = (1 << 19) - 1;
    fn record(&mut self, index: u8, converged: bool) {
        let mask = 1 << index;
        self.converged = (self.converged & !mask) | if converged { mask } else { 0 };
    }
    pub(crate) fn record_shared(&mut self, index: u8, converged: bool) {
        debug_assert!(index < 11);
        self.record(index, converged);
    }
    pub(crate) fn record_wifi_baseband(&mut self, index: u8, converged: bool) {
        debug_assert!(index < 8);
        self.record(11 + index, converged);
    }
    pub(crate) fn record_wifi_fine(&mut self, index: u8, converged: bool) {
        debug_assert!(usize::from(index) < super::FINE_CODES);
        self.record(19 + index, converged);
    }
    /// Shared-radio baseband results, indexed by gain 0 through 10.
    pub fn shared_baseband(self) -> [bool; 11] {
        core::array::from_fn(|index| self.converged & (1 << index) != 0)
    }
    /// Wi-Fi baseband results, indexed by gain 0 through 7.
    pub fn wifi_baseband(self) -> [bool; 8] {
        core::array::from_fn(|index| self.converged & (1 << (11 + index)) != 0)
    }
    /// Results of the fine radio searches after Wi-Fi baseband gain zero,
    /// in calibration order.
    pub fn wifi_fine(self) -> [bool; super::FINE_CODES] {
        core::array::from_fn(|index| self.converged & (1 << (19 + index)) != 0)
    }
    /// Whether every search converged before its iteration limit.
    pub const fn all_converged(self) -> bool {
        self.converged == Self::ALL
    }
    /// Number of searches that exhausted the outer iteration limit.
    pub const fn iteration_limited(self) -> u32 {
        (Self::ALL & !self.converged).count_ones()
    }
    /// Baseband searches whose initial pair was used after exhaustion.
    /// Fine-code exhaustion does not restore the initial pair.
    pub const fn initial_pairs_reused(self) -> u32 {
        (Self::BASEBAND & !self.converged).count_ones()
    }
}
