//! ESP32-S31 HE20 peer installation at the protocol/PAC boundary.

use oer_esp32s31_hal::{
    ieee80211::mac::WifiMacHal,
    owner::RadioRuntimeOwner,
    types::{
        MacAssociationId, MacHe20PeerConfig, MacHe20PeerError, MacHeBssColor,
        MacHeDefaultPacketExtensionDuration, MacHePacketPaddingDuration,
        MacMinimumMpduStartSpacing,
    },
};

use oer_ieee80211_mac::he::{He20PeerState, HeElementError, parse_he20_peer_state};

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum He20InstallError {
    Element(HeElementError),
    Hardware(MacHe20PeerError),
    InvalidAssociationId(u16),
    InvalidMinimumMpduStartSpacing(u8),
    InvalidPeerState,
}

/// Finite PAC transactions required to install one parsed HE20 peer.
pub trait He20PeerHardware {
    fn program_he20_peer(
        &mut self,
        config: MacHe20PeerConfig,
        rts_threshold: Option<u16>,
    ) -> Result<(), MacHe20PeerError>;

    fn program_he20_association(
        &mut self,
        association_id: MacAssociationId,
        minimum_mpdu_start_spacing: MacMinimumMpduStartSpacing,
        bssid_index: u8,
    );

    fn initialize_he_buffer_status_report(&mut self);
}

impl He20PeerHardware for WifiMacHal<'_> {
    fn program_he20_peer(
        &mut self,
        config: MacHe20PeerConfig,
        rts_threshold: Option<u16>,
    ) -> Result<(), MacHe20PeerError> {
        WifiMacHal::program_he20_peer(self, config, rts_threshold)
    }

    fn program_he20_association(
        &mut self,
        association_id: MacAssociationId,
        minimum_mpdu_start_spacing: MacMinimumMpduStartSpacing,
        bssid_index: u8,
    ) {
        WifiMacHal::program_he20_association(
            self,
            association_id,
            minimum_mpdu_start_spacing,
            bssid_index,
        )
    }

    fn initialize_he_buffer_status_report(&mut self) {
        WifiMacHal::initialize_he_buffer_status_report(self);
    }
}

impl He20PeerHardware for RadioRuntimeOwner {
    fn program_he20_peer(
        &mut self,
        config: MacHe20PeerConfig,
        rts_threshold: Option<u16>,
    ) -> Result<(), MacHe20PeerError> {
        He20PeerHardware::program_he20_peer(&mut self.wifi_mac_hal(), config, rts_threshold)
    }

    fn program_he20_association(
        &mut self,
        association_id: MacAssociationId,
        minimum_mpdu_start_spacing: MacMinimumMpduStartSpacing,
        bssid_index: u8,
    ) {
        He20PeerHardware::program_he20_association(
            &mut self.wifi_mac_hal(),
            association_id,
            minimum_mpdu_start_spacing,
            bssid_index,
        )
    }

    fn initialize_he_buffer_status_report(&mut self) {
        He20PeerHardware::initialize_he_buffer_status_report(&mut self.wifi_mac_hal());
    }
}

/// Parse and install one associated HE20 peer without retaining vendor node
/// layout or exposing raw MMIO above the PAC.
///
/// SOURCE: complete pinned `libnet80211.a[ieee80211_he.o]`
/// capability/operation parsers and `libpp.a[hal_mac_ctl.o]`
/// hardware leaves. Earlier source history is not an oracle.
pub fn install_he20_peer<H: He20PeerHardware>(
    hardware: &mut H,
    capability: &[u8],
    operation: &[u8],
    association_id: u16,
    minimum_mpdu_start_spacing: u8,
    bssid_index: u8,
) -> Result<He20PeerState, He20InstallError> {
    let state = parse_he20_peer_state(capability, operation).map_err(He20InstallError::Element)?;
    program_he20_peer_state(
        hardware,
        state,
        association_id,
        minimum_mpdu_start_spacing,
        bssid_index,
    )?;
    Ok(state)
}

/// Install an already parsed HE20 peer plan.
///
/// Association orchestration parses one immutable peer view before touching
/// hardware. Reusing that value here guarantees that rate control, HE-SIG
/// color/ER-SU policy and the programmed S31 registers cannot be derived from
/// different parses of mutable application storage.
pub fn program_he20_peer_state<H: He20PeerHardware>(
    hardware: &mut H,
    state: He20PeerState,
    association_id: u16,
    minimum_mpdu_start_spacing: u8,
    bssid_index: u8,
) -> Result<(), He20InstallError> {
    let association_id = MacAssociationId::new(u32::from(association_id))
        .ok_or(He20InstallError::InvalidAssociationId(association_id))?;
    let minimum_mpdu_start_spacing =
        MacMinimumMpduStartSpacing::new(u32::from(minimum_mpdu_start_spacing)).ok_or(
            He20InstallError::InvalidMinimumMpduStartSpacing(minimum_mpdu_start_spacing),
        )?;
    let packet_padding_duration =
        MacHePacketPaddingDuration::new(u32::from(state.packet_padding_eight_us) * 8)
            .ok_or(He20InstallError::InvalidPeerState)?;
    let default_packet_extension_duration = MacHeDefaultPacketExtensionDuration::new(u32::from(
        state.default_packet_extension_duration,
    ))
    .ok_or(He20InstallError::InvalidPeerState)?;
    let bss_color =
        MacHeBssColor::new(u32::from(state.bss_color)).ok_or(He20InstallError::InvalidPeerState)?;

    hardware
        .program_he20_peer(
            MacHe20PeerConfig {
                packet_padding_duration,
                default_packet_extension_duration,
                bss_color,
                bss_color_enabled: state.bss_color_enabled,
                partial_bss_color: state.partial_bss_color,
                extended_range_single_user_disabled: state.extended_range_single_user_disabled,
            },
            state.rts_threshold,
        )
        .map_err(He20InstallError::Hardware)?;
    hardware.program_he20_association(association_id, minimum_mpdu_start_spacing, bssid_index);
    hardware.initialize_he_buffer_status_report();
    Ok(())
}

#[cfg(test)]
mod tests;
