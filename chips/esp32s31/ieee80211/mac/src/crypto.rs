//! Typed ESP32-S31 hardware-key transactions.
//!
//! This is the allocation-free subset recovered from
//! `hal_crypto_set_key_entry` and `hal_crypto_enable`. It owns only MMIO
//! publication; WPA2 derivation and long-lived secret ownership remain in the
//! protocol crate.

use oer_esp32s31_hal::{
    ieee80211::mac::WifiMacHal,
    owner::RadioAccess,
    types::{MacCcmpKeyIdentity, MacKeyInstallOutcome},
};

use oer_ieee80211_wire::ccmp::GroupKeyId;

use subtle::ConstantTimeEq;

use zeroize::Zeroize;

const STA_PAIRWISE_HARDWARE_INDEX: u8 = 4;
const STA_GROUP_HARDWARE_INDEX: u8 = 1;
// The vendor AP pairwise pool begins at slot 8. Public AP AIDs 1..=15 map to
// slots 8..=22; the remaining physical entries are not claimed by this API.
// AP GTK key id one maps to `1 + key_id`, therefore slot 2.
pub const AP_PAIRWISE_HARDWARE_INDEX_BASE: u8 = 8;
pub const AP_PAIRWISE_SLOT_COUNT: u8 = 15;
const AP_GROUP_HARDWARE_INDEX_BASE: u8 = 1;
const CCMP_KEY_BYTES: usize = 16;

/// Finite hardware authority for the two open STA CCMP slots.
pub trait CcmpKeyHardware {
    /// Apply the immutable interface association decision before key publication.
    fn configure_ccmp_association(
        &mut self,
        interface: oer_esp32s31_hal::types::MacInterface,
        mode: oer_esp32s31_hal::types::MacSppMode,
    );
    fn install_sta_ccmp_entry(
        &mut self,
        index: u8,
        identity: MacCcmpKeyIdentity,
        temporal_key: &[u8; CCMP_KEY_BYTES],
    ) -> MacKeyInstallOutcome;
    fn install_ap_ccmp_entry(
        &mut self,
        index: u8,
        identity: MacCcmpKeyIdentity,
        temporal_key: &[u8; CCMP_KEY_BYTES],
    ) -> MacKeyInstallOutcome {
        self.install_sta_ccmp_entry(index, identity, temporal_key)
    }
    fn clear_ccmp_entry(&mut self, index: u8);

    /// Query one hardware entry when the implementation exposes validity.
    fn ccmp_entry_is_valid(&self, _index: u8) -> Option<bool> {
        None
    }
}

impl CcmpKeyHardware for WifiMacHal<'_> {
    fn configure_ccmp_association(
        &mut self,
        interface: oer_esp32s31_hal::types::MacInterface,
        mode: oer_esp32s31_hal::types::MacSppMode,
    ) {
        WifiMacHal::configure_ccmp_association(self, interface, mode);
    }
    fn install_sta_ccmp_entry(
        &mut self,
        index: u8,
        identity: MacCcmpKeyIdentity,
        temporal_key: &[u8; CCMP_KEY_BYTES],
    ) -> MacKeyInstallOutcome {
        self.install_station_ccmp_entry(index, identity, temporal_key)
    }

    fn install_ap_ccmp_entry(
        &mut self,
        index: u8,
        identity: MacCcmpKeyIdentity,
        temporal_key: &[u8; CCMP_KEY_BYTES],
    ) -> MacKeyInstallOutcome {
        self.install_access_point_ccmp_entry(index, identity, temporal_key)
    }

    fn clear_ccmp_entry(&mut self, index: u8) {
        WifiMacHal::clear_ccmp_entry(self, index);
    }

    fn ccmp_entry_is_valid(&self, index: u8) -> Option<bool> {
        WifiMacHal::ccmp_entry_is_valid(self, index)
    }
}

