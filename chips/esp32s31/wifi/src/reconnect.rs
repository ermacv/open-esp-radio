//! Facade policy values. No HIL state or diagnostic counters enter the product.
use oer_time::Duration;
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct ReconnectPolicy(oer_ieee80211_runtime::station::ReconnectPolicy);
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct ZeroReconnectInterval;
impl ReconnectPolicy {
    pub const DISABLED: Self = Self(oer_ieee80211_runtime::station::ReconnectPolicy::DISABLED);
    pub const fn every(interval: Duration) -> Result<Self, ZeroReconnectInterval> {
        match oer_ieee80211_runtime::station::ReconnectPolicy::every(interval) {
            Ok(policy) => Ok(Self(policy)),
            Err(_) => Err(ZeroReconnectInterval),
        }
    }
    #[cfg(target_arch = "riscv32")]
    pub(crate) fn runtime(self) -> oer_ieee80211_runtime::station::ReconnectPolicy {
        self.0
    }
}
