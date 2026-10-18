use embassy_sync::blocking_mutex::raw::NoopRawMutex;

use super::*;

#[test]
fn marker_and_public_dimensions_cannot_drift() {
    assert_eq!(
        DefaultWifiResourceProfile::RX_DESCRIPTOR_COUNT,
        ESP32S31_DEFAULT_RX_DESCRIPTOR_COUNT
    );
    assert_eq!(
        DefaultWifiResourceProfile::TX_AMPDU_FRAME_COUNT,
        ESP32S31_DEFAULT_TX_AMPDU_FRAME_COUNT
    );
    assert_eq!(DefaultWifiResourceProfile::RX_REORDER_WINDOW, 16);
    assert_eq!(DefaultWifiResourceProfile::RX_STAGE_SLOT_COUNT, 32);
    assert_eq!(DefaultWifiResourceProfile::RX_DESCRIPTOR_COUNT, 96);
    assert_eq!(ESP32S31_DEFAULT_NETWORK_TX_QUEUE_DEPTH, 67);
    assert_eq!(ESP32S31_DEFAULT_NETWORK_OWNER_TX_QUEUE_DEPTH, 128);
    const {
        assert!(
            ESP32S31_DEFAULT_NETWORK_PACKET_POOL_CAPACITY
                > ESP32S31_DEFAULT_NETWORK_OWNER_TX_QUEUE_DEPTH
        );
        assert!(
            ESP32S31_DEFAULT_NETWORK_RX_PACKET_POOL_CAPACITY
                > ESP32S31_DEFAULT_NETWORK_RX_QUEUE_DEPTH
        );
    }
    assert_eq!(
        ESP32S31_DEFAULT_NETWORK_TX_QUEUE_DEPTH
            - ESP32S31_PERMANENT_NETWORK_ENDPOINTS
            - ESP32S31_NETWORK_TX_PIPELINE_CREDITS,
        2 * ESP32S31_DEFAULT_TX_AMPDU_FRAME_COUNT,
        "two aggregate arenas remain visible while Core1 owns one unpublished TX token"
    );
}

#[test]
fn default_station_memory_is_acquired_as_one_owner_graph() {
    static MEMORY: DefaultWifiMemory<NoopRawMutex> = DefaultWifiMemory::new();

    static SCAN: DefaultScanMemory = DefaultScanMemory::new();

    let lease = MEMORY
        .claim(&SCAN)
        .expect("fresh station memory is available");
    assert_eq!(
        lease.rx_dma.buffers().len(),
        ESP32S31_DEFAULT_RX_DESCRIPTOR_COUNT
    );
    assert_eq!(lease.scan_frame.len(), ESP32S31_DEFAULT_SCAN_FRAME_CAPACITY);
    assert_eq!(lease.ap_beacon.len(), AP_BEACON_CAPACITY);
    assert!(matches!(
        MEMORY.claim(&SCAN),
        Err(DefaultWifiMemoryError::InUse)
    ));
}

#[test]
fn conflicting_scan_claim_does_not_consume_the_radio_arena() {
    static FIRST: DefaultWifiMemory<NoopRawMutex> = DefaultWifiMemory::new();
    static SECOND: DefaultWifiMemory<NoopRawMutex> = DefaultWifiMemory::new();
    static BUSY: DefaultScanMemory = DefaultScanMemory::new();
    static FRESH: DefaultScanMemory = DefaultScanMemory::new();

    let first = FIRST.claim(&BUSY).unwrap();
    first.scan_frame[0] = 0x71;
    assert!(matches!(
        SECOND.claim(&BUSY),
        Err(DefaultWifiMemoryError::InUse)
    ));
    let second = SECOND
        .claim(&FRESH)
        .expect("failed pair must release its radio reservation");
    second.scan_frame[0] = 0x35;
    assert_eq!(first.scan_frame[0], 0x71);
    assert_eq!(second.scan_frame[0], 0x35);
}

#[test]
fn conflicting_radio_claim_does_not_consume_the_scan_arena() {
    static FIRST: DefaultWifiMemory<NoopRawMutex> = DefaultWifiMemory::new();
    static SECOND: DefaultWifiMemory<NoopRawMutex> = DefaultWifiMemory::new();
    static BUSY: DefaultScanMemory = DefaultScanMemory::new();
    static FRESH: DefaultScanMemory = DefaultScanMemory::new();

    let _first = FIRST.claim(&BUSY).unwrap();
    assert!(matches!(
        FIRST.claim(&FRESH),
        Err(DefaultWifiMemoryError::InUse)
    ));
    assert!(SECOND.claim(&FRESH).is_ok());
}
