//! Role behavior over the HAL register model: admission, event correlation,
//! frame lending and resume. The engine's vendor sequences are covered by
//! the engine tests and the host stand.

use std::{boxed::Box, vec, vec::Vec};

use oer_espressif_ieee802154_engine::engine::{
    Ieee802154Engine, Ieee802154EngineBuffers, Ieee802154Interfaces, Ieee802154State,
    PENDING_TABLE_SIZE,
};
use oer_espressif_ieee802154_engine::pib::Ieee802154PibDefaults;
use oer_espressif_ieee802154_engine::{
    ll::{Ieee802154LlCommand, model::Ieee802154LlModel},
    tx_power::Ieee802154TxPowerLevels,
    types::{
        Ieee802154CcaMode, Ieee802154Event, Ieee802154MultipanIndex, Ieee802154RxAbortReason,
        Ieee802154RxAbortReasonObservation, Ieee802154TxAbortReason,
        Ieee802154TxAbortReasonObservation,
    },
};
use oer_ieee802154::{
    AppliedSecurity, AutoPendingMode, CSL_IE_TEMPLATE, CancelError, CcaMode,
    CcaThresholdCapability, CcaThresholdError, Channel, CommandError, Configuration,
    EnergyScanRequest, FrameAddress, FramePending, FrameView, Ieee802154Instant, Interface,
    InterfaceSetting, MacKeys, PendingTableHalf, RadioCommand, RadioEvent, RadioState, RequestId,
    RestingState, ScheduledReceiveRequest, SentAcknowledgement, TransmitPowerError, TxMode,
    TxRequest, TxSecurity, TxStatus, csl_phase,
};

use super::{
    IEEE802154_ENHANCED_ACK_IE_CAPACITY, Ieee802154Csl, Ieee802154EnhancedAckGenerator,
    Ieee802154EnhancedAckIeTooLong, Ieee802154Radio, Ieee802154RadioSink, Ieee802154Random,
};

static LEVELS: [i8; 3] = [-9, 0, 10];

/// A random source that always draws 21.
struct FixedRandom;

impl Ieee802154Random for FixedRandom {
    fn random(&mut self) -> u32 {
        21
    }
}

/// Owned copy of one delivered event: the frame bytes and the event.
#[derive(Debug, PartialEq)]
enum Seen {
    Received {
        mac: Vec<u8>,
        channel: u8,
    },
    TransmitDone {
        id: RequestId,
        status: TxStatus,
        ack_pending: Option<FramePending>,
    },
    EnergyScanDone(i8),
    EnergyScanFailed,
    ClearChannelAssessmentDone {
        idle: bool,
    },
    ClearChannelAssessmentFailed,
    ScheduledReceiveDone(RequestId),
    Fault,
}

/// The delivered events, the security each transmission reported and the
/// acknowledgement each received frame was sent.
#[derive(Default)]
struct Sink(
    Vec<Seen>,
    Vec<Option<AppliedSecurity>>,
    Vec<SentAcknowledgement>,
    Vec<Option<Interface>>,
);

impl Ieee802154RadioSink for Sink {
    fn event(&mut self, event: RadioEvent<'_>) {
        match event {
            RadioEvent::Received(frame) => {
                self.2.push(frame.metadata.sent_acknowledgement);
                self.3.push(frame.metadata.interface);
            }
            RadioEvent::TransmitDone { security, .. } => self.1.push(security),
            _ => {}
        }
        self.0.push(match event {
            RadioEvent::Received(frame) => Seen::Received {
                mac: frame.frame.bytes().to_vec(),
                channel: frame.metadata.channel.get(),
            },
            RadioEvent::TransmitDone {
                id,
                status,
                acknowledgement,
                ..
            } => Seen::TransmitDone {
                id,
                status,
                ack_pending: acknowledgement.map(|ack| ack.metadata.frame_pending),
            },
            RadioEvent::EnergyScanDone { energy_dbm, .. } => Seen::EnergyScanDone(energy_dbm),
            RadioEvent::EnergyScanFailed { .. } => Seen::EnergyScanFailed,
            RadioEvent::ClearChannelAssessmentDone { idle, .. } => {
                Seen::ClearChannelAssessmentDone { idle }
            }
            RadioEvent::ClearChannelAssessmentFailed { .. } => Seen::ClearChannelAssessmentFailed,
            RadioEvent::ScheduledReceiveDone { id } => Seen::ScheduledReceiveDone(id),
            RadioEvent::Fault { .. } | RadioEvent::Lifecycle(_) => Seen::Fault,
        });
    }
}

/// 2006 data frame without an ACK request.
const DATA: [u8; 10] = [0x41, 0x98, 0x01, 0x34, 0x12, 0xff, 0xff, 0x78, 0x56, 0xaa];
/// The same frame requesting an ACK.
const DATA_ACK: [u8; 10] = [0x61, 0x98, 0x01, 0x34, 0x12, 0xff, 0xff, 0x78, 0x56, 0xaa];

/// `mac` as the receive DMA writes it, with RSSI and LQI after it.
fn image(mac: &[u8]) -> Vec<u8> {
    let mut image = vec![mac.len() as u8 + 2];
    image.extend_from_slice(mac);
    image.extend_from_slice(&[-60i8 as u8, 200]);
    image
}

fn channel(number: u8) -> Channel {
    Channel::new(number).unwrap()
}

struct Bench<R = FixedRandom> {
    radio: Ieee802154Radio<'static, R>,
    hw: Ieee802154LlModel,
    clock: oer_time_virtual::VirtualClock,
    sink: Sink,
}

impl Bench {
    fn enabled() -> Self {
        Self::with_engine(engine(), FixedRandom)
    }

    /// An enabled radio over an engine built with multi-PAN.
    fn multipan(interfaces: u8) -> Self {
        let buffers = Box::leak(Box::new(Ieee802154EngineBuffers::new()));
        let levels = Ieee802154TxPowerLevels::new(&LEVELS).unwrap();
        Self::with_engine(
            Ieee802154Engine::new_multipan(
                buffers,
                levels,
                Ieee802154PibDefaults::default(),
                Ieee802154Interfaces::new(interfaces).unwrap(),
            ),
            FixedRandom,
        )
    }
}

fn engine() -> Ieee802154Engine<'static> {
    let buffers = Box::leak(Box::new(Ieee802154EngineBuffers::new()));
    let levels = Ieee802154TxPowerLevels::new(&LEVELS).unwrap();
    Ieee802154Engine::new(buffers, levels, Ieee802154PibDefaults::default())
}

impl<R: Ieee802154Random> Bench<R> {
    fn with_engine(mut engine: Ieee802154Engine<'static>, random: R) -> Self {
        let mut hw = Ieee802154LlModel::default();
        engine.enable();
        engine.mac_init(&mut hw, Ieee802154PibDefaults::default());
        let mut bench = Self {
            radio: Ieee802154Radio::new(engine, random),
            hw,
            clock: oer_time_virtual::VirtualClock::starting_at(oer_time::Instant::from_micros(42)),
            sink: Sink::default(),
        };
        bench
            .radio
            .enable(&mut bench.hw, &bench.clock, &mut bench.sink)
            .unwrap();
        bench
    }

    fn submit(&mut self, command: RadioCommand<'_>) -> Result<(), CommandError> {
        self.radio
            .submit(&mut self.hw, &self.clock, command, &mut self.sink)
            .map(|_| ())
    }

    fn deliver(&mut self, mac: &[u8]) {
        let address = self.hw.rx_address.unwrap();
        assert!(self.radio.engine().model_dma_write(address, &image(mac)));
    }

    fn interrupt(&mut self, events: &[Ieee802154Event]) {
        self.hw.raise(events);
        self.radio.isr(&mut self.hw, &self.clock, &mut self.sink);
    }

    fn transmit(&mut self, id: u32, mac: &[u8], on: u8, mode: TxMode) -> Result<(), CommandError> {
        self.submit(RadioCommand::Transmit(TxRequest {
            id: RequestId::new(id),
            frame: FrameView::new(mac).unwrap(),
            channel: channel(on),
            mode,
            transmit_power_dbm: None,
            max_frame_retries: 0,
            security: Default::default(),
            interface: Interface::PRIMARY,
            time_sync: None,
        }))
    }

    fn seen(&mut self) -> Vec<Seen> {
        core::mem::take(&mut self.sink.0)
    }
}

#[test]
fn admission_needs_enable() {
    let buffers = Box::leak(Box::new(Ieee802154EngineBuffers::new()));
    let levels = Ieee802154TxPowerLevels::new(&LEVELS).unwrap();
    let engine = Ieee802154Engine::new(buffers, levels, Ieee802154PibDefaults::default());
    let mut radio = Ieee802154Radio::new(engine, FixedRandom);
    let clock: oer_time_virtual::VirtualClock =
        oer_time_virtual::VirtualClock::starting_at(oer_time::Instant::from_micros(42));
    assert_eq!(
        radio.submit(
            &mut Ieee802154LlModel::default(),
            &clock,
            RadioCommand::Sleep {
                id: RequestId::new(1)
            },
            &mut Sink::default()
        ),
        Err(CommandError::Disabled)
    );
    assert_eq!(radio.state(), RadioState::Disabled);
}

