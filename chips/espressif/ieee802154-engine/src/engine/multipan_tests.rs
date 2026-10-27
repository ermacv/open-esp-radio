//! Multi-PAN behavior over the HAL register model, read from the pinned
//! `update_mpf_index`, `ieee802154_ack_config_pending_bit` and
//! `esp_ieee802154_multipan.c`.

use crate::ll::model::Ieee802154LlModel;
use crate::ll::model::ModelAddress;
use crate::{tx_power::*, types::*};
use std::{boxed::Box, vec, vec::Vec};

use crate::pib::{AutoPendingMode, Ieee802154MultipanIndex, Ieee802154PibDefaults};
use oer_ieee802154_mac::FrameAddress;

use super::{
    FRAME_SIZE, Ieee802154Engine, Ieee802154EngineBuffers, Ieee802154EnhancedAck,
    Ieee802154Environment, Ieee802154FrameInfo, Ieee802154Interfaces, Ieee802154ReceivedAck,
    Ieee802154RxSlot, Ieee802154SecurityFrameError, Ieee802154State, Ieee802154TxError,
};

static LEVELS: [i8; 1] = [0];

const IF0: Ieee802154MultipanIndex = Ieee802154MultipanIndex::CONTEXT0;
const IF1: Ieee802154MultipanIndex = Ieee802154MultipanIndex::CONTEXT1;

#[derive(Default)]
struct Received(Vec<Ieee802154FrameInfo>);

impl Ieee802154Environment for Received {
    fn enhanced_ack_security_failed(&mut self, _error: Ieee802154SecurityFrameError) {}

    fn coexistence_break(&mut self) {}