impl CcmpKeyHardware for RadioAccess {
    fn configure_ccmp_association(
        &mut self,
        interface: oer_esp32s31_hal::types::MacInterface,
        mode: oer_esp32s31_hal::types::MacSppMode,
    ) {
        self.wifi_mac_hal()
            .configure_ccmp_association(interface, mode);
    }
    fn install_sta_ccmp_entry(
        &mut self,
        index: u8,
        identity: MacCcmpKeyIdentity,
        temporal_key: &[u8; CCMP_KEY_BYTES],
    ) -> MacKeyInstallOutcome {
        CcmpKeyHardware::install_sta_ccmp_entry(
            &mut self.wifi_mac_hal(),
            index,
            identity,
            temporal_key,
        )
    }

    fn install_ap_ccmp_entry(
        &mut self,
        index: u8,
        identity: MacCcmpKeyIdentity,
        temporal_key: &[u8; CCMP_KEY_BYTES],
    ) -> MacKeyInstallOutcome {
        CcmpKeyHardware::install_ap_ccmp_entry(
            &mut self.wifi_mac_hal(),
            index,
            identity,
            temporal_key,
        )
    }

    fn clear_ccmp_entry(&mut self, index: u8) {
        CcmpKeyHardware::clear_ccmp_entry(&mut self.wifi_mac_hal(), index);
    }

    fn ccmp_entry_is_valid(&self, _index: u8) -> Option<bool> {
        None
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum CryptoKeyError {
    InvalidGroupKeyId,
    InvalidAccessPointAssociationId,
    Occupied,
    HardwareRejected,
}

/// Fail-closed result of replacing the one occupied STA group slot.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum StaGroupCcmpReplaceError {
    /// The backend refused replacement before the old entry was touched.
    InvalidReplacement(CryptoKeyError),
    /// The new publication failed and the exact old key was restored.
    ReplacementRolledBack(CryptoKeyError),
    /// Neither complete epoch is installed. The slot token is invalidated and
    /// group RX must remain quarantined until disconnect teardown.
    RollbackFailed {
        replacement: CryptoKeyError,
        rollback: CryptoKeyError,
    },
    /// The supplied old material does not authorize the occupied slot.
    CurrentMaterialMismatch,
}

/// Authority for the one STA pairwise CCMP slot installed by this module.
///
/// The token deliberately contains no key bytes. Consuming it through
/// [`StaPairwiseCcmpSlot::clear`] clears the hardware entry.
#[must_use = "the installed hardware key must remain owned until it is explicitly cleared"]
pub struct StaPairwiseCcmpSlot {
    peer: [u8; 6],
}

/// Authority for the one active STA group CCMP slot recovered from the
/// promoted production backend.
#[must_use = "the installed hardware key must remain owned until it is explicitly cleared"]
pub struct StaGroupCcmpSlot {
    key_id: GroupKeyId,
    installed: bool,
}

/// Zeroizing software rollback authority for the one installed STA GTK.
///
/// The hardware slot token intentionally contains no secret. Connected group
/// rekey must therefore retain this separate owner; without it a failed
/// replacement cannot truthfully restore the old hardware epoch.
#[must_use = "GTK material must remain owned until replacement or teardown"]
pub struct StaGroupCcmpKeyMaterial {
    key_id: GroupKeyId,
    temporal_key: [u8; CCMP_KEY_BYTES],
}

impl StaGroupCcmpKeyMaterial {
    pub fn new(key_id: GroupKeyId, temporal_key: [u8; CCMP_KEY_BYTES]) -> Self {
        Self {
            key_id,
            temporal_key,
        }
    }

    pub const fn key_id(&self) -> GroupKeyId {
        self.key_id
    }

