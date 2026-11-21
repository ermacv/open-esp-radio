//! The ordinary queue transactions over host memory: what each publishes,
//! what it keeps, and what it refuses.

use super::*;
use crate::{radio::ieee80211::registers::HostWords, system::memory::dma_address::model_reference};
use oer_portable_memory::HardwareOwnedTxDma;

/// A prepared chain of a host model: its head is recorded, never followed.
struct ModelDma(DmaRef<WifiTxDmaDescriptor>);

#[allow(unsafe_code, reason = "a host model's head is never dereferenced")]
// SAFETY: host tests have no DMA engine; the head is only compared and
// published into host memory.
unsafe impl PreparedTxDma<DmaRef<WifiTxDmaDescriptor>> for ModelDma {
    fn descriptor_head(&self) -> DmaRef<WifiTxDmaDescriptor> {
        self.0
    }
}

#[allow(unsafe_code, reason = "a host model's head is never dereferenced")]
// SAFETY: as for `PreparedTxDma` above.
unsafe impl HardwareOwnedTxDma<DmaRef<WifiTxDmaDescriptor>> for ModelDma {
    fn descriptor_head(&self) -> DmaRef<WifiTxDmaDescriptor> {
        self.0
    }
}

impl crate::radio::ieee80211::dma::tx_storage::CommitOwner for ModelDma {}

impl CommitTxDma for ModelDma {
    type Started<'a> = ModelDma;

    fn commit(self, start: impl for<'a> FnOnce(&Self::Started<'a>)) {
        start(&self);
    }
}

const HEAD: u32 = 0x2f01_2344;

fn dma() -> ModelDma {
    ModelDma(model_reference(HEAD))
}

fn legacy_program(dma: &ModelDma) -> MacLegacyTxProgram {
    MacLegacyTxProgram::new(
        dma,
        MacLegacyTxParameters {
            control: MacTxControlFrame {
                protection: MacTxProtection::None,
                rate: MacLegacyRate::Ofdm6M,
                power_primary: 0x21,
                power_alternate: 0x22,
            },
            rate: MacLegacyRate::Ofdm24M,
            signal: 0x123,
            data_power: 0x33,
            timeout: 0x456,
            scheduler_priority: 0x5,
            packet_priority: 0x6,
            priority_count: 0x789,
            aifsn: 0x2,
            contention_window: 0x0f,
            interface: MacInterface::Station,
            response: MacLegacyTxResponse::Ack,
            hardware_key_selector: 0x11,
        },
    )
    .expect("the parameters lie in their fields")
}

#[test]
fn a_legacy_program_refuses_values_beyond_its_fields() {
    let dma = dma();
    let mut parameters = legacy_program(&dma).parameters;
    parameters.timeout = 0x1000;
    assert!(MacLegacyTxProgram::new(&dma, parameters).is_none());
}

