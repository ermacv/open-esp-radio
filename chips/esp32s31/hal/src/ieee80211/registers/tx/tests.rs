//! The ordinary queue transactions over host memory: what each publishes,
//! what it keeps, and what it refuses.

use super::*;
use crate::{ieee80211::registers::HostWords, memory::dma_address::model_reference};

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
    assert!(memory.owners().registers.prepare_bound_legacy_mac_tx(
        &dma,
        MacTxQueueIndex::constant::<0>(),
        legacy_program(&dma)
    ));

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
    assert!(!pti.txop());
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
    assert!(!memory.owners().registers.prepare_bound_legacy_mac_tx(
        &dma,
        MacTxQueueIndex::constant::<1>(),
        legacy_program(&dma)
    ));
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
    memory
        .owners()
        .registers
        .start_bound_mac_tx(&dma, MacTxQueueIndex::constant::<2>());
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
    assert!(completion.last_tx_was_trigger_based() && completion.trigger_flow());
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
    assert!(
        !memory
            .owners()
            .registers
            .begin_mac_tx_timeout_abort(MacTxQueueIndex::constant::<0>())
    );
    assert!(before.changes(&memory).is_empty());
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
fn a_foreign_dma_chain_is_refused_before_writes_with_static_panic_bytes() {
    extern crate std;
    let dma = dma();
    let foreign = ModelDma(model_reference(HEAD + 4));
    let before = HostWords::filled(0);
    let mut memory = before.clone();
    let panic = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        memory.owners().registers.prepare_bound_legacy_mac_tx(
            &foreign,
            MacTxQueueIndex::constant::<0>(),
            legacy_program(&dma),
        );
    }))
    .expect_err("a program belongs to its own DMA chain");
    assert!(before.changes(&memory).is_empty());
    assert_eq!(
        panic.downcast_ref::<&str>().map(|s| s.as_bytes()),
        Some(b"the program is bound to this DMA chain".as_slice())
    );
}
