//! A diagnostic write rejection; the rollback still programs the real table.
use oer_esp32s31_hal::types::{MacCcmpKeyIdentity, MacInterface, MacKeyInstallOutcome, MacSppMode};
use oer_esp32s31_ieee80211_mac::crypto::CcmpKeyHardware;

pub(super) struct RejectOnce<'a, H> {
    pub hardware: &'a mut H,
    pub reject: bool,
}
impl<H: CcmpKeyHardware> CcmpKeyHardware for RejectOnce<'_, H> {
    fn configure_ccmp_association(&mut self, interface: MacInterface, mode: MacSppMode) {
        self.hardware.configure_ccmp_association(interface, mode);
    }
    fn install_sta_ccmp_entry(
        &mut self,
        index: u8,
        identity: MacCcmpKeyIdentity,
        key: &[u8; 16],
    ) -> MacKeyInstallOutcome {
        if core::mem::replace(&mut self.reject, false) {
            MacKeyInstallOutcome::Rejected
        } else {
            self.hardware.install_sta_ccmp_entry(index, identity, key)
        }
    }
    fn clear_ccmp_entry(&mut self, index: u8) {
        self.hardware.clear_ccmp_entry(index);
    }
}