#[test]
fn received_frames_are_lent_without_their_rssi_and_lqi() {
    let mut bench = Bench::enabled();
    bench
        .submit(RadioCommand::Receive {
            id: RequestId::new(2),
            channel: channel(15),
        })
        .unwrap();
    for _ in 0..30 {
        bench.deliver(&DATA);
        bench.interrupt(&[Ieee802154Event::RxDone]);
    }
    let seen = bench.seen();
    assert_eq!(
        seen.len(),
        30,
        "slots return to the ring after each delivery"
    );
    assert_eq!(
        seen[0],
        Seen::Received {
            mac: DATA.to_vec(),
            channel: 15
        }
    );
}

/// A transmission on another channel resumes receive on the resting channel.
#[test]
fn a_transmission_resumes_receive_on_the_resting_channel() {
    let mut bench = Bench::enabled();
    bench
        .submit(RadioCommand::Receive {
            id: RequestId::new(2),
            channel: channel(15),
        })
        .unwrap();
    bench.transmit(3, &DATA, 20, TxMode::Direct).unwrap();
    assert_eq!(bench.hw.channel().unwrap().number(), 20);
    bench.interrupt(&[Ieee802154Event::TxDone]);
    assert_eq!(
        bench.seen(),
        [Seen::TransmitDone {
            id: RequestId::new(3),
            status: TxStatus::Success,
            ack_pending: None
        }]
    );
    assert_eq!(bench.hw.channel().unwrap().number(), 15);
    assert_eq!(
        bench.radio.state(),
        RadioState::Resting(RestingState::Receiving {
            channel: channel(15)
        })
    );
}

/// A frame the transmit start flushes from the ending receive is delivered.
#[test]
fn a_frame_flushed_by_a_transmit_start_is_delivered() {
    let mut bench = Bench::enabled();
    bench
        .submit(RadioCommand::Receive {
            id: RequestId::new(2),
            channel: channel(15),
        })
        .unwrap();
    bench.deliver(&DATA);
    bench.hw.raise(&[Ieee802154Event::RxDone]);
    bench.transmit(3, &DATA, 15, TxMode::Direct).unwrap();
    assert_eq!(
        bench.seen(),
        [Seen::Received {
            mac: DATA.to_vec(),
            channel: 15
        }]
    );
}

#[test]
fn an_acknowledged_transmission_reports_the_ack_pending_bit() {
    let mut bench = Bench::enabled();
    bench
        .transmit(3, &DATA_ACK, 11, TxMode::ClearChannelAssessment)
        .unwrap();
    bench.interrupt(&[Ieee802154Event::TxDone]);
    assert_eq!(bench.seen(), []);
    bench.deliver(&[0x12, 0x00, 0x01]);
    bench.interrupt(&[Ieee802154Event::AckRxDone]);
    assert_eq!(
        bench.seen(),
        [Seen::TransmitDone {
            id: RequestId::new(3),
            status: TxStatus::Success,
            ack_pending: Some(FramePending::Set)
        }]
    );
}

#[test]
fn transmit_failures_keep_their_vendor_reason() {
    for (reason, status) in [
        (Ieee802154TxAbortReason::CcaBusy, TxStatus::ChannelBusy),
        (Ieee802154TxAbortReason::CcaFailed, TxStatus::Aborted),
        (
            Ieee802154TxAbortReason::TxCoexistenceBreak,
            TxStatus::CoexistenceRejected,
        ),
        (
            Ieee802154TxAbortReason::TxSecurityError,
            TxStatus::SecurityFailure,
        ),
    ] {
        let mut bench = Bench::enabled();
        bench
            .transmit(3, &DATA, 11, TxMode::ClearChannelAssessment)
            .unwrap();
        bench.hw.tx_abort = Ieee802154TxAbortReasonObservation::Named(reason);
        bench.interrupt(&[Ieee802154Event::TxAbort]);
        assert_eq!(
            bench.seen(),
            [Seen::TransmitDone {
                id: RequestId::new(3),
                status,
                ack_pending: None
            }]
        );
    }
}

#[test]
fn energy_scans_and_cca_report_results_and_aborts() {
    let mut bench = Bench::enabled();
    let scan = |id| {
        RadioCommand::EnergyScan(EnergyScanRequest {
            id: RequestId::new(id),
            channel: channel(26),
            duration_us: 128,
        })
    };
    bench.submit(scan(2)).unwrap();
    bench.hw.ed_rss = -80;
    bench.interrupt(&[Ieee802154Event::EdDone]);
    bench.submit(scan(3)).unwrap();
    bench.hw.rx_abort = Ieee802154RxAbortReasonObservation::Named(Ieee802154RxAbortReason::EdAbort);
    bench.interrupt(&[Ieee802154Event::RxAbort]);
    bench
        .submit(RadioCommand::ClearChannelAssessment {
            id: RequestId::new(4),
            channel: channel(26),
        })
        .unwrap();
    bench.hw.cca_busy = true;
    bench.interrupt(&[Ieee802154Event::EdDone]);
    bench
        .submit(RadioCommand::ClearChannelAssessment {
            id: RequestId::new(5),
            channel: channel(26),
        })
        .unwrap();
    bench.interrupt(&[Ieee802154Event::RxAbort]);
    assert_eq!(
        bench.seen(),
        [
            Seen::EnergyScanDone(-80),
            Seen::EnergyScanFailed,
            Seen::ClearChannelAssessmentDone { idle: false },
            Seen::ClearChannelAssessmentFailed,
        ]
    );
    assert_eq!(
        bench.radio.state(),
        RadioState::Resting(RestingState::Sleeping)
    );
}

#[test]
fn configuration_and_settings_reach_the_identity_and_the_pib() {
    let mut bench = Bench::enabled();
    for configuration in [
        Configuration::PanId(0x1234),
        Configuration::ShortAddress(0x5678),
        Configuration::ExtendedAddress([1, 2, 3, 4, 5, 6, 7, 8]),
        Configuration::Promiscuous(false),
        Configuration::AutomaticAcknowledgement(false),
        Configuration::PanCoordinator(true),
    ] {
        bench
            .submit(RadioCommand::Configure {
                id: RequestId::new(9),
                configuration,
            })
            .unwrap();
    }
    bench.radio.set_transmit_power(None, 0).unwrap();
    bench
        .radio
        .set_transmit_power(Some(channel(15)), -3)
        .unwrap();
    bench.radio.set_cca_threshold(-70).unwrap();
    bench.radio.set_cca_mode(CcaMode::CarrierAndEnergyDetection);
    assert_eq!(
        (
            bench.hw.panid[0],
            bench.hw.short_address[0],
            bench.hw.extended_address[0]
        ),
        (0x1234, 0x5678, [1, 2, 3, 4, 5, 6, 7, 8])
    );
    let pib = bench.radio.engine().pib();
    assert!(!pib.promiscuous());
    assert!(!pib.auto_ack_tx());
    let mut powers = [0; 16];
    powers[15 - 11] = -3;
    assert_eq!(pib.power_table(), powers);
    assert_eq!(pib.cca_threshold(), -70);
    assert_eq!(pib.cca_mode(), Ieee802154CcaMode::CarrierAndEnergyDetection);
    assert!(pib.coordinator());
}

/// The radio publishes the engine's transmit-power levels, starting at the
/// highest, the Kconfig CCA threshold range starting from the PIB defaults,
/// the chip's receive sensitivity and the enhanced-ACK IE bytes it holds.
#[test]
fn the_radio_publishes_its_power_cca_and_sensitivity() {
    let bench = Bench::enabled();
    let capabilities = bench.radio.port_capabilities();
    let power = capabilities.transmit_power.unwrap();
    assert_eq!(power.levels.levels_dbm(), &LEVELS);
    assert_eq!(power.reset_dbm, 10);
    assert_eq!(
        capabilities.cca_threshold,
        Some(CcaThresholdCapability {
            lowest_dbm: -120,
            highest_dbm: 0,
            reset_dbm: -75,
        })
    );
    assert_eq!(capabilities.receive_sensitivity_dbm, -104);
    assert_eq!(
        usize::from(capabilities.enhanced_ack_header_ies),
        IEEE802154_ENHANCED_ACK_IE_CAPACITY
    );
}

/// A transmit power or CCA threshold outside the published capability is
/// refused, returning it, and the PIB keeps its value.
#[test]
fn a_power_or_threshold_outside_the_capability_is_refused() {
    let mut bench = Bench::enabled();
    for (channel, power_dbm) in [(None, 11), (Some(channel(15)), -10)] {
        assert_eq!(
            bench.radio.set_transmit_power(channel, power_dbm),
            Err(TransmitPowerError::OutOfRange {
                requested_dbm: power_dbm,
                lowest_dbm: -9,
                highest_dbm: 10,
            })
        );
    }
    assert_eq!(
        bench.radio.set_cca_threshold(1),
        Err(CcaThresholdError::OutOfRange {
            requested_dbm: 1,
            lowest_dbm: -120,
            highest_dbm: 0,
        })
    );
    let pib = bench.radio.engine().pib();
    assert_eq!(pib.power_table(), [10; 16]);
    assert_eq!(pib.cca_threshold(), -75);
}

