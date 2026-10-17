//! Multi-PAN behavior over the HAL register model, read from the pinned
//! `update_mpf_index`, `ieee802154_ack_config_pending_bit` and
//! `esp_ieee802154_multipan.c`.

use crate::ll::model::Ieee802154LlModel;
use crate::{tx_power::*, types::*};
use std::{boxed::Box, vec, vec::Vec};

use crate::pib::{AutoPendingMode, Ieee802154MultipanIndex, Ieee802154PibDefaults};
use oer_ieee802154::FrameAddress;

use super::{
    FRAME_SIZE, Ieee802154Engine, Ieee802154EngineBuffers, Ieee802154EnhancedAck,
    Ieee802154Environment, Ieee802154FrameInfo, Ieee802154Interfaces, Ieee802154ReceivedAck,
    Ieee802154RxSlot, Ieee802154State, Ieee802154TxError,
};

static LEVELS: [i8; 1] = [0];

const IF0: Ieee802154MultipanIndex = Ieee802154MultipanIndex::CONTEXT0;
const IF1: Ieee802154MultipanIndex = Ieee802154MultipanIndex::CONTEXT1;

#[derive(Default)]
struct Received(Vec<Ieee802154FrameInfo>);

impl Ieee802154Environment for Received {
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
    engine: Ieee802154Engine<'static>,
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
                levels,
                defaults,
                Ieee802154Interfaces::new(count).unwrap(),
            ),
            None => Ieee802154Engine::new(buffers, levels, defaults),
        };
        let mut hw = Ieee802154LlModel::default();
        engine.enable();
        engine.mac_init(&mut hw, defaults);
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
        self.engine.pib().set_rx_when_idle(true);
        self.engine.receive(&mut self.hw, &mut self.env);
        let address = self.hw.rx_address.unwrap();
        assert!(self.engine.model_dma_write(address, image));
        self.hw.raise(&[Ieee802154Event::RxDone]);
        self.engine.isr(&mut self.hw, &mut self.env);
        self.hw.raise(&[Ieee802154Event::AckTxDone]);
        self.engine.isr(&mut self.hw, &mut self.env);
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
        .pib()
        .set_pending_mode(IF1, AutoPendingMode::Enable);
    bench
        .engine
        .pending_table_for(IF1)
        .add(FrameAddress::Short([0x78, 0x56]))
        .unwrap();
    bench
        .engine
        .pib()
        .set_pending_mode(IF0, AutoPendingMode::Enable);
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