#[test]
fn a_legacy_preparation_publishes_six_words_of_the_reversed_bank() {
    let dma = dma();
    let before = HostWords::filled(0);
    let mut memory = before.clone();
    assert!(
        memory
            .owners()
            .registers
            .prepare_bound_legacy_mac_tx(
                &dma,
                MacTxQueueIndex::constant::<0>(),
                legacy_program(&dma),
            )
            .is_ok()
    );

    // CONTROL and CONFIG of the queue control, PLCP1, PTI, LENGTH_CONTROL
    // and POWER of the queue vector; nothing else.
    let changes = before.changes(&memory);
    let control = changes
        .iter()
        .filter(|(part, _, _)| *part == "wifi_mac_tx_queue_control")
        .count();
    let vector = changes
        .iter()
        .filter(|(part, _, _)| *part == "wifi_mac_tx_queue_vector")
        .count();
    assert_eq!((control, vector, changes.len()), (2, 4, 6));

    let owners = memory.owners();
    let bank = queue::bank(MacTxQueueIndex::constant::<0>());
    let queue_control = &owners.registers.mac.wifi_mac_tx_queue_control;
    let control = queue_control.read_control_n(bank);
    assert_eq!(control.descriptor_address_low().get(), HEAD & 0xf_ffff);
    assert_eq!(control.bits_20_21_opaque().get(), 2);
    assert!(control.bit_22_opaque());
    assert_eq!(control.plcp_format().get(), 1);
    assert!(!control.valid() && !control.enable());
    let config = queue_control.read_config(bank);
    assert_eq!(config.timeout().get(), 0x456);
    assert_eq!(config.aifsn().get(), 0x2);
    assert_eq!(config.contention_window().get(), 0x0f);
    assert_eq!(config.scheduler_priority().get(), 0x5);
    let vectors = &owners.registers.mac.wifi_mac_tx_queue_vector;
    let plcp1 = vectors.read_plcp1(bank);
    assert_eq!(plcp1.legacy_signal().get(), 0x123);
    assert_eq!(plcp1.rate().get(), 0x09);
    assert_eq!(plcp1.key_entry_index().get(), 0x11);
    assert_eq!(plcp1.vector_format().get(), 0);
    let pti = vectors.read_pti(bank);
    assert_eq!(
        [pti.pti_0(), pti.pti_1(), pti.pti_2(), pti.pti_3()].map(|lane| lane.get()),
        [6; 4]
    );
    assert_eq!(pti.count().get(), 0x789);
    let length = vectors.read_length_control(bank);
    assert!(length.txrx_queue_antenna_enable());
    assert_eq!(length.rts_rate(), 0x0b);
    assert_eq!(length.ht20_eof_count().get(), 1);
    let power = vectors.read_power(bank);
    assert_eq!(
        (
            power.data_primary(),
            power.data_alternate(),
            power.rts_primary(),
            power.rts_alternate()
        ),
        (0x33, 0, 0x21, 0x22)
    );
}

#[test]
fn whole_word_publications_clear_every_other_field_while_updates_keep_them() {
    let dma = dma();
    let mut memory = HostWords::filled(u32::MAX);
    // An idle queue control; the vectors hold every bit set.
    memory.wifi_mac_tx_queue_control.fill(0);
    let mut owners = memory.owners();
    owners
        .registers
        .program_legacy_mac_tx_ppdu(MacTxQueueIndex::constant::<0>(), legacy_program(&dma));
    let bank = queue::bank(MacTxQueueIndex::constant::<0>());
    let vectors = &owners.registers.mac.wifi_mac_tx_queue_vector;
    let plcp1 = vectors.read_plcp1(bank);
    assert!(!plcp1.high_control_28() && !plcp1.cbw40());
    assert_eq!(plcp1.high_control_31_30().get(), 0);
    let length = vectors.read_length_control(bank);
    assert!(!length.tx_queue_antenna_enable() && !length.txrx_queue_antenna_last());
    assert_eq!(length.bits_14_21_opaque(), 0);
    assert_eq!(length.ht20_txop_count().get(), 0);
    let pti = vectors.read_pti(bank);
    assert_eq!(pti.txop(), crate::types::MacTxop::None);
    assert_eq!(pti.pti_0().get(), 6);
    // The HE queue control keeps every field but the trigger-based enable.
    let he = owners
        .registers
        .mac
        .wifi_mac_he_init_suffix
        .read_queue_control(queue::he_queue_control(MacTxQueueIndex::constant::<0>()));
    assert!(!he.trigger_based_enable() && he.qos_null_to_translated_bss());
}

#[test]
fn a_busy_queue_is_not_prepared() {
    let dma = dma();
    let before = HostWords::filled(u32::MAX);
    let mut memory = before.clone();
    assert_eq!(
        memory.owners().registers.prepare_bound_legacy_mac_tx(
            &dma,
            MacTxQueueIndex::constant::<1>(),
            legacy_program(&dma),
        ),
        Err(MacTxPrepareRefused::QueueActive)
    );
    assert!(before.changes(&memory).is_empty());
}