/// A 2015 data frame requesting an ACK, short addresses, secured at
/// ENC-MIC-32 with key index 1, then a payload byte and its MIC.
const SECURED_2015: [u8; 20] = [
    0x69, 0xa8, 0x11, 0x34, 0x12, 0x01, 0x00, 0x02, 0x00, 0x0d, 0x01, 0x02, 0x03, 0x04, 0x01, 0xaa,
    0, 0, 0, 0,
];

impl Bench {
    fn receiving() -> Self {
        let mut bench = Self::enabled();
        bench
            .submit(RadioCommand::Receive {
                id: RequestId::new(2),
                channel: channel(15),
            })
            .unwrap();
        bench
    }

    /// The image the MAC transmits next.
    fn transmit_image(&mut self) -> Vec<u8> {
        let address = self.hw.tx_address.unwrap();
        let image = self.radio.engine().model_dma_read(address).unwrap();
        image[..=usize::from(image[0])].to_vec()
    }
}

/// With a generator and keys, a secured 2015 frame is answered with a
/// secured enhanced ACK carrying the next frame counter and the configured
/// header IEs; transmit security is armed for it and the frame is delivered
/// once the ACK is sent.
#[test]
fn a_secured_2015_frame_is_answered_with_a_secured_enhanced_ack() {
    let mut bench = Bench::receiving();
    let mut generator = Ieee802154EnhancedAckGenerator::new();
    generator.set_header_ies(&[0x00, 0x00]).unwrap();
    *bench.radio.mac_keys() = Some(MacKeys::new(1, [1; 16], [2; 16], [3; 16], 7));
    *bench.radio.enhanced_ack() = Some(generator);

    bench.deliver(&SECURED_2015);
    bench.interrupt(&[Ieee802154Event::RxDone]);
    assert!(bench.hw.transmit_security);
    assert_eq!(
        bench.transmit_image(),
        [
            0x15, // MAC bytes plus the FCS
            // ACK, secured, frame pending (the disabled automatic mode marks
            // every frame pending), IE present.
            0x1a, 0x2a, 0x11, 0x34, 0x12, 0x02, 0x00, //
            0x0d, 0x07, 0x00, 0x00, 0x00, 0x01, // frame counter 7, key index 1
            0x00, 0x00, // header IE
            0x00, 0x00, 0x00, 0x00, // MIC
            0x00, 0x00, // FCS
        ]
    );
    assert_eq!(bench.radio.mac_keys().unwrap().frame_counter(), 8);
    assert!(bench.seen().is_empty(), "the frame waits for its ACK");

    bench.interrupt(&[Ieee802154Event::AckTxDone]);
    assert_eq!(
        bench.seen(),
        [Seen::Received {
            mac: SECURED_2015.to_vec(),
            channel: 15
        }]
    );
    assert!(!bench.hw.transmit_security);
    // The receive information of `ot_radio_receive_done`.
    assert_eq!(
        bench.sink.2,
        [SentAcknowledgement {
            frame_pending: true,
            security: Some(AppliedSecurity {
                frame_counter: 7,
                key_id: Some(1)
            })
        }]
    );

    // The next frame was not answered with a secured ACK.
    bench.deliver(&DATA);
    bench.interrupt(&[Ieee802154Event::RxDone]);
    assert_eq!(bench.sink.2[1].security, None);
}

/// Without keys a secured 2015 frame gets no ACK and is delivered at once,
/// as when the vendor generator fails.
#[test]
fn without_keys_a_secured_2015_frame_is_delivered_without_an_ack() {
    let mut bench = Bench::receiving();
    *bench.radio.enhanced_ack() = Some(Ieee802154EnhancedAckGenerator::new());
    bench.deliver(&SECURED_2015);
    bench.interrupt(&[Ieee802154Event::RxDone]);
    assert!(!bench.hw.transmit_security);
    assert_eq!(
        bench.seen(),
        [Seen::Received {
            mac: SECURED_2015.to_vec(),
            channel: 15
        }]
    );
}

#[test]
fn header_ies_are_bounded_by_the_port_capacity() {
    let mut generator = Ieee802154EnhancedAckGenerator::new();
    assert_eq!(
        generator.set_header_ies(&[0; IEEE802154_ENHANCED_ACK_IE_CAPACITY + 1]),
        Err(Ieee802154EnhancedAckIeTooLong)
    );
    assert!(generator.header_ies().is_empty());
    generator
        .set_header_ies(&[1; IEEE802154_ENHANCED_ACK_IE_CAPACITY])
        .unwrap();
    assert_eq!(
        generator.header_ies().len(),
        IEEE802154_ENHANCED_ACK_IE_CAPACITY
    );
}

impl<R: Ieee802154Random> Bench<R> {
    fn tx_abort(&mut self, reason: Ieee802154TxAbortReason) {
        self.hw.tx_abort = Ieee802154TxAbortReasonObservation::Named(reason);
        self.interrupt(&[Ieee802154Event::TxAbort]);
    }
}

/// `SubMac` backs off before each CCA attempt, receiving on the transmit
/// channel while the radio receives when idle; a busy channel backs off
/// again with a larger exponent, and success resumes the resting channel.
#[test]
fn a_csma_ca_transmission_backs_off_before_each_cca_attempt() {
    let mut bench = Bench::enabled();
    bench
        .submit(RadioCommand::Receive {
            id: RequestId::new(2),
            channel: channel(15),
        })
        .unwrap();
    bench
        .transmit(3, &DATA, 20, TxMode::CsmaCa { max_backoffs: 4 })
        .unwrap();
    // Receive on the transmit channel during the backoff.
    assert_eq!(bench.hw.channel().unwrap().number(), 20);
    assert_eq!(bench.hw.command, Some(Ieee802154LlCommand::RxStart));
    // 21 % 2^3 unit periods of 320 us.
    assert_eq!(bench.radio.take_delay(), Some(5 * 320));
    assert_eq!(bench.radio.take_delay(), None, "one timer per backoff");

    bench
        .radio
        .delay_elapsed(&mut bench.hw, &bench.clock, &mut bench.sink);
    assert_eq!(bench.hw.command, Some(Ieee802154LlCommand::CcaTxStart));
    bench.tx_abort(Ieee802154TxAbortReason::CcaBusy);
    assert!(bench.seen().is_empty(), "a busy channel backs off again");
    assert_eq!(bench.hw.command, Some(Ieee802154LlCommand::RxStart));
    // 21 % 2^4.
    assert_eq!(bench.radio.take_delay(), Some(5 * 320));

    bench
        .radio
        .delay_elapsed(&mut bench.hw, &bench.clock, &mut bench.sink);
    bench.interrupt(&[Ieee802154Event::TxDone]);
    assert_eq!(
        bench.seen(),
        [Seen::TransmitDone {
            id: RequestId::new(3),
            status: TxStatus::Success,
            ack_pending: None
        }]
    );
    assert_eq!(bench.hw.channel().unwrap().number(), 15);
    assert_eq!(bench.radio.take_delay(), None);
}

/// Random words in order, counting the draws.
struct Words {
    words: Vec<u32>,
    drawn: std::rc::Rc<core::cell::Cell<usize>>,
}

impl Ieee802154Random for Words {
    fn random(&mut self) -> u32 {
        let index = self.drawn.get();
        self.drawn.set(index + 1);
        self.words[index]
    }
}

/// Each backoff draws one word from the radio's own source, the next backoff
/// the next word.
#[test]
fn each_backoff_draws_one_word_from_the_random_source() {
    let drawn = std::rc::Rc::new(core::cell::Cell::new(0));
    let mut bench = Bench::with_engine(
        engine(),
        Words {
            words: vec![3, 6],
            drawn: drawn.clone(),
        },
    );
    bench
        .transmit(3, &DATA, 20, TxMode::CsmaCa { max_backoffs: 4 })
        .unwrap();
    // 3 % 2^3 unit periods of 320 us.
    assert_eq!(bench.radio.take_delay(), Some(3 * 320));
    assert_eq!(drawn.get(), 1);
    bench
        .radio
        .delay_elapsed(&mut bench.hw, &bench.clock, &mut bench.sink);
    bench.tx_abort(Ieee802154TxAbortReason::CcaBusy);
    // 6 % 2^4.
    assert_eq!(bench.radio.take_delay(), Some(6 * 320));
    assert_eq!(drawn.get(), 2);
}

