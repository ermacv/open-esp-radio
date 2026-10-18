use embassy_sync::blocking_mutex::raw::NoopRawMutex;

use oer_esp32s31_ieee80211_dma::descriptor::{BIT_30, BIT_31, LENGTH_SHIFT};
use oer_ieee80211_mac::vif::StaApVif;

use super::*;

const STA: [u8; 6] = [0x02, 0, 0, 0, 0, 1];
const UPLINK: [u8; 6] = [0x02, 0, 0, 0, 0, 2];
const AP: [u8; 6] = [0x02, 0, 0, 0, 0, 3];
const PEER: [u8; 6] = [0x02, 0, 0, 0, 0, 4];
const ADDRESSES: StaApRxAddresses = StaApRxAddresses {
    station: STA,
    station_bssid: UPLINK,
    access_point: AP,
};

fn segment(
    storage: &mut [u8; 128],
    frame_control: u16,
    receiver: [u8; 6],
    transmitter: [u8; 6],
    third: [u8; 6],
) -> RxSegment<'_> {
    const MPDU_LENGTH: usize = 24;
    const SIGNAL_LENGTH: usize = MPDU_LENGTH + 4;
    const TAIL_OFFSET: usize = 0x38;
    const FRAME_OFFSET: usize = TAIL_OFFSET + 8;
    const RECEIVED: usize = FRAME_OFFSET + MPDU_LENGTH;
    storage[TAIL_OFFSET..TAIL_OFFSET + 4].copy_from_slice(
        &(((SIGNAL_LENGTH + 4) as u32) << 16 | SIGNAL_LENGTH as u32).to_le_bytes(),
    );
    storage[FRAME_OFFSET..FRAME_OFFSET + 2].copy_from_slice(&frame_control.to_le_bytes());
    storage[FRAME_OFFSET + 4..FRAME_OFFSET + 10].copy_from_slice(&receiver);
    storage[FRAME_OFFSET + 10..FRAME_OFFSET + 16].copy_from_slice(&transmitter);
    storage[FRAME_OFFSET + 16..FRAME_OFFSET + 22].copy_from_slice(&third);
    RxSegment {
        descriptor_address: 0x2f00_4000,
        descriptor_word0: 128 | ((RECEIVED as u32) << LENGTH_SHIFT) | BIT_30 | BIT_31,
        buffer: storage,
        next_descriptor_address: 0,
    }
}

const INGRESS: RxIngressConfig = RxIngressConfig {
    ring_entry_limit: 1,
    csi_config: 0,
    flags: 0,
};

#[test]
fn normalized_dma_boundary_routes_each_data_direction_without_role_inference() {
    let mut storage = [0; 128];
    let from_ds = segment(&mut storage, 0x0208, STA, UPLINK, PEER);
    assert_eq!(
        classify_sta_ap_segment(&from_ds, INGRESS, ADDRESSES),
        Ok(StaApRxRoute::Interface(StaApVif::Station))
    );

    let mut storage = [0; 128];
    let to_ds = segment(&mut storage, 0x0108, AP, PEER, STA);
    assert_eq!(
        classify_sta_ap_segment(&to_ds, INGRESS, ADDRESSES),
        Ok(StaApRxRoute::Interface(StaApVif::AccessPoint))
    );
}

#[test]
fn invalid_hardware_unit_fails_before_vif_classification() {
    let mut storage = [0; 128];
    let mut invalid = segment(&mut storage, 0x0108, AP, PEER, STA);
    invalid.descriptor_word0 &= !BIT_31;
    assert_eq!(
        classify_sta_ap_segment(&invalid, INGRESS, ADDRESSES),
        Err(RxError::Invalid)
    );
}

#[test]
fn paired_rx_queue_has_one_affine_endpoint_pair_per_drained_epoch() {
    let queue = StaApStagedRxQueue::<NoopRawMutex, 2, 128, 2>::new();
    let endpoints = queue.split();
    let duplicate = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| queue.split()));
    assert!(duplicate.is_err());
    drop(endpoints);
    let reused = queue.split();
    drop(reused);
}