#[test]
fn starting_a_bound_chain_sets_only_enable_and_valid() {
    let dma = dma();
    let mut memory = HostWords::filled(0);
    memory
        .owners()
        .registers
        .program_legacy_mac_tx_ppdu(MacTxQueueIndex::constant::<2>(), legacy_program(&dma));
    let before = memory.clone();
    assert_eq!(
        memory
            .owners()
            .registers
            .start_bound_mac_tx(dma, MacTxQueueIndex::constant::<2>(), || {}),
        Ok(())
    );
    let changes = before.changes(&memory);
    assert_eq!(changes.len(), 1);
    let control = memory
        .owners()
        .registers
        .mac
        .wifi_mac_tx_queue_control
        .read_control_n(queue::bank(MacTxQueueIndex::constant::<2>()));
    assert!(control.valid() && control.enable());
    assert_eq!(control.descriptor_address_low().get(), HEAD & 0xf_ffff);
}

#[test]
fn a_completion_is_read_from_the_alternate_result_after_a_trigger_based_send() {
    let before = HostWords::filled(u32::MAX);
    let mut memory = before.clone();
    let completion = memory
        .owners()
        .registers
        .take_mac_tx_completion(MacTxQueueIndex::constant::<1>())
        .expect("every completion bit reads set");
    assert_eq!(completion.last_transmission(), MacTxFlow::TriggerBased);
    assert_eq!(completion.flow(), MacTxFlow::TriggerBased);
    assert_eq!(completion.status(), 0xf);
    assert_eq!(completion.detail(), 0xff);
    assert_eq!(completion.trigger_based_packet_count(), 0x7f);
    assert_eq!(completion.secondary_trigger_based_packet_count(), 0x7f);
    // The clear word is written back as read with queue one's bit, which it
    // already holds.
    assert!(before.changes(&memory).is_empty());
}

#[test]
fn no_completion_is_taken_while_none_is_pending() {
    let before = HostWords::filled(0);
    let mut memory = before.clone();
    assert!(
        memory
            .owners()
            .registers
            .take_mac_tx_completion(MacTxQueueIndex::constant::<0>())
            .is_none()
    );
    assert!(
        memory
            .owners()
            .registers
            .take_mac_block_ack_completion(MacTxQueueIndex::constant::<0>())
            .is_none()
    );
    let clock = TickingClock::default();
    assert_eq!(
        memory.owners().registers.abort_mac_tx(
            MacTxQueueIndex::constant::<0>(),
            MacTxAbortCause::TimeoutEdge,
            &clock,
            |_| ()
        ),
        MacTxAbortOutcome::NoEdge
    );
    assert!(before.changes(&memory).is_empty());
    assert_eq!(clock.elapsed(), oer_portable_time::Duration::from_micros(0));
}

#[test]
fn a_completed_queue_detaches_with_its_descriptor_head() {
    let mut memory = HostWords::filled(u32::MAX);
    let before = memory.clone();
    let outcome = memory.owners().registers.with_detached_mac_tx(
        MacTxQueueIndex::constant::<3>(),
        MacTxDetachReason::Completed,
        |detached| detached.confirms_descriptor_head(0x2fff_ffff),
    );
    assert_eq!(outcome, MacTxDetachOutcome::Detached(true));
    // Only CONTROL lost ENABLE and VALID.
    assert_eq!(before.changes(&memory).len(), 1);
}

/// A clock that moves one microsecond at every read, so a settle ends.
#[derive(Default)]
struct TickingClock {
    reads: core::cell::Cell<u64>,
}

impl TickingClock {
    fn elapsed(&self) -> oer_portable_time::Duration {
        oer_portable_time::Duration::from_micros(self.reads.get().saturating_sub(1))
    }
}

impl oer_portable_time::Clock for TickingClock {
    fn now(&self) -> oer_portable_time::Instant {
        let reads = self.reads.get();
        self.reads.set(reads + 1);
        oer_portable_time::Instant::from_micros(reads)
    }
}

