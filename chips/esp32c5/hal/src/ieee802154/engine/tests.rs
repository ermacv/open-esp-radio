use super::*;
use oer_espressif_ieee802154_engine::{
    engine::{
        FRAME_SIZE, Ieee802154EnhancedAck, Ieee802154Environment, Ieee802154FrameInfo,
        Ieee802154ReceivedAck, Ieee802154RxSlot, Ieee802154TxError,
    },
    ll::model::Ieee802154LlModel,
};
use oer_espressif_ieee802154_mac_values::{Ieee802154Channel, Ieee802154RxStatus};
use std::boxed::Box;

static LEVELS: [i8; 1] = [0];

#[derive(Default)]
struct Environment;
impl Ieee802154Environment for Environment {
    fn now_micros(&mut self) -> u64 {
        0
    }
    fn receive_done(&mut self, _: Ieee802154RxSlot, _: &[u8; FRAME_SIZE], _: &Ieee802154FrameInfo) {
    }
    fn receive_sfd_done(&mut self) {}
    fn transmit_done(&mut self, _: &[u8; FRAME_SIZE], _: Option<Ieee802154ReceivedAck<'_>>) {}
    fn transmit_failed(&mut self, _: &[u8; FRAME_SIZE], _: Ieee802154TxError) {}
    fn transmit_sfd_done(&mut self, _: &mut [u8; FRAME_SIZE]) {}
    fn energy_detect_done(&mut self, _: i8) {}
    fn cca_done(&mut self, _: bool) {}
    fn ed_failed(&mut self, _: Ieee802154RxStatus) {}
    fn receive_at_done(&mut self) {}
    fn generate_enhanced_ack(
        &mut self,
        _: &[u8; FRAME_SIZE],
        _: &Ieee802154FrameInfo,
        _: &mut [u8; FRAME_SIZE],
    ) -> Ieee802154EnhancedAck {
        Ieee802154EnhancedAck::Refused
    }
}

#[allow(
    clippy::unwrap_used,
    reason = "the fixed host fixture has a valid channel and power"
)]
fn model() -> (
    Ieee802154Engine<Ieee802154LlModel<DmaWindowAddress>>,
    u32,
    u32,
) {
    let buffers = Box::leak(Box::new(Ieee802154EngineBuffers::new()));
    let rx = core::ptr::from_ref(&buffers.rx_frames()[0]).addr() as u32;
    let tx = core::ptr::from_ref(buffers.tx_frame()).addr() as u32;
    let levels = Ieee802154TxPowerLevels::new(&LEVELS).unwrap();
    let permitted = oer_radio_regulatory::TEST_DOMAIN
        .permit(Ieee802154Channel::new(11).unwrap())
        .unwrap();
    let ceiling = permitted.tx_ceiling(oer_radio_regulatory::PowerLimits {
        antenna_gain_mbm: 0,
        chip_maximum_mbm: i32::MAX,
        owner_cap_mbm: None,
    });
    let start = Ieee802154Permit::new(permitted, ceiling, levels).unwrap();
    let engine = Ieee802154Engine::place(
        Ieee802154LlModel::default(),
        Placed::host(buffers),
        levels,
        Ieee802154PibDefaults::default(),
        start,
    );
    (engine, rx, tx)
}

#[test]
fn placed_owner_publishes_its_own_frames_and_keeps_other_arenas_out() {
    let (mut engine, expected_rx, expected_tx) = model();
    let (other, _, _) = model();
    let other_address = other.model_transmit_address();
    let tx = engine.model_transmit_address();
    assert_ne!(tx, other_address);
    assert_eq!(tx.get(), expected_tx);
    assert!(!engine.model_dma_write(other_address, &[7]));
    assert!(engine.model_dma_read(other_address).is_none());
    assert!(engine.model_rx_slot(other_address).is_none());
    engine.enable();
    engine.mac_init();
    let mut environment = Environment;
    engine.receive(&mut environment);
    let rx = engine.model_ll().rx_address.unwrap();
    assert_eq!(rx.get(), expected_rx);
    assert!(engine.model_rx_slot(rx).is_some());
    assert!(engine.model_dma_write(rx, &[3, 2, 1]));
    assert_eq!(engine.model_dma_read(rx).unwrap()[..3], [3, 2, 1]);
    engine
        .transmit(&mut environment, &[3, 1, 2, 3], false)
        .unwrap();
    assert_eq!(engine.model_ll().tx_address, Some(tx));
    assert_eq!(engine.model_dma_read(tx).unwrap()[..4], [3, 1, 2, 3]);
    // Moving the owner retains both the placed storage and the MAC backend.
    let mut moved = engine;
    assert_eq!(moved.model_ll().tx_address, Some(tx));
    assert_eq!(moved.model_dma_read(tx).unwrap()[..4], [3, 1, 2, 3]);
}

#[test]
fn the_hal_buffer_claim_is_affine() {
    let first = claim_buffers().expect("this test claims the HAL static once");
    assert!(claim_buffers().is_none());
    let _claimed = first.into_inner();
    assert!(claim_buffers().is_none());
}