/// A busy channel, an abort and a coexistence rejection are channel-access
/// failures; after `max_backoffs` of them the next ends the transmission
/// with a busy channel. A sleeping radio sleeps through its backoffs.
#[test]
fn channel_access_failures_end_with_a_busy_channel() {
    let mut bench = Bench::enabled();
    bench
        .transmit(3, &DATA, 11, TxMode::CsmaCa { max_backoffs: 2 })
        .unwrap();
    assert_eq!(bench.hw.command, Some(Ieee802154LlCommand::Stop));
    for reason in [
        Ieee802154TxAbortReason::CcaFailed,
        Ieee802154TxAbortReason::TxCoexistenceBreak,
    ] {
        assert!(bench.radio.take_delay().is_some());
        bench
            .radio
            .delay_elapsed(&mut bench.hw, &bench.clock, &mut bench.sink);
        bench.tx_abort(reason);
        assert!(bench.seen().is_empty());
    }
    assert!(bench.radio.take_delay().is_some());
    bench
        .radio
        .delay_elapsed(&mut bench.hw, &bench.clock, &mut bench.sink);
    bench.tx_abort(Ieee802154TxAbortReason::CcaBusy);
    assert_eq!(
        bench.seen(),
        [Seen::TransmitDone {
            id: RequestId::new(3),
            status: TxStatus::ChannelBusy,
            ack_pending: None
        }]
    );
    assert_eq!(bench.radio.take_delay(), None);
}

/// Other transmit failures end the transmission at once.
#[test]
fn a_security_failure_ends_a_csma_ca_transmission() {
    let mut bench = Bench::enabled();
    bench
        .transmit(3, &DATA, 11, TxMode::CsmaCa { max_backoffs: 4 })
        .unwrap();
    assert!(bench.radio.take_delay().is_some());
    bench
        .radio
        .delay_elapsed(&mut bench.hw, &bench.clock, &mut bench.sink);
    bench.tx_abort(Ieee802154TxAbortReason::TxSecurityError);
    assert_eq!(
        bench.seen(),
        [Seen::TransmitDone {
            id: RequestId::new(3),
            status: TxStatus::SecurityFailure,
            ack_pending: None
        }]
    );
}

/// Without backoffs the frame goes out at once with one CCA.
#[test]
fn no_backoffs_transmit_at_once_with_one_cca() {
    let mut bench = Bench::enabled();
    bench
        .transmit(3, &DATA, 11, TxMode::CsmaCa { max_backoffs: 0 })
        .unwrap();
    assert_eq!(bench.hw.command, Some(Ieee802154LlCommand::CcaTxStart));
    assert_eq!(bench.radio.take_delay(), None);
    bench.tx_abort(Ieee802154TxAbortReason::CcaBusy);
    assert_eq!(
        bench.seen(),
        [Seen::TransmitDone {
            id: RequestId::new(3),
            status: TxStatus::ChannelBusy,
            ack_pending: None
        }]
    );
}

/// A frame received on the transmit channel during a backoff is delivered.
#[test]
fn a_frame_received_during_a_backoff_is_delivered() {
    let mut bench = Bench::enabled();
    bench
        .submit(RadioCommand::Receive {
            id: RequestId::new(2),
            channel: channel(15),
        })
        .unwrap();
    bench
        .transmit(3, &DATA, 15, TxMode::CsmaCa { max_backoffs: 4 })
        .unwrap();
    bench.deliver(&DATA);
    bench.interrupt(&[Ieee802154Event::RxDone]);
    assert_eq!(
        bench.seen(),
        [Seen::Received {
            mac: DATA.to_vec(),
            channel: 15
        }]
    );
}

impl Bench {
    fn transmit_retried(&mut self, mac: &[u8], mode: TxMode, retries: u8) {
        self.transmit_keyed(mac, mode, retries, TxSecurity::default());
    }

    fn transmit_keyed(&mut self, mac: &[u8], mode: TxMode, retries: u8, security: TxSecurity) {
        self.submit(RadioCommand::Transmit(TxRequest {
            id: RequestId::new(3),
            frame: FrameView::new(mac).unwrap(),
            channel: channel(11),
            mode,
            transmit_power_dbm: None,
            max_frame_retries: retries,
            security,
            interface: Interface::PRIMARY,
            time_sync: None,
        }))
        .unwrap();
    }

    /// The frame went out, and no acknowledgement arrived.
    fn no_ack(&mut self) {
        self.interrupt(&[Ieee802154Event::TxDone]);
        self.tx_abort(Ieee802154TxAbortReason::RxAckTimeout);
    }
}

/// `SubMac` retries a frame without acknowledgement after a random delay
/// whose exponent grows from 0, and reports no acknowledgement once its
/// retries are spent.
#[test]
fn a_frame_without_acknowledgement_is_retried_after_a_growing_delay() {
    let mut bench = Bench::enabled();
    bench.transmit_retried(&DATA_ACK, TxMode::Direct, 2);
    assert_eq!(bench.hw.command, Some(Ieee802154LlCommand::TxStart));
    // 21 % 2^0, then 21 % 2^1 unit periods.
    for delay in [0, 320] {
        bench.no_ack();
        assert!(bench.seen().is_empty(), "the frame is retried");
        assert_eq!(bench.radio.take_delay(), Some(delay));
        bench.hw.command = None;
        bench
            .radio
            .delay_elapsed(&mut bench.hw, &bench.clock, &mut bench.sink);
        assert_eq!(bench.hw.command, Some(Ieee802154LlCommand::TxStart));
    }
    bench.no_ack();
    assert_eq!(
        bench.seen(),
        [Seen::TransmitDone {
            id: RequestId::new(3),
            status: TxStatus::NoAcknowledgement,
            ack_pending: None
        }]
    );
    assert_eq!(bench.radio.take_delay(), None);
}

/// A CSMA-CA frame that finds no channel is retried with fresh backoffs.
#[test]
fn a_csma_ca_frame_without_channel_access_is_retried() {
    let mut bench = Bench::enabled();
    bench.transmit_retried(&DATA, TxMode::CsmaCa { max_backoffs: 1 }, 1);
    // Two CCA attempts per channel acquisition, two acquisitions.
    for _ in 0..4 {
        assert!(bench.seen().is_empty());
        assert!(bench.radio.take_delay().is_some());
        bench
            .radio
            .delay_elapsed(&mut bench.hw, &bench.clock, &mut bench.sink);
        assert_eq!(bench.hw.command, Some(Ieee802154LlCommand::CcaTxStart));
        bench.tx_abort(Ieee802154TxAbortReason::CcaBusy);
    }
    assert_eq!(
        bench.seen(),
        [Seen::TransmitDone {
            id: RequestId::new(3),
            status: TxStatus::ChannelBusy,
            ack_pending: None
        }]
    );
}

/// A success after a retry ends the transmission as usual.
#[test]
fn a_retried_frame_can_succeed() {
    let mut bench = Bench::enabled();
    bench.transmit_retried(&DATA_ACK, TxMode::ClearChannelAssessment, 3);
    bench.tx_abort(Ieee802154TxAbortReason::CcaBusy);
    assert!(bench.seen().is_empty());
    // A channel-access retry starts at once, with no delay.
    assert_eq!(bench.radio.take_delay(), None);
    assert_eq!(bench.hw.command, Some(Ieee802154LlCommand::CcaTxStart));
    bench.interrupt(&[Ieee802154Event::TxDone]);
    bench.deliver(&[0x02, 0x00, 0x01]);
    bench.interrupt(&[Ieee802154Event::AckRxDone]);
    assert!(matches!(
        bench.seen().as_slice(),
        [Seen::TransmitDone {
            status: TxStatus::Success,
            ..
        }]
    ));
}

/// The key a request carries is armed for its first attempt and again
/// before each later one, since the engine clears it when an attempt ends.
#[test]
fn a_request_key_is_armed_for_each_attempt() {
    // 2006 data frame requesting an ACK, secured at ENC-MIC-32 key mode 0.
    let secured = [
        0x69, 0x98, 0x01, 0x34, 0x12, 0xff, 0xff, 0x78, 0x56, 0x05, 1, 2, 3, 4, 0xaa, 0, 0, 0, 0,
    ];
    let mut bench = Bench::enabled();
    bench.transmit_keyed(&secured, TxMode::Direct, 1, KEY);
    assert!(bench.hw.transmit_security);
    bench.no_ack();
    assert!(!bench.hw.transmit_security, "the failed attempt cleared it");
    assert!(bench.radio.take_delay().is_some());
    bench
        .radio
        .delay_elapsed(&mut bench.hw, &bench.clock, &mut bench.sink);
    assert!(bench.hw.transmit_security, "the retry armed it again");
}

/// The CCM* key and nonce source a request carries.
const KEY: TxSecurity = TxSecurity::Key {
    key: [7; 16],
    address: [9; 8],
};

/// A 2006 data frame requesting an ACK, secured at ENC-MIC-32 in key
/// identifier mode 1 with a zero counter and key index, then a payload byte
/// and its MIC.
const SECURED_MODE_1: [u8; 20] = [
    0x69, 0x98, 0x01, 0x34, 0x12, 0x02, 0x00, 0x01, 0x00, 0x0d, 0, 0, 0, 0, 0, 0xaa, 0, 0, 0, 0,
];

impl Bench {
    /// The frame counter and key index of the secured frame the MAC
    /// transmits next.
    fn transmitted_security(&mut self) -> ([u8; 4], u8) {
        let image = self.transmit_image();
        (image[11..15].try_into().unwrap(), image[15])
    }
}

