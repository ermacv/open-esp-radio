//! Closed ED-DONE/TIMER0 `EVENT_STATUS` validation transaction vocabulary.
//!
//! This module is intended only for a reset-isolated validation image. The
//! production API uses the generated affine W1C snapshot; these two named
//! writes remain feature-gated so the historical discriminator can compare
//! ED-DONE and TIMER0 independently without exposing a general raw image.

/// Fixed public-LL energy-detection duration used by the discriminator.
pub const VALIDATION_ED_DURATION: u32 = 8;

#[inline]
fn order_device_accesses() {
    crate::svd::device_access::fence();
}

/// Enable only RX-ABORT, ED-DONE, and TIMER0.
#[inline]
pub fn enable_ed_timer_abort_events(registers: &mut crate::svd::Ieee802154Mac) {
    registers.event_enable().modify(|_, writer| {
        writer.tx_done().clear_bit();
        writer.rx_done().clear_bit();
        writer.ack_tx_done().clear_bit();
        writer.ack_rx_done().clear_bit();
        writer.rx_abort().set_bit();
        writer.tx_abort().clear_bit();
        writer.ed_done().set_bit();
        writer.bit_7_opaque().clear_bit();
        writer.timer0_overflow().set_bit();
        writer.timer1_overflow().clear_bit();
        writer.clock_count_match().clear_bit();
        writer.tx_sfd_done().clear_bit();
        writer.rx_sfd_done().clear_bit();
        writer.bit_13_opaque().clear_bit()
    });
    order_device_accesses();
}

/// Replace only the event-enable field with zero during cleanup.
#[inline]
pub fn disable_all_events(registers: &mut crate::svd::Ieee802154Mac) {
    registers.event_enable().modify(|_, writer| {
        writer.tx_done().clear_bit();
        writer.rx_done().clear_bit();
        writer.ack_tx_done().clear_bit();
        writer.ack_rx_done().clear_bit();
        writer.rx_abort().clear_bit();
        writer.tx_abort().clear_bit();
        writer.ed_done().clear_bit();
        writer.bit_7_opaque().clear_bit();
        writer.timer0_overflow().clear_bit();
        writer.timer1_overflow().clear_bit();
        writer.clock_count_match().clear_bit();
        writer.tx_sfd_done().clear_bit();
        writer.rx_sfd_done().clear_bit();
        writer.bit_13_opaque().clear_bit()
    });
    order_device_accesses();
}

/// Return one semantic `EVENT_STATUS` sample through generated field readers.
#[inline]
pub fn event_status_events(
    registers: &crate::svd::Ieee802154Mac,
) -> crate::ieee802154::ownership::Ieee802154EventReadback {
    order_device_accesses();
    let events = crate::ieee802154::ownership::Ieee802154EventReadback::from_event_status(
        &registers.event_status().read(),
    );
    order_device_accesses();
    events
}

/// Select exactly ED_ABORT, ED_STOP and ED_COEX_REJECT abort reasons.
#[inline]
pub fn enable_ed_abort_reasons(registers: &mut crate::svd::Ieee802154Mac) {
    registers
        .rx_abort_enable()
        .modify(|_, writer| writer.events().ed_operation_reasons());
    order_device_accesses();
}

/// Mask every RX-abort reason during terminal cleanup.
#[inline]
pub fn disable_all_rx_abort_reasons(registers: &mut crate::svd::Ieee802154Mac) {
    registers
        .rx_abort_enable()
        .modify(|_, writer| writer.events().none());
    order_device_accesses();
}

/// Return the complete public energy-detection duration field.
#[inline]
pub fn ed_duration(registers: &crate::svd::Ieee802154Mac) -> u32 {
    order_device_accesses();
    let duration = registers.ed_duration().read().duration().bits();
    order_device_accesses();
    duration
}

/// Program the public standalone-CCA energy-detection duration, exactly eight.
#[inline]
#[allow(
    unsafe_code,
    reason = "the reviewed validation transaction admits only its constrained register operation"
)]
pub fn set_ed_duration_eight(registers: &mut crate::svd::Ieee802154Mac) {
    // SAFETY: the generated field is the reviewed low twenty-four-bit RW
    // duration. RMW matches the public bitfield assignment and retains the
    // adjacent high byte. This closed API admits only the fixed public-LL
    // standalone-CCA value eight.
    registers
        .ed_duration()
        .modify(|_reader, writer| unsafe { writer.duration().bits(VALIDATION_ED_DURATION) });
    order_device_accesses();
}

