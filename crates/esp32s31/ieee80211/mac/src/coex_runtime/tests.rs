use super::*;

#[derive(Default)]
struct Hardware {
    set: Option<(MacPti, MacPti)>,
    clears: u8,
    itwt: Option<(bool, MacPti)>,
    itwt_clear: Option<MacItwtClearIndex>,
    tx: Option<(MacTxQueueIndex, MacTxPtiProgram)>,
}

impl MacRuntimeCoexHardware for Hardware {
    fn publish_rx_beacon_pti(&mut self, beacon: MacPti, shared: MacPti) {
        self.set = Some((beacon, shared));
    }

    fn clear_rx_beacon_pti_request(&mut self) {
        self.clears += 1;
    }

    fn publish_itwt_pti(&mut self, argument_is_zero: bool, shared: MacPti) {
        self.itwt = Some((argument_is_zero, shared));
    }

    fn clear_itwt_pti_request(&mut self, index: MacItwtClearIndex) {
        self.itwt_clear = Some(index);
    }

    fn publish_tx_pti(&mut self, queue: MacTxQueueIndex, program: MacTxPtiProgram) {
        self.tx = Some((queue, program));
    }
}

#[test]
fn lmac_carries_only_bounded_pti_values() {
    let mut hardware = Hardware::default();
    let beacon = MacPti::new(7).unwrap();
    let shared = MacPti::new(5).unwrap();
    configure_rx_beacon_pti(&mut hardware, beacon, shared);
    clear_rx_beacon_pti(&mut hardware);
    assert_eq!(hardware.set, Some((beacon, shared)));
    assert_eq!(hardware.clears, 1);
    assert!(MacPti::new(16).is_none());
}

#[test]
fn lmac_carries_only_bounded_itwt_and_tx_arguments() {
    let mut hardware = Hardware::default();
    let shared = MacPti::new(5).unwrap();
    let clear = MacItwtClearIndex::new(31).unwrap();
    let queue = MacTxQueueIndex::new(3).unwrap();
    let program = MacTxPtiProgram {
        scheduler_priority: MacPti::new(1).unwrap(),
        pti_2: MacPti::new(2).unwrap(),
        pti_1: MacPti::new(3).unwrap(),
        pti_0: MacPti::new(4).unwrap(),
        pti_3: MacPti::new(5).unwrap(),
        count: oer_esp32s31_hal::ieee80211::mac::MacTxPtiCount::new(0x0fff).unwrap(),
    };
    configure_itwt_pti(&mut hardware, true, shared);
    clear_itwt_pti(&mut hardware, clear);
    configure_tx_pti(&mut hardware, queue, program);
    assert_eq!(hardware.itwt, Some((true, shared)));
    assert_eq!(hardware.itwt_clear, Some(clear));
    assert_eq!(hardware.tx, Some((queue, program)));
    assert!(MacItwtClearIndex::new(32).is_none());
    assert!(MacTxQueueIndex::new(4).is_none());
}