    /// Compare key bytes without making a same-KeyID admission decision leak
    /// the first differing secret octet.
    pub fn same_temporal_key(&self, other: &Self) -> bool {
        bool::from(self.temporal_key.ct_eq(&other.temporal_key))
    }
}

impl Drop for StaGroupCcmpKeyMaterial {
    fn drop(&mut self) {
        self.temporal_key.zeroize();
    }
}

/// Authority for one AP peer pairwise CCMP entry.
#[must_use = "the installed hardware key must remain owned until it is explicitly cleared"]
pub struct ApPairwiseCcmpSlot {
    peer: [u8; 6],
    association_id: u16,
    hardware_index: u8,
}

/// A GTK wire ID supported by the S31 AP layout (1 through 3).
///
/// Zero belongs to the station's hardware group slot and cannot be selected
/// for an AP installation. Validate this restriction before calling hardware.
///
/// ```no_run
/// # #![cfg_attr(target_arch = "riscv32", no_std)]
/// # #![cfg_attr(target_arch = "riscv32", no_main)]
/// # #[cfg(target_arch = "riscv32")]
/// # #[panic_handler]
/// # fn panic(_: &core::panic::PanicInfo) -> ! {
/// #     loop {}
/// # }
/// use oer_esp32s31_ieee80211_mac::crypto::ApGroupKeyId;
/// assert!(ApGroupKeyId::new(0).is_none());
/// assert_eq!(ApGroupKeyId::new(1).map(ApGroupKeyId::get), Some(1));
/// assert_eq!(ApGroupKeyId::new(3).map(ApGroupKeyId::get), Some(3));
/// assert!(ApGroupKeyId::new(4).is_none());
/// ```
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct ApGroupKeyId(GroupKeyId);

impl ApGroupKeyId {
    pub const fn new(key_id: u8) -> Option<Self> {
        match GroupKeyId::new(key_id) {
            Some(id) if id.get() != 0 => Some(Self(id)),
            _ => None,
        }
    }

    pub const fn get(self) -> u8 {
        self.0.get()
    }

    pub const fn group_key_id(self) -> GroupKeyId {
        self.0
    }
}

/// Authority for the one group key exposed by an AP service epoch.
#[must_use = "the installed hardware key must remain owned until it is explicitly cleared"]
pub struct ApGroupCcmpSlot {
    key_id: ApGroupKeyId,
    hardware_index: u8,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct ApCcmpClearReport {
    pub pairwise_hardware_index: u8,
    pub group_hardware_index: u8,
}

/// Hardware indices cleared at the connected-to-disconnected boundary.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct StaCcmpClearReport {
    pub pairwise_hardware_index: u8,
    pub group_hardware_index: u8,
}

impl StaGroupCcmpSlot {
    pub const fn hardware_index(&self) -> u8 {
        STA_GROUP_HARDWARE_INDEX
    }

    pub const fn key_id(&self) -> GroupKeyId {
        self.key_id
    }

    pub fn clear<H: CcmpKeyHardware>(self, hardware: &mut H) {
        if self.installed {
            hardware.clear_ccmp_entry(STA_GROUP_HARDWARE_INDEX);
        }
    }
}

impl StaPairwiseCcmpSlot {
    pub const fn hardware_index(&self) -> u8 {
        STA_PAIRWISE_HARDWARE_INDEX
    }

    pub const fn peer(&self) -> &[u8; 6] {
        &self.peer
    }

    pub fn clear<H: CcmpKeyHardware>(self, hardware: &mut H) {
        hardware.clear_ccmp_entry(STA_PAIRWISE_HARDWARE_INDEX);
    }
}

impl ApPairwiseCcmpSlot {
    pub const fn hardware_index(&self) -> u8 {
        self.hardware_index
    }

    pub const fn peer(&self) -> &[u8; 6] {
        &self.peer
    }

    pub const fn association_id(&self) -> u16 {
        self.association_id
    }

    pub fn clear<H: CcmpKeyHardware>(self, hardware: &mut H) {
        hardware.clear_ccmp_entry(self.hardware_index);
    }
}

impl ApGroupCcmpSlot {
    pub const fn hardware_index(&self) -> u8 {
        self.hardware_index
    }

    pub const fn key_id(&self) -> ApGroupKeyId {
        self.key_id
    }

