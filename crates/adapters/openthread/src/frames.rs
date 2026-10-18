//! The translations between OpenThread's radio values and the portable
//! IEEE 802.15.4 contract.

use oer_ieee802154::{
    AppliedSecurity, AutoPendingMode, Configuration, FrameAddress, LinkMetrics, MAX_MAC_FRAME_LEN,
    ProbingInitiator, SentAcknowledgement, TxSecurity, TxStatus,
};

/// Bytes of the FCS OpenThread counts in every PSDU; the MAC appends it on
/// transmission and replaces it with RSSI and LQI on reception.
pub const FCS_SIZE: usize = 2;

/// The MAC bytes of a PSDU to transmit: OpenThread counts the FCS the MAC
/// appends. `None` for a PSDU without MAC bytes.
pub fn psdu_mac(psdu: &[u8]) -> Option<&[u8]> {
    psdu.len()
        .checked_sub(FCS_SIZE)
        .filter(|&length| length > 0)
        .map(|length| &psdu[..length])
}

/// Copy MAC bytes into `psdu` as OpenThread reads a received PSDU: the MAC
/// bytes and a zero FCS it does not check. Returns the PSDU length, or
/// `None` when the buffer is too short.
pub fn write_psdu(mac: &[u8], psdu: &mut [u8]) -> Option<usize> {
    let length = mac.len() + FCS_SIZE;
    let target = psdu.get_mut(..length)?;
    target[..mac.len()].copy_from_slice(mac);
    target[mac.len()..].fill(0);
    Some(length)
}

/// An extended address as OpenThread's glue passes it
/// (`u64::from_be_bytes(otExtAddress.m8)`) in over-the-air byte order.
pub const fn extended_address(address: u64) -> [u8; 8] {
    address.to_le_bytes()
}

/// A short source-match entry in the pending table, in over-the-air byte
/// order.
pub const fn short_pending_address(address: u16) -> FrameAddress {
    FrameAddress::Short(address.to_le_bytes())
}

/// An extended source-match entry in the pending table.
pub const fn extended_pending_address(address: u64) -> FrameAddress {
    FrameAddress::Extended(extended_address(address))
}

/// The automatic frame-pending mode of a source-match configuration:
/// disabled matching answers every poll with frame pending, enabled
/// matching looks the source up, in the enhanced mode ESP-IDF's OpenThread
/// port selects for Thread 1.2 and later (`otPlatRadioEnableSrcMatch`).
pub const fn pending_mode(enabled: bool) -> AutoPendingMode {
    if enabled {
        AutoPendingMode::Enhanced
    } else {
        AutoPendingMode::Disable
    }
}

/// Why a transmission failed, in the classes OpenThread's glue turns into
/// `otError`: channel-access failure, missing or invalid acknowledgement,
/// an invalid frame, or an abort.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum TransmitFailure {
    /// The channel was not acquired (`OT_ERROR_CHANNEL_ACCESS_FAILURE`).
    ChannelAccess,
    /// No acknowledgement arrived (`OT_ERROR_NO_ACK`).
    NoAcknowledgement,
    /// The acknowledgement was invalid (`OT_ERROR_NO_ACK`).
    InvalidAcknowledgement,
    /// The frame could not be sent.
    InvalidFrame,
    /// Anything else (`OT_ERROR_ABORT`).
    Other,
}

/// Classify a transmission's status as ESP-IDF's OpenThread port does: a
/// busy channel, an abort and a coexistence rejection are channel-access
/// failures. `None` for success.
pub const fn transmit_failure(status: TxStatus) -> Option<TransmitFailure> {
    match status {
        TxStatus::Success => None,
        TxStatus::ChannelBusy | TxStatus::Aborted | TxStatus::CoexistenceRejected => {
            Some(TransmitFailure::ChannelAccess)
        }
        TxStatus::NoAcknowledgement => Some(TransmitFailure::NoAcknowledgement),
        TxStatus::InvalidAcknowledgement => Some(TransmitFailure::InvalidAcknowledgement),
        TxStatus::InvalidFrame => Some(TransmitFailure::InvalidFrame),
        TxStatus::HardwareFailure | TxStatus::SecurityFailure => Some(TransmitFailure::Other),
    }
}

/// An energy scan's duration in microseconds.
pub const fn scan_micros(duration_millis: u16) -> u32 {
    duration_millis as u32 * 1_000
}

/// Who secures a frame, from its OpenThread transmit information: nobody
/// when the stack secured it (`mIsSecurityProcessed`), the radio under the
/// frame's own counter and key index for a retransmission (`mIsARetx`),
/// otherwise the radio with new ones.
pub const fn tx_security(retransmission: bool, security_processed: bool) -> TxSecurity {
    if security_processed {
        TxSecurity::Processed
    } else if retransmission {
        TxSecurity::Retransmission
    } else {
        TxSecurity::Radio
    }
}

