//! The single owner of the access-point TSF writes.
//!
//! The MAC's access-point timer counts the interface's TSF. It is written
//! only by restarting it from zero and by stopping it,
//! and every such write goes through [`AccessPointTsf`], which starts a new
//! generation of its [`TsfRelation`]: a TSF value or sample of the timer
//! before the write no longer describes it. The hardware write takes an
//! [`AccessPointTsfWrite`] that only this module constructs, so a backend
//! implements [`ApTsfHardware`] but no caller writes the timer around the
//! owner.
//!
//! [`AccessPointTsf::read`] snapshots the AP timer through the exclusive
//! Wi-Fi register owner. That same owner serializes STA/AP snapshot selection
//! and TSF writes; the shared readback registers cannot be borrowed separately.
//! A snapshot neither changes this owner's generation nor proves hardware
//! insertion of a beacon/probe-response timestamp on air.

use oer_esp32s31_hal::{ieee80211::mac::WifiMacHal, owner::RadioRuntimeOwner};
use oer_ieee80211_lower_mac::{TsfGeneration, TsfRelation};
use oer_ieee80211_mac::tsf::TsfInstant;

/// The uncertainty of an access-point TSF sample: a count of the timer's
/// microseconds.
pub const ACCESS_POINT_TSF_SAMPLE_UNCERTAINTY: oer_ieee80211_mac::tsf::TsfDuration =
    oer_ieee80211_mac::tsf::TsfDuration::from_micros(1);

/// The capability to write the access-point TSF, which only
/// [`AccessPointTsf`] holds.
#[derive(Debug)]
pub struct AccessPointTsfWrite {
    _owner: (),
}

/// The access-point timer's lifecycle edges.
pub trait ApTsfHardware {
    /// Reset the timer to zero and start it; only [`AccessPointTsf`] can
    /// call it.
    fn reset_and_start_access_point_tsf(&mut self, write: AccessPointTsfWrite);
    /// Stop the timer; only [`AccessPointTsf`] can call it.
    fn stop_access_point_tsf(&mut self, write: AccessPointTsfWrite);
}

/// Access to a coherent AP snapshot, separate from timer lifecycle writes.
///
/// Implementations complete select/read/release under one exclusive hardware
/// owner shared with every other TSF domain. They expose neither a selector
/// guard nor a callback that could re-enter another snapshot. The production
/// implementations borrow the same `WifiRadioRegisters` owner as STA reads.
pub trait ApTsfSnapshotHardware {
    fn access_point_tsf(&mut self) -> u64;
}

impl ApTsfSnapshotHardware for WifiMacHal<'_> {
    fn access_point_tsf(&mut self) -> u64 {
        WifiMacHal::access_point_tsf(self)
    }
}

impl ApTsfSnapshotHardware for RadioRuntimeOwner {
    fn access_point_tsf(&mut self) -> u64 {
        self.wifi_mac_hal().access_point_tsf()
    }
}

impl ApTsfHardware for WifiMacHal<'_> {
    fn reset_and_start_access_point_tsf(&mut self, _: AccessPointTsfWrite) {
        WifiMacHal::reset_and_start_access_point_tsf(self);
    }

    fn stop_access_point_tsf(&mut self, _: AccessPointTsfWrite) {
        WifiMacHal::stop_access_point_tsf(self);
    }
}

impl ApTsfHardware for RadioRuntimeOwner {
    fn reset_and_start_access_point_tsf(&mut self, _: AccessPointTsfWrite) {
        self.wifi_mac_hal().reset_and_start_access_point_tsf();
    }

    fn stop_access_point_tsf(&mut self, _: AccessPointTsfWrite) {
        self.wifi_mac_hal().stop_access_point_tsf();
    }
}

/// The owner of the access-point TSF writes. It lives as long as its user
/// (one access point of the role, one port core); its generations come from
/// an epoch its creator supplies, a number no other TSF relation owner took,
/// so a later owner's generations never repeat an earlier one's.
#[derive(Debug, Eq, PartialEq)]
pub struct AccessPointTsf {
    relation: TsfRelation,
}

impl AccessPointTsf {
    /// An owner in `epoch`, a number no other owner took.
    pub const fn new(epoch: u32) -> Self {
        Self {
            relation: TsfRelation::new(epoch, ACCESS_POINT_TSF_SAMPLE_UNCERTAINTY),
        }
    }

    /// The generation of the access-point TSF's relation.
    pub const fn generation(&self) -> TsfGeneration {
        self.relation.generation()
    }

    /// Read the AP timer without changing its relation or generation.
    ///
    /// This finite synchronous transaction holds the shared TSF register
    /// owner until the selector is released. It has no suspension or
    /// selected-state value that cancellation or `forget` could retain.
    pub fn read<H: ApTsfSnapshotHardware>(&self, hardware: &mut H) -> TsfInstant {
        TsfInstant::from_micros(hardware.access_point_tsf())
    }

    /// Reset the timer to zero and start it: a new generation, as an
    /// access point's `set_tsf`.
    pub fn restart<H: ApTsfHardware>(&mut self, hardware: &mut H) {
        self.relation.break_relation();
        hardware.reset_and_start_access_point_tsf(AccessPointTsfWrite { _owner: () });
    }

    /// Stop the timer before relinquishing or changing the access-point
    /// role: a new generation, since the stopped timer no longer counts.
    ///
    /// The HAL owns the finite PAC transaction recovered from
    /// `libpp.a[hal_tsf.o]::hal_disable_softap_tsf`.
    pub fn stop<H: ApTsfHardware>(&mut self, hardware: &mut H) {
        self.relation.break_relation();
        hardware.stop_access_point_tsf(AccessPointTsfWrite { _owner: () });
    }

    /// Start a new generation without a write: the access point follows
    /// another TSF (a channel change).
    pub fn break_relation(&mut self) {
        self.relation.break_relation();
    }
}

#[cfg(test)]
mod tests;