fn valid_queue(memory: &mut HostWords, queue: MacTxQueueIndex, valid: bool) {
    memory
        .owners()
        .registers
        .mac
        .wifi_mac_tx_queue_control
        .modify_control_n(queue::bank(queue), |writer| {
            writer.set_valid(valid).set_enable(true);
        });
}

#[test]
fn a_forced_abort_of_a_valid_queue_settles_then_disables_it() {
    let mut memory = HostWords::filled(0);
    let queue = MacTxQueueIndex::constant::<0>();
    valid_queue(&mut memory, queue, true);
    let clock = TickingClock::default();
    let outcome = memory.owners().registers.abort_mac_tx(
        queue,
        MacTxAbortCause::Forced,
        &clock,
        |_aborted| 7,
    );
    assert_eq!(outcome, MacTxAbortOutcome::Aborted(7));
    // The vendor's delay ran whole before the queue was sampled.
    assert!(clock.elapsed() >= MAC_TX_ABORT_SETTLE.limit());
    let registers = &memory.owners().registers;
    let control = &registers.mac.wifi_mac_tx_queue_control;
    assert!(!queue::queue_enabled(control, queue));
    assert!(!queue::queue_valid(control, queue));
}

#[test]
fn an_abort_that_finds_valid_clear_leaves_the_queue_to_its_completion() {
    let mut memory = HostWords::filled(0);
    let queue = MacTxQueueIndex::constant::<0>();
    valid_queue(&mut memory, queue, false);
    let clock = TickingClock::default();
    let outcome =
        memory
            .owners()
            .registers
            .abort_mac_tx(queue, MacTxAbortCause::Forced, &clock, |_| ());
    assert_eq!(outcome, MacTxAbortOutcome::OnAir);
    assert!(queue::queue_enabled(
        &memory.owners().registers.mac.wifi_mac_tx_queue_control,
        queue
    ));
}

#[test]
fn a_collision_detach_without_the_event_changes_nothing() {
    let before = HostWords::filled(0);
    let mut memory = before.clone();
    let outcome = memory.owners().registers.with_detached_mac_tx(
        MacTxQueueIndex::constant::<0>(),
        MacTxDetachReason::Collision,
        |_| (),
    );
    assert_eq!(outcome, MacTxDetachOutcome::NoEvent);
    assert!(before.changes(&memory).is_empty());
}

#[test]
fn pti_programs_the_reversed_bank_in_five_lane_edges() {
    let mut memory = HostWords::filled(0);
    let pti = |value| MacPti::new(value).expect("a four-bit PTI");
    memory.owners().registers.set_tx_pti(
        MacTxQueueIndex::new(3).expect("an ordinary queue"),
        MacTxPtiProgram {
            scheduler_priority: pti(1),
            pti_2: pti(2),
            pti_1: pti(3),
            pti_0: pti(4),
            pti_3: pti(5),
            count: MacTxPtiCount::new(0xabc).expect("a twelve-bit count"),
        },
    );
    let owners = memory.owners();
    let bank = queue::bank(MacTxQueueIndex::constant::<3>());
    let lanes = owners.registers.mac.wifi_mac_tx_queue_vector.read_pti(bank);
    assert_eq!(
        [lanes.pti_0(), lanes.pti_1(), lanes.pti_2(), lanes.pti_3()].map(|lane| lane.get()),
        [4, 3, 2, 5]
    );
    assert_eq!(lanes.count().get(), 0xabc);
    assert_eq!(
        owners
            .registers
            .mac
            .wifi_mac_tx_queue_control
            .read_config(bank)
            .scheduler_priority()
            .get(),
        1
    );
}

#[test]
fn a_foreign_dma_chain_is_refused_before_writes() {
    let dma = dma();
    let foreign = ModelDma(model_reference(HEAD + 4));
    let before = HostWords::filled(0);
    let mut memory = before.clone();
    assert_eq!(
        memory.owners().registers.prepare_bound_legacy_mac_tx(
            &foreign,
            MacTxQueueIndex::constant::<0>(),
            legacy_program(&dma),
        ),
        Err(MacTxPrepareRefused::Fault(
            MacTxPrepareFault::ForeignProgram
        ))
    );
    assert!(before.changes(&memory).is_empty());
}

