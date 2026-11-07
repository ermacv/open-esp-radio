//! Hardware capacity and public RX BlockAck classification.
//! Software sequence reordering belongs to the upper MAC.

use oer_protocols_ieee80211_wire::sequence::SequenceNumber;

use crate::mac::rx::PUBLIC_HEADER_SIZE;

// SOURCE: BLOB_LIBNET80211_RX_BLOCK_ACK. The vendor keeps three values apart:
// - Hardware entry: `libnet80211[ieee80211_ht.o]::ht_recv_action_ba_addba_request`
//   loads the constant 64 at +0x356 and passes it as the sixth argument of
//   `ic_add_rx_ba` at +0x36e..+0x372, whatever window was negotiated; complete
//   `libpp[hal_ampdu.o]::hal_agreement_add_rx_ba` publishes it in the seven-bit
//   hardware window. This backend instead programs the negotiated window.
// - Negotiated window: the same function (+0x154..+0x172) and
//   `ampdu_rx_start.constprop.0` take min(request, `g_wifi_menuconfig+0x30`),
//   or the configured limit when the request is 0.
// - Configured limit: `libnet80211[ieee80211_ioctl.o]::wifi_menuconfig_init`
//   copies `wifi_init_config_t` offset 112 (`rx_ba_win`) to
//   `g_wifi_menuconfig+0x30` and refuses values outside 2..64 when AMPDU RX is
//   enabled; the library holds no default. Each build chooses it: ESP-IDF
//   4d80c1b2 `components/esp_wifi/Kconfig:199-205` (`ESP_WIFI_RX_BA_WIN`)
//   defaults to 6, or 16 when Wi-Fi memory goes to PSRAM first; the S31 iperf
//   example sets 64 (`examples/wifi/iperf/sdkconfig.defaults.esp32s31:11`).
// This constant is the largest window the hardware entry is known to run
// with: the vendor's installed 64. Keep the protocol/reorder window
// independent of the number of RX DMA descriptors: descriptors are recycled
// while this sequence window remains active.
pub const RX_BLOCK_ACK_MAX_WINDOW: u16 = 64;
use oer_protocols_ieee80211_wire::{
    frame::{FrameControlFields, FrameType},
    trigger::BarControlFields,
};

/// Receive BlockAck session and window start named by one BlockAckReq.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct RxBlockAckRequestKey {
    pub peer: [u8; 6],
    pub tid: u8,
    pub starting_sequence: SequenceNumber,
}

const BLOCK_ACK_REQUEST_LENGTH: usize = 20;

/// Extract the receive session a BlockAckReq addresses.
///
/// Only the TID and the Starting Sequence Control are read. The BAR Control
/// policy bits (BAR Ack Policy, Compressed, Multi-TID) are not validated,
/// and a TID above 7 is ignored.
///
/// SOURCE: `libnet80211.a::sta_recv_ctl`/`hostap_recv_ctl` dispatch control
/// subtype 8 to `ieee80211_recv_bar`, and `ieee80211_process_bar_info` reads
/// only `bar_control >> 12` and `ssc >> 4` (vendor-library RX BlockAckReq
/// answer).
pub fn rx_block_ack_request_key(
    raw: &[u8],
    local_address: [u8; 6],
    expected_peer: Option<[u8; 6]>,
) -> Option<RxBlockAckRequestKey> {
    let frame = raw.get(PUBLIC_HEADER_SIZE..PUBLIC_HEADER_SIZE + BLOCK_ACK_REQUEST_LENGTH)?;
    let frame_control = u16::from_le_bytes([frame[0], frame[1]]);
    if FrameControlFields::from_raw(frame_control).kind() != FrameType::Control
        || FrameControlFields::from_raw(frame_control).subtype() != 8
        || frame[4..10] != local_address
    {
        return None;
    }
    let peer: [u8; 6] = frame[10..16].try_into().ok()?;
    if expected_peer.is_some_and(|expected| expected != peer) {
        return None;
    }
    let control = u16::from_le_bytes([frame[16], frame[17]]);
    let tid = BarControlFields::from_bytes(control.to_le_bytes())
        .tid()
        .get();
    if tid > 7 {
        return None;
    }
    Some(RxBlockAckRequestKey {
        peer,
        tid,
        starting_sequence: SequenceNumber::from_sequence_control(u16::from_le_bytes([
            frame[18], frame[19],
        ])),
    })
}

pub const RX_BLOCK_ACK_BANK_COUNT: usize = 8;

#[cfg(test)]
mod tests;