/// With MAC keys the radio secures each attempt as ESP-IDF's OpenThread
/// port does: every CCA attempt of the first transmission takes a new frame
/// counter with the current key index, a retransmission keeps its counter.
#[test]
fn the_radio_secures_each_attempt_with_its_mac_keys() {
    let mut bench = Bench::enabled();
    *bench.radio.mac_keys() = Some(MacKeys::new(4, [1; 16], [2; 16], [3; 16], 100));
    bench.transmit_retried(&SECURED_MODE_1, TxMode::CsmaCa { max_backoffs: 1 }, 1);

    assert!(bench.radio.take_delay().is_some());
    bench
        .radio
        .delay_elapsed(&mut bench.hw, &bench.clock, &mut bench.sink);
    assert!(bench.hw.transmit_security);
    assert_eq!(bench.transmitted_security(), ([100, 0, 0, 0], 4));
    bench.tx_abort(Ieee802154TxAbortReason::CcaBusy);

    // The second CCA attempt is a transmit of its own.
    assert!(bench.radio.take_delay().is_some());
    bench
        .radio
        .delay_elapsed(&mut bench.hw, &bench.clock, &mut bench.sink);
    assert_eq!(bench.transmitted_security(), ([101, 0, 0, 0], 4));
    bench.no_ack();

    // The retransmission keeps the counter.
    assert!(bench.radio.take_delay().is_some());
    bench
        .radio
        .delay_elapsed(&mut bench.hw, &bench.clock, &mut bench.sink);
    assert!(bench.radio.take_delay().is_some());
    bench
        .radio
        .delay_elapsed(&mut bench.hw, &bench.clock, &mut bench.sink);
    assert!(bench.hw.transmit_security);
    assert_eq!(bench.transmitted_security(), ([101, 0, 0, 0], 4));
    assert_eq!(bench.radio.mac_keys().unwrap().frame_counter(), 102);
}

/// The key a request carries wins over the MAC keys, which keep their
/// counter; without keys a secured frame goes out as given.
#[test]
fn a_request_key_or_no_keys_leave_the_frame_as_given() {
    let mut bench = Bench::enabled();
    *bench.radio.mac_keys() = Some(MacKeys::new(4, [1; 16], [2; 16], [3; 16], 100));
    bench.transmit_keyed(&SECURED_MODE_1, TxMode::Direct, 0, KEY);
    assert!(bench.hw.transmit_security);
    assert_eq!(bench.transmitted_security(), ([0, 0, 0, 0], 0));
    assert_eq!(bench.radio.mac_keys().unwrap().frame_counter(), 100);

    let mut bench = Bench::enabled();
    bench.transmit_retried(&SECURED_MODE_1, TxMode::Direct, 0);
    assert!(!bench.hw.transmit_security);
    assert_eq!(bench.transmitted_security(), ([0, 0, 0, 0], 0));
}

impl Bench {
    fn transmit_secured(&mut self, mac: &[u8], security: TxSecurity) {
        self.submit(RadioCommand::Transmit(TxRequest {
            id: RequestId::new(3),
            frame: FrameView::new(mac).unwrap(),
            channel: channel(11),
            mode: TxMode::Direct,
            transmit_power_dbm: None,
            max_frame_retries: 0,
            security,
            interface: Interface::PRIMARY,
            time_sync: None,
        }))
        .unwrap();
    }
}

/// The transmission reports the frame counter and key index it wrote, also
/// when no acknowledgement arrived, as the port leaves them in the stack's
/// frame for `SubMac::SignalFrameCounterUsedOnTxDone`.
#[test]
fn a_transmission_reports_the_security_it_wrote() {
    let mut bench = Bench::enabled();
    *bench.radio.mac_keys() = Some(MacKeys::new(4, [1; 16], [2; 16], [3; 16], 100));
    bench.transmit_secured(&SECURED_MODE_1, TxSecurity::Radio);
    assert_eq!(bench.transmitted_security(), ([100, 0, 0, 0], 4));
    bench.no_ack();
    assert_eq!(
        bench.sink.1,
        [Some(AppliedSecurity {
            frame_counter: 100,
            key_id: Some(4)
        })]
    );

    // Unsecured frames report nothing.
    bench.transmit_secured(&DATA, TxSecurity::Radio);
    bench.interrupt(&[Ieee802154Event::TxDone]);
    assert_eq!(bench.sink.1[1], None);
}

/// The stack's retransmission (`mIsARetx`) is secured again under the
/// counter and key index the frame carries; a frame the stack secured
/// itself (`mIsSecurityProcessed`) goes out as given.
#[test]
fn stack_retransmissions_keep_their_counter_and_processed_frames_are_sent_as_given() {
    let mut carried = SECURED_MODE_1;
    carried[10..15].copy_from_slice(&[55, 0, 0, 0, 3]);

    let mut bench = Bench::enabled();
    *bench.radio.mac_keys() = Some(MacKeys::new(4, [1; 16], [2; 16], [3; 16], 100));
    bench.transmit_secured(&carried, TxSecurity::Retransmission);
    assert!(bench.hw.transmit_security);
    assert_eq!(bench.transmitted_security(), ([55, 0, 0, 0], 3));
    bench.interrupt(&[Ieee802154Event::TxDone]);
    bench.tx_abort(Ieee802154TxAbortReason::RxAckTimeout);
    assert_eq!(bench.sink.1, [None]);

    bench.transmit_secured(&carried, TxSecurity::Processed);
    assert!(!bench.hw.transmit_security);
    assert_eq!(bench.transmitted_security(), ([55, 0, 0, 0], 3));
    assert_eq!(bench.radio.mac_keys().unwrap().frame_counter(), 100);
}

impl Bench {
    fn scheduled_receive(&mut self, id: u32, start: u64, duration_us: u32) {
        self.submit(RadioCommand::ScheduledReceive(ScheduledReceiveRequest {
            id: RequestId::new(id),
            channel: channel(15),
            start: Ieee802154Instant::from_micros(start),
            duration_us,
        }))
        .unwrap();
    }
}

/// `otPlatRadioReceiveAt`: the window receives on its channel; as in the
/// vendor driver, the first frame ends it and the radio sleeps.
#[test]
fn a_scheduled_window_ends_with_its_first_frame() {
    let mut bench = Bench::receiving();
    bench.scheduled_receive(5, 5_000, 2_000);
    assert_eq!(
        bench.radio.state(),
        RadioState::Resting(RestingState::ScheduledReceiving {
            id: RequestId::new(5),
            channel: channel(15),
        })
    );
    assert_eq!(bench.radio.engine().state(), Ieee802154State::Rx);
    assert!(bench.hw.etm_enabled[1], "timer one starts the receiver");

    // The window opens, then a frame arrives.
    bench.interrupt(&[Ieee802154Event::Timer1Overflow]);
    bench.deliver(&DATA);
    bench.interrupt(&[Ieee802154Event::RxDone]);
    assert_eq!(
        bench.seen(),
        [
            Seen::Received {
                mac: DATA.to_vec(),
                channel: 15
            },
            Seen::ScheduledReceiveDone(RequestId::new(5)),
        ]
    );
    assert_eq!(
        bench.radio.state(),
        RadioState::Resting(RestingState::Sleeping)
    );
}

/// A window without frames ends at its end and leaves the radio asleep.
#[test]
fn a_scheduled_window_without_frames_ends_at_its_end() {
    let mut bench = Bench::enabled();
    bench.scheduled_receive(5, 5_000, 2_000);
    bench.interrupt(&[Ieee802154Event::Timer1Overflow]);
    assert_eq!(bench.seen(), []);
    bench.interrupt(&[Ieee802154Event::Timer1Overflow]);
    assert_eq!(
        bench.seen(),
        [Seen::ScheduledReceiveDone(RequestId::new(5))]
    );
    assert_eq!(
        bench.radio.state(),
        RadioState::Resting(RestingState::Sleeping)
    );
}

/// A window that already ended does not start: the radio sleeps and the
/// window ends at once.
#[test]
fn an_elapsed_window_ends_at_once() {
    let mut bench = Bench::receiving();
    // The bench clock reads 42 microseconds.
    bench.scheduled_receive(5, 10, 20);
    assert_eq!(
        bench.seen(),
        [Seen::ScheduledReceiveDone(RequestId::new(5))]
    );
    assert_eq!(
        bench.radio.state(),
        RadioState::Resting(RestingState::Sleeping)
    );
    assert_ne!(bench.radio.engine().state(), Ieee802154State::Rx);
}

/// A transmission ends a window, and the radio sleeps after it; a window
/// end the engine still reports afterwards is ignored.
#[test]
fn a_transmission_ends_a_window() {
    let mut bench = Bench::enabled();
    bench.scheduled_receive(5, 5_000, 2_000);
    bench.transmit(6, &DATA, 15, TxMode::Direct).unwrap();
    bench.interrupt(&[Ieee802154Event::TxDone]);
    assert_eq!(
        bench.seen(),
        [Seen::TransmitDone {
            id: RequestId::new(6),
            status: TxStatus::Success,
            ack_pending: None
        }]
    );
    assert_eq!(
        bench.radio.state(),
        RadioState::Resting(RestingState::Sleeping)
    );
    bench.interrupt(&[Ieee802154Event::Timer1Overflow]);
    assert_eq!(bench.seen(), []);
    assert_eq!(
        bench.radio.state(),
        RadioState::Resting(RestingState::Sleeping)
    );
}