#[test]
fn a_chain_is_not_started_on_a_queue_prepared_for_another_since() {
    let first = dma();
    let second = ModelDma(model_reference(HEAD + 4));
    let mut memory = HostWords::filled(0);
    let queue = MacTxQueueIndex::constant::<0>();
    let first_program = legacy_program(&first);
    let second_program = legacy_program(&second);
    assert_eq!(
        memory
            .owners()
            .registers
            .prepare_bound_legacy_mac_tx(&first, queue, first_program),
        Ok(())
    );
    assert_eq!(
        memory
            .owners()
            .registers
            .prepare_bound_legacy_mac_tx(&second, queue, second_program),
        Ok(())
    );

    let handed_over = core::cell::Cell::new(false);
    assert_eq!(
        memory
            .owners()
            .registers
            .start_bound_mac_tx(first, queue, || handed_over.set(true)),
        Err(MacTxPrepareRefused::Fault(
            MacTxPrepareFault::ChainNotNamed {
                named: (HEAD + 4) & 0xf_ffff
            }
        ))
    );
    assert!(!handed_over.get(), "the first chain stays with its owner");
    let control = |memory: &mut HostWords| {
        memory
            .owners()
            .registers
            .mac
            .wifi_mac_tx_queue_control
            .read_control_n(queue::bank(queue))
    };
    let stopped = control(&mut memory);
    assert!(!stopped.enable() && !stopped.valid());

    assert_eq!(
        memory
            .owners()
            .registers
            .start_bound_mac_tx(second, queue, || handed_over.set(true)),
        Ok(())
    );
    assert!(handed_over.get());
    let started = control(&mut memory);
    assert!(started.enable() && started.valid());
}

/// Every non-HT rate of IEEE 802.11-2020 (clauses 15, 16 and 17: DSSS 1 and
/// 2 Mb/s, CCK 5.5 and 11 Mb/s, each with its preamble, and the eight OFDM
/// rates) against the value the register model names for it in
/// `PLCP1%s.RATE` and in `LENGTH_CONTROL%s.RTS_RATE`.
#[test]
fn every_non_ht_rate_selects_the_models_rate_of_the_same_speed_and_preamble() {
    use MacDsssPreamble::{Long, Short};
    let table = [
        (
            MacLegacyRate::Dsss1M,
            PlcpRate::Dsss1mLong,
            RtsRate::Dsss1mLong,
        ),
        (
            MacLegacyRate::Dsss2M(Long),
            PlcpRate::Dsss2mLong,
            RtsRate::Dsss2mLong,
        ),
        (
            MacLegacyRate::Dsss2M(Short),
            PlcpRate::Dsss2mShort,
            RtsRate::Dsss2mShort,
        ),
        (
            MacLegacyRate::Cck5M5(Long),
            PlcpRate::Cck5m5Long,
            RtsRate::Cck5m5Long,
        ),
        (
            MacLegacyRate::Cck5M5(Short),
            PlcpRate::Cck5m5Short,
            RtsRate::Cck5m5Short,
        ),
        (
            MacLegacyRate::Cck11M(Long),
            PlcpRate::Cck11mLong,
            RtsRate::Cck11mLong,
        ),
        (
            MacLegacyRate::Cck11M(Short),
            PlcpRate::Cck11mShort,
            RtsRate::Cck11mShort,
        ),
        (MacLegacyRate::Ofdm6M, PlcpRate::Ofdm6m, RtsRate::Ofdm6m),
        (MacLegacyRate::Ofdm9M, PlcpRate::Ofdm9m, RtsRate::Ofdm9m),
        (MacLegacyRate::Ofdm12M, PlcpRate::Ofdm12m, RtsRate::Ofdm12m),
        (MacLegacyRate::Ofdm18M, PlcpRate::Ofdm18m, RtsRate::Ofdm18m),
        (MacLegacyRate::Ofdm24M, PlcpRate::Ofdm24m, RtsRate::Ofdm24m),
        (MacLegacyRate::Ofdm36M, PlcpRate::Ofdm36m, RtsRate::Ofdm36m),
        (MacLegacyRate::Ofdm48M, PlcpRate::Ofdm48m, RtsRate::Ofdm48m),
        (MacLegacyRate::Ofdm54M, PlcpRate::Ofdm54m, RtsRate::Ofdm54m),
    ];
    for (rate, plcp, rts) in table {
        assert_eq!(rate.plcp_rate(), plcp, "{rate:?}");
        assert_eq!(rate.rts_rate(), rts, "{rate:?}");
        assert_eq!(MacLegacyRate::from_plcp_rate(plcp), Some(rate), "{rate:?}");
        assert_eq!(
            MacLegacyRate::from_code(rate.code()),
            Some(rate),
            "{rate:?}"
        );
    }
    // An HT rate is no non-HT rate.
    assert_eq!(MacLegacyRate::from_plcp_rate(PlcpRate::HtLongMcs0), None);
}

