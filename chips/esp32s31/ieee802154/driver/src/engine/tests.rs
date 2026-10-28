use super::*;
use oer_espressif_ieee802154_engine::{
    engine::{
        Ieee802154EnhancedAck, Ieee802154Environment, Ieee802154FrameInfo, Ieee802154ReceivedAck,
        Ieee802154RxSlot, Ieee802154SecurityFrameError, Ieee802154TxError,
    },
    ll::model::Ieee802154LlModel,
};
use oer_espressif_ieee802154_mac_values::FRAME_SIZE;
use oer_espressif_ieee802154_mac_values::{Ieee802154Channel, Ieee802154Event, Ieee802154RxStatus};
use std::boxed::Box;

static LEVELS: [i8; 1] = [0];

#[derive(Default)]
struct Environment;
impl Ieee802154Environment for Environment {
    fn enhanced_ack_security_failed(&mut self, _error: Ieee802154SecurityFrameError) {}

    fn coexistence_break(&mut self) {}
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
    )
    .expect("placed frame geometry");
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
fn detach_rebind_reuses_placed_storage_across_mac_epochs() {
    let (mut engine, rx, tx) = model();
    let mut environment = Environment;
    engine.enable();
    engine.mac_init();
    engine.set_panid(0x4321);
    engine.receive(&mut environment);
    engine = match engine.detach() {
        Err(engine) => engine,
        Ok(_) => panic!("active RX cannot relinquish its placed owner"),
    };
    for _ in 0..3 {
        engine.sleep(&mut environment);
        engine.mac_deinit();
        let (detached, mac) = match engine.detach() {
            Ok(parts) => parts,
            Err(_) => panic!("quiescent MAC must detach"),
        };
        engine = detached.rebind(mac);
        engine.enable();
        engine.mac_init();
        assert_eq!(engine.panid(), 0x4321);
        engine.receive(&mut environment);
        assert_eq!(
            engine.model_ll().rx_address.map(DmaWindowAddress::get),
            Some(rx)
        );
        assert_eq!(engine.model_transmit_address().get(), tx);
    }
}

#[test]
fn service_quarantines_the_placed_owner_with_its_exact_fault() {
    let (mut engine, _, _) = model();
    engine.model_ll().events = Ieee802154Event::ClockCountMatch.mask();
    let before = std::format!("{:?}", engine.model_ll());
    let mut fault = match engine.isr(&mut Environment) {
        Ok(_) => panic!("unsupported event must quarantine the owner"),
        Err(quarantined) => quarantined,
    };
    assert_eq!(std::format!("{:?}", fault._owners.inner.model_ll()), before);
    assert_eq!(
        fault.fault(),
        oer_espressif_ieee802154_engine::engine::Fault::UnhandledEvents {
            observation:
                oer_espressif_ieee802154_mac_values::Ieee802154EventObservation::from_parts(
                    Ieee802154Event::ClockCountMatch.mask(),
                    false,
                    false,
                ),
            state: oer_espressif_ieee802154_engine::engine::Ieee802154State::Disable,
        }
    );
}

#[test]
fn dropping_quarantine_keeps_both_affine_owners_without_reuse() {
    use std::{cell::Cell, rc::Rc};
    struct Owner(Rc<Cell<usize>>);
    impl Drop for Owner {
        fn drop(&mut self) {
            self.0.set(self.0.get() + 1);
        }
    }
    let mac_drops = Rc::new(Cell::new(0));
    let dma_drops = Rc::new(Cell::new(0));
    let fault = Fault::UnhandledEvents {
        observation: oer_espressif_ieee802154_mac_values::Ieee802154EventObservation::from_parts(
            Ieee802154Event::RxDone.mask(),
            true,
            true,
        ),
        state: oer_espressif_ieee802154_engine::engine::Ieee802154State::Rx,
    };
    let quarantine = Quarantined::new((Owner(mac_drops.clone()), Owner(dma_drops.clone())), fault);
    assert_eq!(quarantine.fault(), fault);
    drop(quarantine);
    assert_eq!(mac_drops.get(), 0);
    assert_eq!(dma_drops.get(), 0);
}

#[test]
fn successful_service_returns_the_same_placed_owner_for_the_next_entry() {
    let (engine, _, tx) = model();
    let mut engine = engine
        .isr(&mut Environment)
        .expect("empty sample is handled");
    assert_eq!(engine.model_transmit_address().get(), tx);
    engine.model_ll().events = Ieee802154Event::ClockCountMatch.mask();
    let quarantine = match engine.isr(&mut Environment) {
        Ok(_) => panic!("next entry must quarantine"),
        Err(quarantine) => quarantine,
    };
    assert_eq!(quarantine._owners.model_transmit_address().get(), tx);
}

#[test]
fn a_fault_after_dispatch_retains_the_same_dma_owner_and_actual_progress() {
    use oer_espressif_ieee802154_engine::engine::{Ieee802154State, fault::ServiceError};
    let (mut engine, _, tx) = model();
    engine.enable();
    engine.mac_init();
    engine.update_pib(&mut Environment, |pib| pib.set_promiscuous(false));
    engine
        .transmit(
            &mut Environment,
            &[
                12, 0x61, 0x98, 1, 0x34, 0x12, 0x78, 0x56, 0x78, 0x56, 0xaa, 0, 0,
            ],
            false,
        )
        .unwrap();
    engine.model_ll().events = Ieee802154Event::TxDone.mask().with(Ieee802154Event::RxDone);
    let mut quarantined = match engine.isr(&mut Environment) {
        Ok(_) => panic!("incompatible RX_DONE must quarantine after TX_DONE"),
        Err(fault) => fault,
    };
    let Fault::Service {
        entry_state,
        state,
        reason,
        progress,
        ..
    } = quarantined.fault()
    else {
        panic!("service fault");
    };
    assert_eq!(entry_state, Ieee802154State::Tx);
    assert_eq!(state, Ieee802154State::RxAck);
    assert_eq!(reason, ServiceError::EventState(Ieee802154Event::RxDone));
    assert!(progress.acknowledged);
    assert_eq!(progress.completed, Ieee802154Event::TxDone.mask());
    assert_eq!(quarantined._owners.model_transmit_address().get(), tx);
    assert_eq!(
        quarantined
            ._owners
            .inner
            .model_ll()
            .tx_address
            .map(DmaWindowAddress::get),
        Some(tx)
    );
}