/// A scheduled transmission with a CCA assesses the channel first, as
/// `esp_i_transmit_at` with `cca` does.
#[test]
fn a_scheduled_transmission_can_assess_the_channel() {
    let at = Ieee802154Instant::from_micros(5_000);
    let mut bench = Bench::enabled();
    bench
        .transmit(6, &DATA, 15, TxMode::Scheduled { at, cca: true })
        .unwrap();
    assert_eq!(bench.radio.engine().state(), Ieee802154State::TxCca);
    assert!(bench.hw.etm_enabled[0], "timer zero starts the CCA");

    let mut bench = Bench::enabled();
    bench
        .transmit(6, &DATA, 15, TxMode::Scheduled { at, cca: false })
        .unwrap();
    assert_eq!(bench.radio.engine().state(), Ieee802154State::Tx);
}

/// A 2015 data frame requesting an ACK, without security.
const PLAIN_2015: [u8; 10] = [0x61, 0xa8, 0x11, 0x34, 0x12, 0x01, 0x00, 0x02, 0x00, 0xaa];

/// The CSL period and sample time of the CSL tests: 100 units (16 ms),
/// sampling at 5000 us; the bench clock reads 42 us.
const CSL: Ieee802154Csl = Ieee802154Csl {
    period: 100,
    sample_time: 5_000,
};

/// The CSL IE content of `image`: phase and period.
fn csl_content(image: &[u8]) -> [u16; 2] {
    let at = image
        .windows(2)
        .position(|pair| pair == [0x04, 0x0d])
        .expect("the frame carries a CSL IE")
        + 2;
    [
        u16::from_le_bytes([image[at], image[at + 1]]),
        u16::from_le_bytes([image[at + 2], image[at + 3]]),
    ]
}

/// With CSL, the enhanced ACK carries the port's CSL IE template, filled
/// with the period and phase when its SFD goes out.
#[test]
fn a_csl_receiver_acknowledges_with_a_csl_ie() {
    let mut bench = Bench::receiving();
    *bench.radio.enhanced_ack() = Some(Ieee802154EnhancedAckGenerator::new());
    *bench.radio.csl() = CSL;
    bench.deliver(&PLAIN_2015);
    bench.interrupt(&[Ieee802154Event::RxDone]);
    assert_eq!(csl_content(&bench.transmit_image()), [0, 0]);
    bench.interrupt(&[Ieee802154Event::TxSfdDone]);
    let phase = csl_phase(42, CSL.sample_time, CSL.period).unwrap();
    assert_eq!(csl_content(&bench.transmit_image()), [phase, CSL.period]);

    // Without CSL the ACK has no IE.
    let mut bench = Bench::receiving();
    *bench.radio.enhanced_ack() = Some(Ieee802154EnhancedAckGenerator::new());
    bench.deliver(&PLAIN_2015);
    bench.interrupt(&[Ieee802154Event::RxDone]);
    assert!(
        !bench
            .transmit_image()
            .windows(2)
            .any(|pair| pair == [0x04, 0x0d])
    );
}

/// A frame the stack prepared with a CSL IE gets the period and phase at
/// its SFD; a CSL retransmission takes a new frame counter.
#[test]
fn a_csl_receiver_fills_its_frames_and_secures_retries_anew() {
    // 2015 data frame, IE present, secured at ENC-MIC-32 in key identifier
    // mode 1, a CSL IE template, a termination IE, a payload byte and MIC.
    let mut frame = std::vec![0x69, 0xaa, 0x11, 0x34, 0x12, 0x01, 0x00, 0x02, 0x00];
    frame.extend_from_slice(&[0x0d, 55, 0, 0, 0, 3]);
    frame.extend_from_slice(&CSL_IE_TEMPLATE);
    frame.extend_from_slice(&[0x00, 0x3f, 0xaa, 0, 0, 0, 0]);

    let mut bench = Bench::enabled();
    *bench.radio.mac_keys() = Some(MacKeys::new(4, [1; 16], [2; 16], [3; 16], 100));
    *bench.radio.csl() = CSL;
    bench.transmit_secured(&frame, TxSecurity::Retransmission);
    // The retransmission took counter 100 and kept key index 3.
    assert_eq!(bench.transmitted_security(), ([100, 0, 0, 0], 3));
    bench.interrupt(&[Ieee802154Event::TxSfdDone]);
    let phase = csl_phase(42, CSL.sample_time, CSL.period).unwrap();
    assert_eq!(csl_content(&bench.transmit_image()), [phase, CSL.period]);
}

impl Bench {
    fn configure(&mut self, configuration: Configuration) -> Result<(), CommandError> {
        self.submit(RadioCommand::Configure {
            id: RequestId::new(9),
            configuration,
        })
    }
}

/// Source matching fills interface zero's pending table and mode; a full
/// half refuses a new source before admission.
#[test]
fn source_matching_configures_the_pending_table() {
    let mut bench = Bench::enabled();
    bench
        .configure(Configuration::PendingMode(AutoPendingMode::Enhanced))
        .unwrap();
    let short = |index: u8| FrameAddress::Short([index, 0]);
    for index in 0..PENDING_TABLE_SIZE as u8 {
        bench
            .configure(Configuration::AddPendingAddress(short(index)))
            .unwrap();
    }
    assert_eq!(
        bench.configure(Configuration::AddPendingAddress(short(200))),
        Err(CommandError::PendingTableFull)
    );
    bench
        .configure(Configuration::AddPendingAddress(short(3)))
        .unwrap();
    bench
        .configure(Configuration::AddPendingAddress(FrameAddress::Extended(
            [7; 8],
        )))
        .unwrap();
    bench
        .configure(Configuration::RemovePendingAddress(short(3)))
        .unwrap();
    bench
        .configure(Configuration::AddPendingAddress(short(200)))
        .unwrap();

    let engine = bench.radio.engine();
    assert_eq!(
        engine.pib().pending_mode(Ieee802154MultipanIndex::CONTEXT0),
        AutoPendingMode::Enhanced
    );
    let table = engine.pending_table();
    assert!(table.contains(short(200)) && !table.contains(short(3)));
    assert!(table.contains(FrameAddress::Extended([7; 8])));

    bench
        .configure(Configuration::ResetPendingTable(PendingTableHalf::Short))
        .unwrap();
    let table = bench.radio.engine().pending_table();
    assert!(!table.contains(short(0)));
    assert!(table.contains(FrameAddress::Extended([7; 8])));
}

impl Bench {
    fn configure_interface(
        &mut self,
        index: u8,
        setting: InterfaceSetting,
    ) -> Result<(), CommandError> {
        self.submit(RadioCommand::Configure {
            id: RequestId::new(9),
            configuration: Configuration::Interface {
                interface: Interface::new(index),
                setting,
            },
        })
    }
}

/// `configure` applies a configuration in any state, as the vendor's
/// setters do: disabled, and during an operation, without changing the
/// radio's state; only a full pending table refuses.
#[test]
fn configure_applies_in_every_state() {
    let buffers = Box::leak(Box::new(Ieee802154EngineBuffers::new()));
    let levels = Ieee802154TxPowerLevels::new(&LEVELS).unwrap();
    let mut engine = Ieee802154Engine::new_multipan(
        buffers,
        levels,
        Ieee802154PibDefaults::default(),
        Ieee802154Interfaces::new(2).unwrap(),
    );
    let mut hw = Ieee802154LlModel::default();
    engine.enable();
    engine.mac_init(&mut hw, Ieee802154PibDefaults::default());
    let mut radio = Ieee802154Radio::new(engine, FixedRandom);
    let interface = |setting| Configuration::Interface {
        interface: Interface::new(1),
        setting,
    };
    radio
        .configure(&mut hw, interface(InterfaceSetting::Enabled(true)))
        .unwrap();
    radio
        .configure(&mut hw, interface(InterfaceSetting::PanId(0xabcd)))
        .unwrap();
    radio
        .configure(&mut hw, Configuration::Promiscuous(true))
        .unwrap();
    assert_eq!(radio.state(), RadioState::Disabled);
    assert!(
        hw.multipan_enable
            .contains(Ieee802154MultipanIndex::CONTEXT1)
    );
    assert_eq!(hw.panid[1], 0xabcd);
    assert!(radio.engine().pib().promiscuous());

    let mut bench = Bench {
        radio,
        hw,
        clock: oer_time_virtual::VirtualClock::starting_at(oer_time::Instant::from_micros(42)),
        sink: Sink::default(),
    };
    bench
        .radio
        .enable(&mut bench.hw, &bench.clock, &mut bench.sink)
        .unwrap();
    bench
        .transmit(
            5,
            &[0x41, 0x98, 0x01, 0x34, 0x12, 0xff, 0xff, 0x78, 0x56],
            20,
            TxMode::Direct,
        )
        .unwrap();
    let transmitting = bench.radio.state();
    assert!(matches!(transmitting, RadioState::Transmitting { .. }));
    let short = FrameAddress::Short([0x22, 0x00]);
    bench
        .radio
        .configure(&mut bench.hw, Configuration::PanId(0x1234))
        .unwrap();
    bench
        .radio
        .configure(
            &mut bench.hw,
            interface(InterfaceSetting::AddPendingAddress(short)),
        )
        .unwrap();
    assert_eq!(bench.radio.state(), transmitting);
    assert_eq!(bench.hw.panid[0], 0x1234);
    assert!(
        bench
            .radio
            .engine()
            .pending_table_for(Ieee802154MultipanIndex::CONTEXT1)
            .contains(short)
    );
    for index in 0..PENDING_TABLE_SIZE {
        let address = FrameAddress::Short([index as u8, 0x10]);
        let _ = bench
            .radio
            .configure(&mut bench.hw, Configuration::AddPendingAddress(address));
    }
    assert_eq!(
        bench.radio.configure(
            &mut bench.hw,
            Configuration::AddPendingAddress(FrameAddress::Short([0xff, 0xff])),
        ),
        Err(CommandError::PendingTableFull)
    );
}