/// Every single-stream HT MCS (IEEE 802.11-2020 19.5, MCS 0-7) with the
/// normal (800 ns) and the short (400 ns) guard interval against the
/// register model's `PLCP1%s.RATE` names, and the MCS index of HT-SIG and of
/// the HT40 length word.
#[test]
fn every_ht_mcs_and_guard_interval_selects_the_models_ht_rate() {
    use oer_chips_esp32s31_pac::wifi_mac::{data_length::Ht40Mcs, ht_signal::Mcs};
    let table = [
        (
            MacHtMcs::Mcs0,
            PlcpRate::HtLongMcs0,
            PlcpRate::HtShortMcs0,
            Mcs::Mcs0,
            Ht40Mcs::Mcs0,
        ),
        (
            MacHtMcs::Mcs1,
            PlcpRate::HtLongMcs1,
            PlcpRate::HtShortMcs1,
            Mcs::Mcs1,
            Ht40Mcs::Mcs1,
        ),
        (
            MacHtMcs::Mcs2,
            PlcpRate::HtLongMcs2,
            PlcpRate::HtShortMcs2,
            Mcs::Mcs2,
            Ht40Mcs::Mcs2,
        ),
        (
            MacHtMcs::Mcs3,
            PlcpRate::HtLongMcs3,
            PlcpRate::HtShortMcs3,
            Mcs::Mcs3,
            Ht40Mcs::Mcs3,
        ),
        (
            MacHtMcs::Mcs4,
            PlcpRate::HtLongMcs4,
            PlcpRate::HtShortMcs4,
            Mcs::Mcs4,
            Ht40Mcs::Mcs4,
        ),
        (
            MacHtMcs::Mcs5,
            PlcpRate::HtLongMcs5,
            PlcpRate::HtShortMcs5,
            Mcs::Mcs5,
            Ht40Mcs::Mcs5,
        ),
        (
            MacHtMcs::Mcs6,
            PlcpRate::HtLongMcs6,
            PlcpRate::HtShortMcs6,
            Mcs::Mcs6,
            Ht40Mcs::Mcs6,
        ),
        (
            MacHtMcs::Mcs7,
            PlcpRate::HtLongMcs7,
            PlcpRate::HtShortMcs7,
            Mcs::Mcs7,
            Ht40Mcs::Mcs7,
        ),
    ];
    for (mcs, normal, short, signal, ht40) in table {
        for channel_width in [MacHtChannelWidth::Mhz20, MacHtChannelWidth::Mhz40] {
            let rate = |guard_interval| MacHtRate {
                mcs,
                guard_interval,
                channel_width,
            };
            assert_eq!(
                rate(MacHtGuardInterval::Normal).plcp_rate(),
                normal,
                "{mcs:?}"
            );
            assert_eq!(
                rate(MacHtGuardInterval::Short).plcp_rate(),
                short,
                "{mcs:?}"
            );
        }
        assert_eq!(mcs.signal_mcs(), signal, "{mcs:?}");
        assert_eq!(mcs.ht40_mcs(), ht40, "{mcs:?}");
    }
}

