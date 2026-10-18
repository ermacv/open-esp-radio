//! Hardware authority required by a finite access-point transaction.
//! This contract borrows the existing key, receive-policy and TSF owner; it
//! neither acquires a radio nor creates a second resource lifecycle.

use oer_esp32s31_ieee80211_mac::{
    ap_policy::ApRxPolicyHardware, ap_tsf::ApTsfHardware, crypto::CcmpKeyHardware,
};

pub trait ApRuntimeHardware: CcmpKeyHardware + ApRxPolicyHardware + ApTsfHardware {}

impl<T> ApRuntimeHardware for T where T: CcmpKeyHardware + ApRxPolicyHardware + ApTsfHardware {}