    pub fn clear<H: CcmpKeyHardware>(self, hardware: &mut H) {
        hardware.clear_ccmp_entry(self.hardware_index);
    }
}

pub fn clear_ap_ccmp_slots<H: CcmpKeyHardware>(
    hardware: &mut H,
    pairwise: ApPairwiseCcmpSlot,
    group: ApGroupCcmpSlot,
) -> ApCcmpClearReport {
    let report = ApCcmpClearReport {
        pairwise_hardware_index: pairwise.hardware_index(),
        group_hardware_index: group.hardware_index(),
    };
    group.clear(hardware);
    pairwise.clear(hardware);
    report
}

/// Consume and clear both association-scoped station key authorities.
///
/// Group is cleared before pairwise to match the recovered station teardown
/// order. The returned indices are observations only; they cannot authorize a
/// later clear or key use.
pub fn clear_sta_ccmp_slots<H: CcmpKeyHardware>(
    hardware: &mut H,
    pairwise: StaPairwiseCcmpSlot,
    group: StaGroupCcmpSlot,
) -> StaCcmpClearReport {
    let report = StaCcmpClearReport {
        pairwise_hardware_index: pairwise.hardware_index(),
        group_hardware_index: group.hardware_index(),
    };
    group.clear(hardware);
    pairwise.clear(hardware);
    report
}

/// Installs the WPA2 temporal key into the recovered STA pairwise slot.
///
/// The slot must be invalid on entry. This fail-closed rule prevents an open
/// driver instance from overwriting a key owned by retained vendor state or a
/// different interface. The key is borrowed only for the bounded MMIO
/// transaction and is never retained by the returned ownership token.
pub fn install_sta_pairwise_ccmp<H: CcmpKeyHardware>(
    hardware: &mut H,
    peer: [u8; 6],
    temporal_key: &[u8; CCMP_KEY_BYTES],
) -> Result<StaPairwiseCcmpSlot, CryptoKeyError> {
    match hardware.install_sta_ccmp_entry(
        STA_PAIRWISE_HARDWARE_INDEX,
        MacCcmpKeyIdentity::Pairwise { peer },
        temporal_key,
    ) {
        MacKeyInstallOutcome::Installed => {}
        MacKeyInstallOutcome::Occupied => return Err(CryptoKeyError::Occupied),
        MacKeyInstallOutcome::Rejected => return Err(CryptoKeyError::HardwareRejected),
    }
    Ok(StaPairwiseCcmpSlot { peer })
}

/// Installs the recovered STA GTK into hardware slot 1.
///
/// ESP32-S31 exposes one active station group slot regardless of the logical
/// RSN key ID. The logical ID is still encoded in the hardware control word,
/// exactly as in the reviewed promoted CCMP installation path.
///
/// ```compile_fail,E0308
/// # #![cfg_attr(target_arch = "riscv32", no_std)]
/// # #![cfg_attr(target_arch = "riscv32", no_main)]
/// # #[cfg(target_arch = "riscv32")]
/// # #[panic_handler]
/// # fn panic(_: &core::panic::PanicInfo) -> ! {
/// #     loop {}
/// # }
/// use oer_esp32s31_ieee80211_mac::crypto::{CcmpKeyHardware, install_sta_group_ccmp};
/// fn install<H: CcmpKeyHardware>(hardware: &mut H) {
///     let _ = install_sta_group_ccmp(hardware, 4_u8, &[0; 16]);
/// }
/// ```
///
/// ```no_run
/// # #![cfg_attr(target_arch = "riscv32", no_std)]
/// # #![cfg_attr(target_arch = "riscv32", no_main)]
/// # #[cfg(target_arch = "riscv32")]
/// # #[panic_handler]
/// # fn panic(_: &core::panic::PanicInfo) -> ! {
/// #     loop {}
/// # }
/// use oer_esp32s31_ieee80211_mac::crypto::{CcmpKeyHardware, install_sta_group_ccmp};
/// use oer_ieee80211_wire::ccmp::GroupKeyId;
/// fn install<H: CcmpKeyHardware>(hardware: &mut H, key_id: GroupKeyId) {
///     let _ = install_sta_group_ccmp(hardware, key_id, &[0; 16]);
/// }
/// ```
pub fn install_sta_group_ccmp<H: CcmpKeyHardware>(
    hardware: &mut H,
    key_id: GroupKeyId,
    temporal_key: &[u8; CCMP_KEY_BYTES],
) -> Result<StaGroupCcmpSlot, CryptoKeyError> {
    match hardware.install_sta_ccmp_entry(
        STA_GROUP_HARDWARE_INDEX,
        MacCcmpKeyIdentity::Group {
            key_id: key_id.get(),
        },
        temporal_key,
    ) {
        MacKeyInstallOutcome::Installed => {}
        MacKeyInstallOutcome::Occupied => return Err(CryptoKeyError::Occupied),
        MacKeyInstallOutcome::Rejected => return Err(CryptoKeyError::HardwareRejected),
    }
    Ok(StaGroupCcmpSlot {
        key_id,
        installed: true,
    })
}

/// Install one AP client's pairwise CCMP key into the AID-derived slot.
///
/// The slot and direction encoding are evidenced by a retained vendor AP
/// hardware run: AP peer identity one selected descriptor key byte `0x48`,
/// and `hal_crypto_set_key_entry` uses pairwise direction three for entries
/// above the four group-key slots. AID one owns slot 8 and AID fifteen owns
/// slot 22. The complete vendor path
/// `esp_wifi_set_ap_key_internal -> ic_set_key -> wDev_Insert_KeyEntry ->
/// hal_crypto_set_key_entry` passes connection context one and encodes it in
/// bits 8..=9 of the key-entry control halfword. The station path passes zero.
pub fn install_ap_pairwise_ccmp<H: CcmpKeyHardware>(
    hardware: &mut H,
    peer: [u8; 6],
    association_id: u16,
    temporal_key: &[u8; CCMP_KEY_BYTES],
) -> Result<ApPairwiseCcmpSlot, CryptoKeyError> {
    if association_id == 0 || association_id > u16::from(AP_PAIRWISE_SLOT_COUNT) {
        return Err(CryptoKeyError::InvalidAccessPointAssociationId);
    }
    let hardware_index = AP_PAIRWISE_HARDWARE_INDEX_BASE
        + u8::try_from(association_id - 1).expect("validated AP association id fits u8");
    match hardware.install_ap_ccmp_entry(
        hardware_index,
        MacCcmpKeyIdentity::Pairwise { peer },
        temporal_key,
    ) {
        MacKeyInstallOutcome::Installed => {}
        MacKeyInstallOutcome::Occupied => return Err(CryptoKeyError::Occupied),
        MacKeyInstallOutcome::Rejected => return Err(CryptoKeyError::HardwareRejected),
    }
    Ok(ApPairwiseCcmpSlot {
        peer,
        association_id,
        hardware_index,
    })
}

/// Install one AP GTK. Vendor AP group slots map directly to `1 + key_id`;
/// the first service provisions key id one and therefore owns slot 2. Key
/// id 0 would take entry 1, the station group key's: this S31 AP layout
/// refuses it. This is a hardware layout restriction; GTK wire ID zero is
/// valid for RSN and the station's dedicated group slot accepts it.
///
/// ```compile_fail,E0308
/// # #![cfg_attr(target_arch = "riscv32", no_std)]
/// # #![cfg_attr(target_arch = "riscv32", no_main)]
/// # #[cfg(target_arch = "riscv32")]
/// # #[panic_handler]
/// # fn panic(_: &core::panic::PanicInfo) -> ! {
/// #     loop {}
/// # }
/// use oer_esp32s31_ieee80211_mac::crypto::{CcmpKeyHardware, install_ap_group_ccmp};
/// use oer_ieee80211_wire::ccmp::GroupKeyId;
/// fn install<H: CcmpKeyHardware>(hardware: &mut H, key_id: GroupKeyId) {
///     let _ = install_ap_group_ccmp(hardware, key_id, &[0; 16]);
/// }
/// ```
///
/// ```no_run
/// # #![cfg_attr(target_arch = "riscv32", no_std)]
/// # #![cfg_attr(target_arch = "riscv32", no_main)]
/// # #[cfg(target_arch = "riscv32")]
/// # #[panic_handler]
/// # fn panic(_: &core::panic::PanicInfo) -> ! {
/// #     loop {}
/// # }
/// use oer_esp32s31_ieee80211_mac::crypto::{ApGroupKeyId, CcmpKeyHardware, install_ap_group_ccmp};
/// fn install<H: CcmpKeyHardware>(hardware: &mut H, key_id: ApGroupKeyId) {
///     let _ = install_ap_group_ccmp(hardware, key_id, &[0; 16]);
/// }
/// ```
pub fn install_ap_group_ccmp<H: CcmpKeyHardware>(
    hardware: &mut H,
    key_id: ApGroupKeyId,
    temporal_key: &[u8; CCMP_KEY_BYTES],
) -> Result<ApGroupCcmpSlot, CryptoKeyError> {
    let hardware_index = AP_GROUP_HARDWARE_INDEX_BASE + key_id.get();
    match hardware.install_ap_ccmp_entry(
        hardware_index,
        MacCcmpKeyIdentity::Group {
            key_id: key_id.get(),
        },
        temporal_key,
    ) {
        MacKeyInstallOutcome::Installed => {}
        MacKeyInstallOutcome::Occupied => return Err(CryptoKeyError::Occupied),
        MacKeyInstallOutcome::Rejected => return Err(CryptoKeyError::HardwareRejected),
    }
    Ok(ApGroupCcmpSlot {
        key_id,
        hardware_index,
    })
}

/// Replace the association-owned STA group key in its single hardware slot.
///
/// The existing token remains the unique authority for slot 1. Clearing is
/// performed before publication because the recovered hardware primitive
/// rejects an occupied entry. If publication fails, the token is marked
/// uninstalled so later teardown cannot claim or clear a key which hardware
/// never accepted; the association must then be terminated.
pub fn replace_sta_group_ccmp<H: CcmpKeyHardware>(
    hardware: &mut H,
    slot: &mut StaGroupCcmpSlot,
    key_id: GroupKeyId,
    temporal_key: &[u8; CCMP_KEY_BYTES],
) -> Result<(), CryptoKeyError> {
    if slot.installed {
        hardware.clear_ccmp_entry(STA_GROUP_HARDWARE_INDEX);
        slot.installed = false;
    }
    match install_sta_group_ccmp(hardware, key_id, temporal_key) {
        Ok(replacement) => {
            *slot = replacement;
            Ok(())
        }
        Err(error) => Err(error),
    }
}

/// Replace the occupied STA GTK and restore the exact old entry on failure.
///
/// The caller must keep group replay publication gated for the whole call.
/// A successful return changes only hardware; software replay/key-id rotation
/// is a separate affine commit. `ReplacementRolledBack` proves that the old
/// key is installed again and the old replay epoch may be un-gated.
pub fn replace_sta_group_ccmp_with_rollback<H: CcmpKeyHardware>(
    hardware: &mut H,
    slot: &mut StaGroupCcmpSlot,
    current: &StaGroupCcmpKeyMaterial,
    replacement: &StaGroupCcmpKeyMaterial,
) -> Result<(), StaGroupCcmpReplaceError> {
    if !slot.installed || slot.key_id != current.key_id {
        return Err(StaGroupCcmpReplaceError::CurrentMaterialMismatch);
    }

    hardware.clear_ccmp_entry(STA_GROUP_HARDWARE_INDEX);
    slot.installed = false;
    match install_sta_group_ccmp(hardware, replacement.key_id, &replacement.temporal_key) {
        Ok(installed) => {
            *slot = installed;
            Ok(())
        }
        Err(replacement_error) => {
            match install_sta_group_ccmp(hardware, current.key_id, &current.temporal_key) {
                Ok(restored) => {
                    *slot = restored;
                    Err(StaGroupCcmpReplaceError::ReplacementRolledBack(
                        replacement_error,
                    ))
                }
                Err(rollback) => Err(StaGroupCcmpReplaceError::RollbackFailed {
                    replacement: replacement_error,
                    rollback,
                }),
            }
        }
    }
}

#[cfg(test)]
mod tests;