/// Every HE SU MCS this backend sends (IEEE 802.11ax-2021 27.5, MCS 0-9)
/// against the register model's `PLCP1%s.RATE` names and HE-SIG-A's MCS,
/// whatever the guard interval, coding and DCM.
#[test]
fn every_he_mcs_selects_the_models_he_rate() {
    use oer_chips_esp32s31_pac::wifi_mac::he_su_signal_a1::{GiLtf, Mcs};
    let table = [
        (MacHeMcs::Mcs0, PlcpRate::HeMcs0, Mcs::Mcs0),
        (MacHeMcs::Mcs1, PlcpRate::HeMcs1, Mcs::Mcs1),
        (MacHeMcs::Mcs2, PlcpRate::HeMcs2, Mcs::Mcs2),
        (MacHeMcs::Mcs3, PlcpRate::HeMcs3, Mcs::Mcs3),
        (MacHeMcs::Mcs4, PlcpRate::HeMcs4, Mcs::Mcs4),
        (MacHeMcs::Mcs5, PlcpRate::HeMcs5, Mcs::Mcs5),
        (MacHeMcs::Mcs6, PlcpRate::HeMcs6, Mcs::Mcs6),
        (MacHeMcs::Mcs7, PlcpRate::HeMcs7, Mcs::Mcs7),
        (MacHeMcs::Mcs8, PlcpRate::HeMcs8, Mcs::Mcs8),
        (MacHeMcs::Mcs9, PlcpRate::HeMcs9, Mcs::Mcs9),
    ];
    for (mcs, plcp, signal) in table {
        for guard_interval_and_ltf in [
            GiLtf::OneLtf800ns,
            GiLtf::TwoLtf800ns,
            GiLtf::TwoLtf1600ns,
            GiLtf::FourLtf3200ns,
        ] {
            for fec_coding in [MacHeFecCoding::Bcc, MacHeFecCoding::Ldpc] {
                for dcm in [false, true] {
                    let rate = MacHeRate {
                        mcs,
                        guard_interval_and_ltf,
                        fec_coding,
                        dcm,
                    };
                    assert_eq!(rate.plcp_rate(), plcp, "{mcs:?}");
                }
            }
        }
        assert_eq!(mcs.signal_mcs(), signal, "{mcs:?}");
    }
}

/// A TX queue's `PLCP1.BSSID_SELECT` holds its interface's selector, and its
/// `CONFIG.INTERFACE` the interface itself, for each of the four.
#[test]
fn a_tx_queue_of_each_interface_carries_its_selector() {
    let dma = dma();
    for (interface, selector) in [
        (MacInterface::Station, 0),
        (MacInterface::AccessPoint, 1),
        (MacInterface::Context2, 2),
        (MacInterface::Context3, 3),
    ] {
        let mut memory = HostWords::filled(0);
        let mut parameters = legacy_program(&dma).parameters;
        parameters.interface = interface;
        let program = MacLegacyTxProgram::new(&dma, parameters).expect("the parameters fit");
        memory
            .owners()
            .registers
            .prepare_bound_legacy_mac_tx(&dma, MacTxQueueIndex::constant::<0>(), program)
            .expect("an idle queue");
        let owners = memory.owners();
        let bank = queue::bank(MacTxQueueIndex::constant::<0>());
        let plcp1 = owners
            .registers
            .mac
            .wifi_mac_tx_queue_vector
            .read_plcp1(bank);
        assert_eq!(plcp1.bssid_select().get(), selector, "{interface:?}");
        let config = owners
            .registers
            .mac
            .wifi_mac_tx_queue_control
            .read_config(bank);
        assert_eq!(config.interface(), interface);
    }
}