/// Write the security header fields the radio assigned into the PSDU
/// OpenThread gave it, as `otMacFrameSetFrameCounter` and
/// `otMacFrameSetKeyId` write them into the port's frame. Returns `false`
/// when the PSDU has no auxiliary security header to write.
pub fn write_applied_security(applied: AppliedSecurity, psdu: &mut [u8]) -> bool {
    let length = psdu.len();
    if length > MAX_MAC_FRAME_LEN + FCS_SIZE {
        return false;
    }
    // The portable writer reads the frame as a `[PHR, PSDU...]` image.
    let mut image = [0; 1 + MAX_MAC_FRAME_LEN + FCS_SIZE];
    image[0] = length as u8;
    image[1..=length].copy_from_slice(psdu);
    if !applied.write(&mut image[..=length]) {
        return false;
    }
    psdu.copy_from_slice(&image[1..=length]);
    true
}

/// The frame counter and key index of the secured enhanced ACK a received
/// frame was sent (`mAckFrameCounter`, `mAckKeyId`).
pub fn sent_ack_security(sent: SentAcknowledgement) -> Option<(u32, u8)> {
    let security = sent.security?;
    Some((security.frame_counter, security.key_id?))
}

/// A time OpenThread gives in the low 32 bits of the radio clock
/// (`otPlatRadioReceiveAt`, `mTxDelayBaseTime + mTxDelay`), in the full
/// clock: the instant nearest to `now` with those low bits, up to half the
/// 32-bit range before or after it.
pub const fn radio_time(now: u64, low: u32) -> u64 {
    let offset = low.wrapping_sub(now as u32) as i32;
    now.wrapping_add_signed(offset as i64)
}

/// A CSL period as OpenThread passes it (`otPlatRadioEnableCsl`, units of
/// ten symbols) for the radio's 16-bit CSL IE field, as the port's
/// `otMacFrameSetCslIe` truncates it.
pub const fn csl_period(period: u32) -> u16 {
    period as u16
}

/// The frame-pending table changes that take the sources `old_short` and
/// `old_extended` to `new_short` and `new_extended`, as OpenThread's
/// `otPlatRadio*SrcMatch*` calls reach ESP-IDF's port one entry at a time:
/// removals first, then additions.
pub fn pending_changes(
    old_short: &[u16],
    new_short: &[u16],
    old_extended: &[u64],
    new_extended: &[u64],
    mut apply: impl FnMut(Configuration),
) {
    for &short in old_short.iter().filter(|short| !new_short.contains(short)) {
        apply(Configuration::RemovePendingAddress(short_pending_address(
            short,
        )));
    }
    for &extended in old_extended
        .iter()
        .filter(|extended| !new_extended.contains(extended))
    {
        apply(Configuration::RemovePendingAddress(
            extended_pending_address(extended),
        ));
    }
    for &short in new_short.iter().filter(|short| !old_short.contains(short)) {
        apply(Configuration::AddPendingAddress(short_pending_address(
            short,
        )));
    }
    for &extended in new_extended
        .iter()
        .filter(|extended| !old_extended.contains(extended))
    {
        apply(Configuration::AddPendingAddress(extended_pending_address(
            extended,
        )));
    }
}

/// The coexistence priority of immediate transmission and reception after
/// an OpenThread role change, as ESP-IDF's `handle_ot_role_change` sets it
/// with software coexistence; the other scene levels stay. The composition
/// maps it to its radio's coexistence levels.
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub enum RoleCoexPriority {
    /// The low level: the device keeps its receiver on when idle.
    Low,
    /// The middle level: a sleepy device.
    Middle,
}

/// The TX/RX coexistence priority of a device that keeps its receiver on
/// when idle, or not.
pub const fn role_txrx_priority(rx_on_when_idle: bool) -> RoleCoexPriority {
    if rx_on_when_idle {
        RoleCoexPriority::Low
    } else {
        RoleCoexPriority::Middle
    }
}

/// One enhanced-ACK probing initiator of OpenThread's table: its short
/// address, its extended address as `otExtAddress` holds it (most
/// significant byte first) and its metrics. The radio keeps the extended
/// address in frame byte order.
pub const fn probing_initiator(
    short_address: u16,
    extended_address: [u8; 8],
    metrics: LinkMetrics,
) -> ProbingInitiator {
    let mut frame_order = [0; 8];
    let mut index = 0;
    while index < 8 {
        frame_order[index] = extended_address[7 - index];
        index += 1;
    }
    ProbingInitiator {
        short_address,
        extended_address: frame_order,
        metrics,
    }
}

#[cfg(test)]
mod tests;