/// An engine built with multi-PAN gives the radio its interfaces; each
/// interface setting reaches that interface's identity, enable bit and
/// frame-pending table, as `esp_i_set_multipan_*` and
/// `esp_i_multipan_*` do.
#[test]
fn interface_settings_reach_their_interface() {
    let mut bench = Bench::multipan(2);
    assert_eq!(bench.radio.interfaces(), 2);
    let short = FrameAddress::Short([0x22, 0x00]);
    for setting in [
        InterfaceSetting::PanId(0xabcd),
        InterfaceSetting::ShortAddress(0x0011),
        InterfaceSetting::ExtendedAddress([8, 7, 6, 5, 4, 3, 2, 1]),
        InterfaceSetting::Enabled(true),
        InterfaceSetting::AddPendingAddress(short),
    ] {
        bench.configure_interface(1, setting).unwrap();
    }
    assert_eq!(
        (
            bench.hw.panid[1],
            bench.hw.short_address[1],
            bench.hw.extended_address[1]
        ),
        (0xabcd, 0x0011, [8, 7, 6, 5, 4, 3, 2, 1])
    );
    assert!(
        bench
            .hw
            .multipan_enable
            .contains(Ieee802154MultipanIndex::CONTEXT1)
    );
    let engine = bench.radio.engine();
    assert!(
        engine
            .pending_table_for(Ieee802154MultipanIndex::CONTEXT1)
            .contains(short)
    );
    assert!(!engine.pending_table().contains(short));

    bench
        .configure_interface(1, InterfaceSetting::Enabled(false))
        .unwrap();
    assert!(
        !bench
            .hw
            .multipan_enable
            .contains(Ieee802154MultipanIndex::CONTEXT1)
    );
    bench
        .configure_interface(
            1,
            InterfaceSetting::ResetPendingTable(PendingTableHalf::Short),
        )
        .unwrap();
    assert!(
        !bench
            .radio
            .engine()
            .pending_table_for(Ieee802154MultipanIndex::CONTEXT1)
            .contains(short)
    );
    assert_eq!(
        bench.configure_interface(2, InterfaceSetting::PanId(1)),
        Err(CommandError::UnknownInterface {
            interface: Interface::new(2),
            interfaces: 2,
        })
    );
    assert!(bench.radio.interface_mac_keys(Interface::new(2)).is_none());
}

/// An interface's frame-pending table refuses a source it has no room for,
/// independently of the other interfaces.
#[test]
fn an_interface_pending_table_refuses_a_source_without_room() {
    let mut bench = Bench::multipan(2);
    for index in 0..PENDING_TABLE_SIZE as u16 {
        let address = FrameAddress::Short(index.to_le_bytes());
        bench
            .configure_interface(1, InterfaceSetting::AddPendingAddress(address))
            .unwrap();
    }
    let extra = FrameAddress::Short([0xee, 0xee]);
    assert_eq!(
        bench.configure_interface(1, InterfaceSetting::AddPendingAddress(extra)),
        Err(CommandError::PendingTableFull)
    );
    bench
        .submit(RadioCommand::Configure {
            id: RequestId::new(9),
            configuration: Configuration::AddPendingAddress(extra),
        })
        .unwrap();
}

/// A radio without multi-PAN has the primary interface alone and reports
/// it for every frame.
#[test]
fn a_radio_without_multi_pan_has_one_interface() {
    let mut bench = Bench::receiving();
    assert_eq!(
        bench.configure_interface(0, InterfaceSetting::PanId(1)),
        Err(CommandError::Unsupported {
            command: oer_ieee802154::CommandKind::Configure,
            required: oer_ieee802154::RadioCapabilities::MULTI_PAN,
        })
    );
    bench.deliver(&DATA);
    bench.interrupt(&[Ieee802154Event::RxDone]);
    assert_eq!(bench.sink.3, [Some(Interface::PRIMARY)]);
}

impl Bench {
    /// A multi-PAN radio receiving on channel 15 whose interface 1 is PAN
    /// 0x1234 with short address 1, the destination of [`SECURED_2015`],
    /// and whose interface 0 is another PAN.
    fn multipan_receiving() -> Self {
        let mut bench = Self::multipan(2);
        bench
            .configure_interface(0, InterfaceSetting::PanId(0x4321))
            .unwrap();
        for setting in [
            InterfaceSetting::PanId(0x1234),
            InterfaceSetting::ShortAddress(0x0001),
            InterfaceSetting::Enabled(true),
        ] {
            bench.configure_interface(1, setting).unwrap();
        }
        bench
            .submit(RadioCommand::Receive {
                id: RequestId::new(2),
                channel: channel(15),
            })
            .unwrap();
        bench
    }
}

/// The enhanced ACK of a frame is secured with the keys of the interface
/// the frame matched, and the frame reports that interface and the ACK's
/// security, as the multi-instance port's `ot_radio_enh_ack_generator`
/// and `ot_radio_receive_done` use `mpf_index`.
#[test]
fn an_enhanced_ack_is_secured_by_the_matched_interface() {
    let mut bench = Bench::multipan_receiving();
    *bench.radio.mac_keys() = Some(MacKeys::new(1, [1; 16], [2; 16], [3; 16], 7));
    *bench.radio.interface_mac_keys(Interface::new(1)).unwrap() =
        Some(MacKeys::new(1, [4; 16], [5; 16], [6; 16], 50));
    *bench.radio.enhanced_ack() = Some(Ieee802154EnhancedAckGenerator::new());

    bench.deliver(&SECURED_2015);
    bench.interrupt(&[Ieee802154Event::RxDone]);
    assert!(bench.hw.transmit_security);
    assert_eq!(bench.hw.security_key, [5; 16]);
    bench.interrupt(&[Ieee802154Event::AckTxDone]);
    assert_eq!(bench.sink.3, [Some(Interface::new(1))]);
    assert_eq!(
        bench.sink.2[0].security,
        Some(AppliedSecurity {
            frame_counter: 50,
            key_id: Some(1)
        })
    );
    let interface_keys = bench.radio.interface_mac_keys(Interface::new(1)).unwrap();
    assert_eq!(interface_keys.unwrap().frame_counter(), 51);
    assert_eq!(bench.radio.mac_keys().unwrap().frame_counter(), 7);
}

/// A broadcast belongs to no single interface and reports none, which the
/// multi-instance port hands to every instance.
#[test]
fn a_broadcast_reports_no_interface() {
    let mut bench = Bench::multipan_receiving();
    bench.deliver(&DATA);
    bench.interrupt(&[Ieee802154Event::RxDone]);
    assert_eq!(bench.sink.3, [None]);
}

/// A transmission is secured with the keys of its interface and that
/// interface's extended address as the nonce source, as the multi-instance
/// port's `radio_start_transmit` does.
#[test]
fn a_transmission_is_secured_by_its_interface() {
    let mut bench = Bench::multipan(2);
    let address = [0x10, 0x20, 0x30, 0x40, 0x50, 0x60, 0x70, 0x80];
    bench
        .configure_interface(1, InterfaceSetting::ExtendedAddress(address))
        .unwrap();
    *bench.radio.mac_keys() = Some(MacKeys::new(4, [1; 16], [2; 16], [3; 16], 100));
    *bench.radio.interface_mac_keys(Interface::new(1)).unwrap() =
        Some(MacKeys::new(4, [4; 16], [5; 16], [6; 16], 300));
    bench
        .submit(RadioCommand::Transmit(TxRequest {
            id: RequestId::new(3),
            frame: FrameView::new(&SECURED_MODE_1).unwrap(),
            channel: channel(11),
            mode: TxMode::Direct,
            transmit_power_dbm: None,
            max_frame_retries: 0,
            security: TxSecurity::Radio,
            interface: Interface::new(1),
            time_sync: None,
        }))
        .unwrap();
    assert!(bench.hw.transmit_security);
    assert_eq!(bench.transmitted_security(), ([0x2c, 0x01, 0, 0], 4));
    assert_eq!(bench.hw.security_key, [5; 16]);
    assert_eq!(bench.hw.security_address, bench.hw.extended_address[1]);
    assert_eq!(bench.radio.mac_keys().unwrap().frame_counter(), 100);
}