/// Return one complete TIMER0 counter sample.
#[inline]
pub fn timer0_value(registers: &crate::svd::Ieee802154Mac) -> u32 {
    order_device_accesses();
    let value = registers.timer0_value().read().value().bits();
    order_device_accesses();
    value
}

/// Program the validation timer with one nonzero field-sized threshold.
#[inline]
#[allow(
    unsafe_code,
    reason = "the reviewed validation transaction admits only its constrained register operation"
)]
pub fn set_timer0_threshold(registers: &mut crate::svd::Ieee802154Mac, threshold: u32) {
    // SAFETY: the generated threshold register is a complete 32-bit
    // write-only word. The validation HAL admits only a nonzero bounded value.
    unsafe {
        registers
            .timer0_threshold()
            .write_with_zero(|writer| writer.threshold().bits(threshold));
    }
    order_device_accesses();
}

/// Start TIMER0 through its source-confirmed finite opcode.
#[inline]
#[allow(
    unsafe_code,
    reason = "the reviewed validation transaction admits only its constrained register operation"
)]
pub fn start_timer0(registers: &mut crate::svd::Ieee802154Mac) {
    // SAFETY: the generated field admits this public-LL opcode and the command
    // is issued only inside the closed validation transaction.
    unsafe {
        registers
            .command()
            .write_with_zero(|writer| writer.opcode().timer0_start());
    }
    order_device_accesses();
}

/// Stop TIMER0 through its source-confirmed finite opcode.
#[inline]
#[allow(
    unsafe_code,
    reason = "the reviewed validation transaction admits only its constrained register operation"
)]
pub fn stop_timer0(registers: &mut crate::svd::Ieee802154Mac) {
    // SAFETY: see [`start_timer0`]; this is the paired finite stop opcode.
    unsafe {
        registers
            .command()
            .write_with_zero(|writer| writer.opcode().timer0_stop());
    }
    order_device_accesses();
}

/// Start the fixed-duration energy-detection stimulus.
#[inline]
#[allow(
    unsafe_code,
    reason = "the reviewed validation transaction admits only its constrained register operation"
)]
pub fn start_ed(registers: &mut crate::svd::Ieee802154Mac) {
    // SAFETY: the generated field admits the public-LL ED_START opcode. This
    // module does not claim that PHY/RF/BTBB prerequisites are ready; any abort
    // remains visible in the complete status evidence and fails the probe.
    unsafe {
        registers
            .command()
            .write_with_zero(|writer| writer.opcode().ed_start());
    }
    order_device_accesses();
}

/// Issue the public-LL STOP opcode as best-effort timeout cleanup.
#[inline]
#[allow(
    unsafe_code,
    reason = "the reviewed validation transaction admits only its constrained register operation"
)]
pub fn stop_operation(registers: &mut crate::svd::Ieee802154Mac) {
    // SAFETY: this is the finite public-LL STOP opcode. It is cleanup for a
    // validation timeout, not evidence that STOP is synchronous.
    unsafe {
        registers
            .command()
            .write_with_zero(|writer| writer.opcode().stop());
    }
    order_device_accesses();
}

/// Select only ED-DONE through its generated W1C field variant.
#[inline]
#[allow(
    unsafe_code,
    reason = "the reviewed validation transaction admits only its constrained register operation"
)]
pub fn write_ed_done_event(registers: &mut crate::svd::Ieee802154Mac) {
    // SAFETY: the generated field accessor selects only ED-DONE in this
    // reset-isolated validation transaction.
    unsafe {
        registers
            .event_status()
            .write_with_zero(|writer| writer.ed_done().bit(true));
    }
    order_device_accesses();
}

/// Select only TIMER0 through its generated W1C field variant.
#[inline]
#[allow(
    unsafe_code,
    reason = "the reviewed validation transaction admits only its constrained register operation"
)]
pub fn write_timer0_event(registers: &mut crate::svd::Ieee802154Mac) {
    // SAFETY: the generated field accessor selects only TIMER0 in this
    // reset-isolated validation transaction.
    unsafe {
        registers
            .event_status()
            .write_with_zero(|writer| writer.timer0_overflow().bit(true));
    }
    order_device_accesses();
}