    fn now_micros(&mut self) -> u64 {
        0
    }
    fn receive_done(
        &mut self,
        _slot: Ieee802154RxSlot,
        _frame: &[u8; FRAME_SIZE],
        info: &Ieee802154FrameInfo,
    ) {
        self.0.push(*info);
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

/// A 2006 data frame requesting an ACK from short source 0x5678 to short
/// `destination` in PAN `panid`, as the receive DMA writes it.
fn data_to(panid: u16, destination: [u8; 2]) -> Vec<u8> {
    let [pan0, pan1] = panid.to_le_bytes();
    vec![
        12,
        0x61,
        0x98,
        0x01,
        pan0,
        pan1,
        destination[0],
        destination[1],
        0x78,
        0x56,
        0xaa,
        0xc4,
        0xc8,
    ]
}

struct Bench {
    engine: Ieee802154Engine<'static, ModelAddress>,
    hw: Ieee802154LlModel,
    env: Received,
}

impl Bench {
    fn new(interfaces: Option<u8>) -> Self {
        let buffers = Box::leak(Box::new(Ieee802154EngineBuffers::new()));
        let levels = Ieee802154TxPowerLevels::new(&LEVELS).unwrap();
        let defaults = Ieee802154PibDefaults::default();
        let mut engine = match interfaces {
            Some(count) => Ieee802154Engine::new_multipan(
                buffers,
                crate::ll::model::addresses(),
                levels,
                defaults,
                Ieee802154Interfaces::new(count).unwrap(),
                crate::pib::test_permit(11, levels),
            ),
            None => Ieee802154Engine::new(
                buffers,
                crate::ll::model::addresses(),
                levels,
                defaults,
                crate::pib::test_permit(11, levels),
            ),
        };
        let mut hw = Ieee802154LlModel::default();
        engine.enable();
        engine.mac_init(&mut hw);
        engine.update_pib(&mut hw, &mut Received::default(), |pib| {
            pib.set_promiscuous(false)
        });
        Self {
            engine,
            hw,
            env: Received::default(),
        }
    }

    /// Interface 0 owns PAN 0x1234 / short 0x0001, interface 1 owns PAN
    /// 0xabcd / short 0x1111.
    fn with_identities(interfaces: Option<u8>) -> Self {
        let mut bench = Self::new(interfaces);
        let (engine, hw) = (&mut bench.engine, &mut bench.hw);
        engine.set_multipan_panid(hw, IF0, 0x1234);
        engine.set_multipan_short_address(hw, IF0, 0x0001);
        if interfaces.is_some_and(|count| count > 1) {
            engine.set_multipan_panid(hw, IF1, 0xabcd);
            engine.set_multipan_short_address(hw, IF1, 0x1111);
        }
        bench
    }

    fn receive(&mut self, image: &[u8]) -> Ieee802154FrameInfo {
        self.engine.update_pib(&mut self.hw, &mut self.env, |pib| {
            pib.set_rx_when_idle(true)
        });
        self.engine.receive(&mut self.hw, &mut self.env);
        let address = self.hw.rx_address.unwrap();
        assert!(self.engine.model_dma_write(address, image));
        self.hw.raise(&[Ieee802154Event::RxDone]);
        self.engine
            .isr(&mut self.hw, &mut self.env)
            .expect("handled events");
        self.hw.raise(&[Ieee802154Event::AckTxDone]);
        self.engine
            .isr(&mut self.hw, &mut self.env)
            .expect("handled events");
        self.env.0.pop().expect("the frame was delivered")
    }
}

#[test]
fn a_frame_is_assigned_to_the_interface_it_addresses() {
    let mut bench = Bench::with_identities(Some(2));
    assert_eq!(
        bench.receive(&data_to(0xabcd, [0x11, 0x11])).mpf_index,
        Some(IF1)
    );
    assert_eq!(
        bench.receive(&data_to(0x1234, [0x01, 0x00])).mpf_index,
        Some(IF0)
    );
    assert_eq!(
        bench.receive(&data_to(0xabcd, [0x01, 0x00])).mpf_index,
        None
    );
}

#[test]
fn broadcast_frames_belong_to_no_interface() {
    let mut bench = Bench::with_identities(Some(2));
    assert_eq!(
        bench.receive(&data_to(0xabcd, [0xff, 0xff])).mpf_index,
        None
    );
    assert_eq!(
        bench.receive(&data_to(0xffff, [0x11, 0x11])).mpf_index,
        None
    );
}

/// The pending decision uses the addressed interface's mode and table, and
/// an unmatched frame gets none, leaving the hardware pending bit alone.
#[test]
fn the_pending_bit_follows_the_addressed_interface() {
    let mut bench = Bench::with_identities(Some(2));
    bench
        .engine
        .update_pib(&mut bench.hw, &mut bench.env, |pib| {
            pib.set_pending_mode(IF1, AutoPendingMode::Enable)
        });
    bench
        .engine
        .pending_table_for(IF1)
        .add(FrameAddress::Short([0x78, 0x56]))
        .unwrap();
    bench
        .engine
        .update_pib(&mut bench.hw, &mut bench.env, |pib| {
            pib.set_pending_mode(IF0, AutoPendingMode::Enable)
        });
    assert!(bench.receive(&data_to(0xabcd, [0x11, 0x11])).pending);
    assert!(bench.hw.pending_bit);
    assert!(!bench.receive(&data_to(0x1234, [0x01, 0x00])).pending);
    assert!(!bench.hw.pending_bit);

    bench.hw.pending_bit = true;
    assert!(!bench.receive(&data_to(0x9999, [0x11, 0x11])).pending);
    assert!(
        bench.hw.pending_bit,
        "an unmatched frame leaves the bit alone"
    );
}

#[test]
fn without_multipan_every_frame_stays_on_interface_zero() {
    let mut bench = Bench::with_identities(None);
    assert_eq!(
        bench.receive(&data_to(0x9999, [0x22, 0x22])).mpf_index,
        Some(IF0)
    );
}

#[test]
fn interfaces_sleep_individually_and_the_last_sleeps_the_radio() {
    let mut bench = Bench::new(Some(2));
    bench.hw.multipan_enable = Ieee802154MultipanEnableState::NONE;
    let (engine, hw, env) = (&mut bench.engine, &mut bench.hw, &mut bench.env);
    engine.multipan_receive(hw, env, IF0);
    engine.multipan_receive(hw, env, IF1);
    assert_eq!(
        hw.multipan_enable,
        Ieee802154MultipanEnableState::NONE.with(IF0).with(IF1)
    );
    assert_eq!(engine.state(), Ieee802154State::Rx);
    engine.multipan_sleep(hw, env, IF0);
    assert_eq!(engine.state(), Ieee802154State::Rx);
    engine.multipan_sleep(hw, env, IF1);
    assert_eq!(hw.multipan_enable, Ieee802154MultipanEnableState::NONE);
    assert_eq!(engine.state(), Ieee802154State::Sleep);
}

#[test]
fn receive_when_idle_holds_while_any_interface_asks() {
    let mut bench = Bench::new(Some(2));
    bench.engine.multipan_set_rx_when_idle(IF0, true);
    bench.engine.multipan_set_rx_when_idle(IF1, true);
    bench.engine.multipan_set_rx_when_idle(IF0, false);
    assert!(bench.engine.pib().rx_when_idle());
    bench.engine.multipan_set_rx_when_idle(IF1, false);
    assert!(!bench.engine.pib().rx_when_idle());
}

#[test]
#[should_panic(expected = "IEEE802154_ASSERT")]
fn an_interface_beyond_the_configuration_is_rejected() {
    let mut bench = Bench::new(Some(2));
    bench
        .engine
        .set_multipan_panid(&mut bench.hw, Ieee802154MultipanIndex::CONTEXT2, 1);
}

/// Every backend-owned setting is restored even if the MAC register model
/// loses all its fields between epochs. Source match remains engine RAM.
#[test]
fn settings_survive_disable_enable_and_complete_register_loss() {
    let mut bench = Bench::with_identities(Some(2));
    let (engine, hw, env) = (&mut bench.engine, &mut bench.hw, &mut bench.env);
    engine.set_multipan_extended_address(hw, IF0, [1; 8]);
    engine.set_multipan_extended_address(hw, IF1, [2; 8]);
    engine.set_multipan_enable(hw, Ieee802154MultipanEnableState::NONE.with(IF1));
    engine.set_ack_timeout(
        hw,
        crate::types::AckTimeout::new(200).expect("bounded timeout"),
    );
    let priorities = crate::engine::tests::vendor_priorities();
    engine.set_coexistence(crate::coex::Ieee802154Coexistence::Software(priorities));
    let permit = crate::pib::test_permit(20, engine.levels);
    engine.update_pib(hw, env, |pib| {
        pib.set_channel(permit);
        pib.set_power(-3);
        pib.set_cca_mode(Ieee802154CcaMode::Carrier);
        pib.set_cca_threshold(-90);
        pib.set_coordinator(true);
        pib.set_auto_ack_tx(false);
        pib.set_auto_ack_rx(true);
        pib.set_enhanced_ack_tx(false);
        pib.set_pending_mode(IF1, AutoPendingMode::Enhanced);
    });
    engine.multipan_set_rx_when_idle(IF1, true);
    engine
        .pending_table_for(IF1)
        .add(FrameAddress::Short([0x78, 0x56]))
        .expect("room");
    engine.receive(hw, env);
    let saved = hw.clone();
    let saved_pib = engine.pib().clone();
    engine.mac_deinit(hw);
    engine.disable();
    *hw = Ieee802154LlModel::default();
    engine.enable();
    engine.mac_init(hw);
    assert_eq!(engine.pib(), &saved_pib);
    assert_eq!(hw.panid, saved.panid);
    assert_eq!(hw.short_address, saved.short_address);
    assert_eq!(hw.extended_address, saved.extended_address);
    assert_eq!(hw.multipan_enable, saved.multipan_enable);
    assert_eq!(hw.ack_timeout, saved.ack_timeout);
    assert_eq!(hw.frequency_code, saved.frequency_code);
    assert_eq!(hw.tx_power_code, saved.tx_power_code);
    assert_eq!(hw.cca_mode, saved.cca_mode);
    assert_eq!(hw.cca_threshold_dbm, -90);
    assert_eq!(hw.coordinator, saved.coordinator);
    assert_eq!(hw.promiscuous, saved.promiscuous);
    assert_eq!(
        (hw.tx_auto_ack, hw.rx_auto_ack, hw.tx_enhanced_ack),
        (false, true, false)
    );
    assert_eq!(hw.pending_mode, saved.pending_mode);
    assert_eq!(
        hw.ack_pti,
        priorities
            .operation(crate::coex::RadioOperation::Ack)
            .value()
    );
    assert_eq!(
        hw.txrx_pti,
        priorities
            .operation(crate::coex::RadioOperation::Idle)
            .value()
    );
    engine.multipan_set_rx_when_idle(IF1, false);
    assert!(!engine.pib().rx_when_idle());
    assert!(
        engine
            .pending_table_for(IF1)
            .contains(FrameAddress::Short([0x78, 0x56]))
    );
}

#[test]
#[should_panic(expected = "IEEE802154_ASSERT")]
fn pending_mode_checks_the_configured_interface_count() {
    let mut bench = Bench::new(Some(2));
    bench.engine.set_pending_mode(
        &mut bench.hw,
        &mut bench.env,
        Ieee802154MultipanIndex::CONTEXT2,
        AutoPendingMode::Enable,
    );
}

#[test]
fn malformed_destination_never_matches_an_interface() {
    let mut bench = Bench::with_identities(Some(2));
    let mut image = data_to(0xabcd, [0x11, 0x11]);
    image[2] = 0x94; // reserved destination mode, otherwise matching bytes.
    assert_eq!(bench.receive(&image).mpf_index, None);
    let mut image = data_to(0xabcd, [0x11, 0x11]);
    image[0] = 7; // destination extends into the footer despite allocated bytes.
    assert_eq!(bench.receive(&image).mpf_index, None);
}