/// The enhanced ACK to a frame from a Link Metrics probing initiator
/// carries the probing IE with the frame's LQI and link margin, as the
/// port's generator adds `otLinkMetricsEnhAckGenData`; other sources get
/// none.
#[test]
fn an_enhanced_ack_to_a_probing_initiator_carries_its_link_metrics() {
    use oer_ieee802154::LinkMetrics;
    let mut bench = Bench::receiving();
    let mut generator = Ieee802154EnhancedAckGenerator::new();
    generator.probing().set_noise_floor(-97);
    generator
        .probing()
        .configure(
            0x0002,
            [9; 8],
            LinkMetrics {
                lqi: true,
                link_margin: true,
                ..LinkMetrics::NONE
            },
        )
        .unwrap();
    *bench.radio.enhanced_ack() = Some(generator);

    bench.deliver(&PLAIN_2015);
    bench.interrupt(&[Ieee802154Event::RxDone]);
    // LQI 200 and the margin of -60 dBm over -97 dBm, 37 * 255 / 130.
    let probing_ie = [0x06, 0x00, 0x9b, 0xb8, 0xea, 0x00, 200, 72];
    let ack = bench.transmit_image();
    assert!(
        ack.windows(probing_ie.len())
            .any(|window| window == probing_ie)
    );
    bench.interrupt(&[Ieee802154Event::AckTxDone]);

    let mut other = PLAIN_2015;
    other[7] = 0x03;
    bench.deliver(&other);
    bench.interrupt(&[Ieee802154Event::RxDone]);
    let ack = bench.transmit_image();
    assert!(!ack.windows(3).any(|window| window == [0x9b, 0xb8, 0xea]));
}

/// A frame with a Time IE gets the time sync sequence and the network time,
/// the radio clock plus the offset, when its SFD goes out, as the port's
/// `ot_radio_transmit_sfd_done` writes them.
#[test]
fn the_time_ie_gets_the_network_time_at_the_sfd() {
    use oer_ieee802154::TimeSync;
    let mut bench = Bench::enabled();
    // A data frame with room for the Time IE content after its header.
    let mut mac = [0; 20];
    mac[..3].copy_from_slice(&[0x41, 0x88, 0x2a]);
    bench
        .submit(RadioCommand::Transmit(TxRequest {
            id: RequestId::new(3),
            frame: FrameView::new(&mac).unwrap(),
            channel: channel(11),
            mode: TxMode::Direct,
            transmit_power_dbm: None,
            max_frame_retries: 0,
            security: TxSecurity::Radio,
            interface: Interface::PRIMARY,
            time_sync: Some(TimeSync {
                ie_offset: 5,
                sequence: 9,
                network_time_offset: 1_000,
            }),
        }))
        .unwrap();
    bench.interrupt(&[Ieee802154Event::TxSfdDone]);
    let image = bench.transmit_image();
    // The PHR precedes the MAC bytes; the clock reads 42.
    assert_eq!(image[1 + 5], 9);
    assert_eq!(image[1 + 6..1 + 14], 1_042_u64.to_le_bytes());
    assert_eq!(image[1..4], [0x41, 0x88, 0x2a]);
}

fn cancel(bench: &mut Bench, target: u32) -> Result<(), CancelError> {
    bench.radio.cancel(
        &mut bench.hw,
        &bench.clock,
        RequestId::new(target),
        &mut bench.sink,
    )
}

/// A cancelled transmission stops the MAC, ends aborted and the radio
/// receives again on its resting channel.
#[test]
fn a_cancelled_transmission_ends_aborted_and_receive_resumes() {
    let mut bench = Bench::enabled();
    bench
        .submit(RadioCommand::Receive {
            id: RequestId::new(2),
            channel: channel(15),
        })
        .unwrap();
    bench.transmit(5, &DATA, 15, TxMode::Direct).unwrap();
    assert_eq!(bench.hw.command, Some(Ieee802154LlCommand::TxStart));
    assert_eq!(cancel(&mut bench, 6), Err(CancelError::NotRunning));
    cancel(&mut bench, 5).unwrap();
    assert_eq!(
        bench.seen(),
        [Seen::TransmitDone {
            id: RequestId::new(5),
            status: TxStatus::Aborted,
            ack_pending: None,
        }]
    );
    assert_eq!(
        bench.radio.state(),
        RadioState::Resting(RestingState::Receiving {
            channel: channel(15)
        })
    );
    assert_eq!(bench.hw.command, Some(Ieee802154LlCommand::RxStart));
    // The ended transmission is not cancelled twice.
    assert_eq!(cancel(&mut bench, 5), Err(CancelError::NotRunning));
}

/// Disabling a radio with a transmission in flight ends the transmission
/// with its terminal event first, then disables the radio.
#[test]
fn disabling_ends_the_transmission_in_flight_first() {
    let mut bench = Bench::enabled();
    bench.transmit(5, &DATA, 15, TxMode::Direct).unwrap();
    bench
        .radio
        .disable(&mut bench.hw, &bench.clock, &mut bench.sink)
        .unwrap();
    assert_eq!(
        bench.seen(),
        [Seen::TransmitDone {
            id: RequestId::new(5),
            status: TxStatus::Aborted,
            ack_pending: None,
        }]
    );
    assert_eq!(bench.radio.state(), RadioState::Disabled);
    assert_eq!(bench.radio.engine().state(), Ieee802154State::Sleep);
}

/// A cancellation while the MAC waits for the acknowledgement ends the
/// transmission with the missing acknowledgement, without the retries it
/// still had.
#[test]
fn a_cancelled_transmission_is_not_retried() {
    let mut bench = Bench::enabled();
    bench
        .submit(RadioCommand::Transmit(TxRequest {
            id: RequestId::new(5),
            frame: FrameView::new(&DATA_ACK).unwrap(),
            channel: channel(20),
            mode: TxMode::Direct,
            transmit_power_dbm: None,
            max_frame_retries: 3,
            security: Default::default(),
            interface: Interface::PRIMARY,
            time_sync: None,
        }))
        .unwrap();
    bench.interrupt(&[Ieee802154Event::TxDone]);
    assert!(bench.seen().is_empty(), "the MAC waits for the ACK");
    cancel(&mut bench, 5).unwrap();
    assert_eq!(
        bench.seen(),
        [Seen::TransmitDone {
            id: RequestId::new(5),
            status: TxStatus::NoAcknowledgement,
            ack_pending: None,
        }]
    );
    assert_eq!(
        bench.radio.state(),
        RadioState::Resting(RestingState::Sleeping)
    );
    assert_eq!(bench.radio.take_delay(), None, "no retry is due");
}

/// A transmission cancelled during its CSMA-CA backoff ends aborted.
#[test]
fn a_transmission_cancelled_in_its_backoff_ends_aborted() {
    let mut bench = Bench::enabled();
    bench
        .transmit(5, &DATA, 20, TxMode::CsmaCa { max_backoffs: 4 })
        .unwrap();
    assert!(bench.radio.take_delay().is_some(), "the backoff runs");
    cancel(&mut bench, 5).unwrap();
    assert_eq!(
        bench.seen(),
        [Seen::TransmitDone {
            id: RequestId::new(5),
            status: TxStatus::Aborted,
            ack_pending: None,
        }]
    );
    // The backoff's end finds nothing to continue.
    bench
        .radio
        .delay_elapsed(&mut bench.hw, &bench.clock, &mut bench.sink);
    assert!(bench.seen().is_empty());
    assert_ne!(bench.hw.command, Some(Ieee802154LlCommand::CcaTxStart));
    assert_eq!(
        bench.radio.state(),
        RadioState::Resting(RestingState::Sleeping)
    );
}

/// Cancelled measurements and windows report their failure or end.
#[test]
fn cancelled_measurements_and_windows_report_their_end() {
    let mut bench = Bench::enabled();
    bench
        .submit(RadioCommand::EnergyScan(EnergyScanRequest {
            id: RequestId::new(3),
            channel: channel(11),
            duration_us: 1_000,
        }))
        .unwrap();
    cancel(&mut bench, 3).unwrap();
    bench
        .submit(RadioCommand::ClearChannelAssessment {
            id: RequestId::new(4),
            channel: channel(11),
        })
        .unwrap();
    cancel(&mut bench, 4).unwrap();
    bench
        .submit(RadioCommand::ScheduledReceive(ScheduledReceiveRequest {
            id: RequestId::new(7),
            channel: channel(12),
            start: Ieee802154Instant::from_micros(10_000),
            duration_us: 1_000,
        }))
        .unwrap();
    cancel(&mut bench, 7).unwrap();
    assert_eq!(
        bench.seen(),
        [
            Seen::EnergyScanFailed,
            Seen::ClearChannelAssessmentFailed,
            Seen::ScheduledReceiveDone(RequestId::new(7)),
        ]
    );
    assert_eq!(
        bench.radio.state(),
        RadioState::Resting(RestingState::Sleeping)
    );
    assert_eq!(bench.radio.engine().state(), Ieee802154State::Sleep);
}
